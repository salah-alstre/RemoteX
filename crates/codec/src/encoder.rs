use crate::{annexb::nal_types, CodecError, EncodedFrame};
use remotex_common::peer::Codec;
use std::{mem::ManuallyDrop, sync::Once};
use windows::{
    core::{Interface, GUID},
    Win32::{
        Media::MediaFoundation::*,
        System::{
            Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED},
            Variant::VARIANT,
        },
    },
};

static INIT: Once = Once::new();

fn init_mf() -> Result<(), CodecError> {
    let mut result = Ok(());
    INIT.call_once(|| {
        // SAFETY: process-wide one-time initialisation of COM and Media Foundation.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                result = Err(CodecError::Windows(e));
            }
        }
    });
    result
}

#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub prefer_hardware: bool,
}

#[derive(Debug, Clone)]
pub struct EncoderInfo {
    pub codec: Codec,
    pub name: String,
    pub hardware: bool,
}

fn subtype(codec: Codec) -> Result<GUID, CodecError> {
    match codec {
        Codec::H264 => Ok(MFVideoFormat_H264),
        Codec::H265 => Ok(MFVideoFormat_HEVC),
        Codec::Av1 => Err(CodecError::Unsupported(codec)),
    }
}

struct Candidate {
    activate: IMFActivate,
    name: String,
    hardware: bool,
}

fn enumerate(codec: Codec, hardware: bool) -> Result<Vec<Candidate>, CodecError> {
    init_mf()?;
    let out = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype(codec)?,
    };
    let flags = if hardware {
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
    } else {
        MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    let mut found = Vec::new();
    // SAFETY: MFTEnumEx allocates `count` activates which we take ownership of, then free the array.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some(&out),
            &mut list,
            &mut count,
        )?;
        for i in 0..count as usize {
            if let Some(a) = (*list.add(i)).take() {
                let mut len = 0u32;
                let mut buf = [0u16; 256];
                let name = a
                    .GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len))
                    .map(|_| String::from_utf16_lossy(&buf[..len as usize]))
                    .unwrap_or_else(|_| "Unknown encoder".into());
                found.push(Candidate {
                    activate: a,
                    name,
                    hardware,
                });
            }
        }
        if !list.is_null() {
            CoTaskMemFree(Some(list as *const _));
        }
    }
    Ok(found)
}

/// Encoders installed on this machine, hardware first.
pub fn available_encoders() -> Vec<EncoderInfo> {
    let mut all = Vec::new();
    for codec in [Codec::H264, Codec::H265] {
        for hw in [true, false] {
            if let Ok(list) = enumerate(codec, hw) {
                all.extend(list.into_iter().map(|c| EncoderInfo {
                    codec,
                    name: c.name,
                    hardware: c.hardware,
                }));
            }
        }
    }
    all
}

pub struct Encoder {
    transform: IMFTransform,
    codec_api: Option<ICodecAPI>,
    events: Option<IMFMediaEventGenerator>,
    cfg: EncoderConfig,
    name: String,
    hardware: bool,
    frame_index: u64,
    frame_bytes: usize,
    out_buffer_size: u32,
    provides_samples: bool,
    sequence_header: Vec<u8>,
    need_input: u32,
    force_key: bool,
}

// SAFETY: the encoder is created and used on one dedicated thread; it is only moved before first use.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(cfg: EncoderConfig) -> Result<Self, CodecError> {
        let width = cfg.width & !1;
        let height = cfg.height & !1;
        let cfg = EncoderConfig { width, height, ..cfg };
        let mut candidates = Vec::new();
        if cfg.prefer_hardware {
            candidates.extend(enumerate(cfg.codec, true).unwrap_or_default());
        }
        candidates.extend(enumerate(cfg.codec, false).unwrap_or_default());
        if candidates.is_empty() {
            return Err(CodecError::NoEncoder(cfg.codec));
        }
        let mut last_err = CodecError::NoEncoder(cfg.codec);
        for c in candidates {
            match Self::configure(&c, &cfg) {
                Ok(enc) => {
                    tracing::info!(target: "codec", encoder = %enc.name, hardware = enc.hardware, ?cfg.codec, width, height, "encoder ready");
                    return Ok(enc);
                }
                Err(e) => {
                    tracing::warn!(target: "codec", encoder = %c.name, error = %e, "encoder rejected configuration");
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    fn configure(c: &Candidate, cfg: &EncoderConfig) -> Result<Self, CodecError> {
        // SAFETY: standard Media Foundation transform setup; all interfaces are ref counted.
        unsafe {
            let transform: IMFTransform = c.activate.ActivateObject()?;
            let attrs = transform.GetAttributes().ok();
            let mut is_async = false;
            if let Some(a) = &attrs {
                if a.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 1 {
                    a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                    is_async = true;
                }
                let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
            }

            let out_type = MFCreateMediaType()?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &subtype(cfg.codec)?)?;
            out_type.SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_kbps.max(100) * 1000)?;
            pack(&out_type, &MF_MT_FRAME_SIZE, cfg.width, cfg.height)?;
            pack(&out_type, &MF_MT_FRAME_RATE, cfg.fps.max(1), 1)?;
            pack(&out_type, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            if cfg.codec == Codec::H264 {
                out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;
            }
            transform.SetOutputType(0, &out_type, 0)?;

            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            pack(&in_type, &MF_MT_FRAME_SIZE, cfg.width, cfg.height)?;
            pack(&in_type, &MF_MT_FRAME_RATE, cfg.fps.max(1), 1)?;
            pack(&in_type, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &in_type, 0)?;

            let codec_api = transform.cast::<ICodecAPI>().ok();
            if let Some(api) = &codec_api {
                let set = |guid: &GUID, v: u32| {
                    let _ = api.SetValue(guid, &VARIANT::from(v));
                };
                set(&CODECAPI_AVLowLatencyMode, 1);
                set(
                    &CODECAPI_AVEncCommonRateControlMode,
                    eAVEncCommonRateControlMode_CBR.0 as u32,
                );
                set(&CODECAPI_AVEncCommonMeanBitRate, cfg.bitrate_kbps.max(100) * 1000);
                // Rate-control buffer of a quarter second: large enough for a sharp keyframe, small enough
                // that a burst never occupies the link for long (which would delay input behind it).
                set(
                    &CODECAPI_AVEncCommonBufferSize,
                    (cfg.bitrate_kbps.max(100) * 1000 / 4).max(60_000),
                );
                // No B-frames (they need future frames and add a frame of delay) and no look-ahead.
                set(&CODECAPI_AVEncMPVDefaultBPictureCount, 0);
                set(&CODECAPI_AVEncMPVGOPSize, cfg.fps.max(1) * 300);
                set(&CODECAPI_AVEncCommonQualityVsSpeed, 30);
            }

            let info = transform.GetOutputStreamInfo(0)?;
            let provides_samples = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
            let sequence_header = read_sequence_header(&transform);

            // Not every MFT implements FLUSH before streaming starts; only the start messages are mandatory.
            let _ = transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            let events = if is_async {
                transform.cast::<IMFMediaEventGenerator>().ok()
            } else {
                None
            };
            Ok(Self {
                transform,
                codec_api,
                events,
                cfg: cfg.clone(),
                name: c.name.clone(),
                hardware: c.hardware,
                frame_index: 0,
                frame_bytes: (cfg.width * cfg.height * 3 / 2) as usize,
                out_buffer_size: info.cbSize.max(cfg.width * cfg.height),
                provides_samples,
                sequence_header,
                need_input: 0,
                force_key: true,
            })
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_hardware(&self) -> bool {
        self.hardware
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.cfg
    }

    pub fn force_keyframe(&mut self) {
        self.force_key = true;
    }

    /// Adjusts the target bitrate without restarting the stream.
    pub fn set_bitrate(&mut self, kbps: u32) {
        self.cfg.bitrate_kbps = kbps;
        if let Some(api) = &self.codec_api {
            // SAFETY: valid GUID and VARIANT for a documented codec property.
            unsafe {
                let _ = api.SetValue(
                    &CODECAPI_AVEncCommonMeanBitRate,
                    &VARIANT::from(kbps.max(100) * 1000),
                );
            }
        }
    }

    /// Encodes one NV12 frame. May return zero frames while the encoder is priming.
    pub fn encode(&mut self, nv12: &[u8]) -> Result<Vec<EncodedFrame>, CodecError> {
        assert_eq!(
            nv12.len(),
            self.frame_bytes,
            "frame size must match the configured resolution"
        );
        // SAFETY: buffers are created and locked within this call; MFT calls follow the documented protocol.
        unsafe {
            if self.force_key {
                if let Some(api) = &self.codec_api {
                    let _ = api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
                }
                self.force_key = false;
            }
            let sample = self.make_sample(nv12)?;
            let mut out = Vec::new();
            match self.events.clone() {
                Some(events) => self.encode_async(&events, &sample, &mut out)?,
                None => {
                    self.transform.ProcessInput(0, &sample, 0)?;
                    self.drain(&mut out)?;
                }
            }
            self.frame_index += 1;
            Ok(out)
        }
    }

    unsafe fn make_sample(&self, nv12: &[u8]) -> Result<IMFSample, CodecError> {
        let buffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
        let mut ptr = std::ptr::null_mut();
        buffer.Lock(&mut ptr, None, None)?;
        std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
        buffer.Unlock()?;
        buffer.SetCurrentLength(nv12.len() as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        let dur = 10_000_000i64 / self.cfg.fps.max(1) as i64;
        sample.SetSampleTime(self.frame_index as i64 * dur)?;
        sample.SetSampleDuration(dur)?;
        Ok(sample)
    }

    /// Event-driven path used by hardware MFTs.
    unsafe fn encode_async(
        &mut self,
        events: &IMFMediaEventGenerator,
        sample: &IMFSample,
        out: &mut Vec<EncodedFrame>,
    ) -> Result<(), CodecError> {
        let mut fed = false;
        let mut idle_events = 0;
        loop {
            if !fed && self.need_input > 0 {
                self.transform.ProcessInput(0, sample, 0)?;
                self.need_input -= 1;
                fed = true;
                continue;
            }
            let event = events.GetEvent(MF_EVENT_FLAG_NONE)?;
            let kind = MF_EVENT_TYPE(event.GetType()? as i32);
            if kind == METransformNeedInput {
                self.need_input += 1;
            } else if kind == METransformHaveOutput {
                if let Some(f) = self.process_output()? {
                    out.push(f);
                }
                if fed {
                    return Ok(());
                }
            } else {
                idle_events += 1;
                if idle_events > 64 {
                    return Err(CodecError::NoOutput);
                }
            }
        }
    }

    unsafe fn drain(&mut self, out: &mut Vec<EncodedFrame>) -> Result<(), CodecError> {
        loop {
            match self.process_output() {
                Ok(Some(f)) => out.push(f),
                Ok(None) => return Ok(()),
                Err(CodecError::Windows(e)) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }

    /// Returns Ok(None) when the transform asks for more input.
    unsafe fn process_output(&mut self) -> Result<Option<EncodedFrame>, CodecError> {
        for _ in 0..2 {
            let own_sample = if self.provides_samples {
                None
            } else {
                let s = MFCreateSample()?;
                s.AddBuffer(&MFCreateMemoryBuffer(self.out_buffer_size)?)?;
                Some(s)
            };
            let mut buf = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(own_sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let result = self.transform.ProcessOutput(0, &mut buf, &mut status);
            let [b] = buf;
            let sample = ManuallyDrop::into_inner(b.pSample);
            drop(ManuallyDrop::into_inner(b.pEvents));
            match result {
                Ok(()) => {
                    let Some(sample) = sample else { return Ok(None) };
                    return Ok(Some(self.extract(&sample)?));
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    let t = self.transform.GetOutputAvailableType(0, 0)?;
                    self.transform.SetOutputType(0, &t, 0)?;
                    self.sequence_header = read_sequence_header(&self.transform);
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(None)
    }

    unsafe fn extract(&self, sample: &IMFSample) -> Result<EncodedFrame, CodecError> {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut ptr, None, Some(&mut len))?;
        let mut data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        buffer.Unlock()?;
        let keyframe = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) == 1;
        if keyframe && !self.sequence_header.is_empty() {
            let hevc = self.cfg.codec == Codec::H265;
            let have_params = nal_types(&data, hevc)
                .iter()
                .any(|t| if hevc { *t == 33 } else { *t == 7 });
            if !have_params {
                let mut with = self.sequence_header.clone();
                with.extend_from_slice(&data);
                data = with;
            }
        }
        Ok(EncodedFrame { data, keyframe })
    }
}

/// Equivalent of the MFSetAttributeSize/MFSetAttributeRatio inline helpers (two u32 packed in a u64).
unsafe fn pack(t: &IMFMediaType, key: &GUID, hi: u32, lo: u32) -> windows::core::Result<()> {
    t.SetUINT64(key, ((hi as u64) << 32) | lo as u64)
}

unsafe fn read_sequence_header(t: &IMFTransform) -> Vec<u8> {
    let Ok(ty) = t.GetOutputCurrentType(0) else {
        return Vec::new();
    };
    let Ok(size) = ty.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) else {
        return Vec::new();
    };
    let mut buf = vec![0u8; size as usize];
    let _ = ty.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut buf, None);
    buf
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: best-effort orderly shutdown of the stream.
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
    }
}
