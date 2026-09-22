//! Exercises the real capture -> encode pipeline. Skips itself on machines without a desktop session.
#![cfg(windows)]

use remotex_capture::{list_monitors, Capturer};
use remotex_codec::{available_encoders, nal_types, Encoder, EncoderConfig};
use remotex_common::peer::Codec;
use std::time::Instant;

fn run(codec: Codec, hardware: bool) {
    remotex_capture::enable_dpi_awareness();
    let Ok(monitors) = list_monitors() else { return };
    let Some(m) = monitors.first() else { return };
    let Ok(mut cap) = Capturer::new(m) else {
        eprintln!("desktop duplication unavailable, skipping");
        return;
    };
    let (w, h) = (1280u32, 720u32);
    let mut enc = match Encoder::new(EncoderConfig {
        codec,
        width: w,
        height: h,
        fps: 60,
        bitrate_kbps: 8000,
        prefer_hardware: hardware,
    }) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{codec:?} hardware={hardware}: no encoder ({e}), skipping");
            return;
        }
    };
    eprintln!(
        "monitor {}x{}; encoder '{}' hardware={}",
        m.width,
        m.height,
        enc.name(),
        enc.is_hardware()
    );

    let (mut frames, mut bytes, mut keyframes, mut cap_ms, mut enc_ms) = (0, 0usize, 0, 0.0, 0.0);
    let mut first_key_nals = Vec::new();
    let start = Instant::now();
    while frames < 40 && start.elapsed().as_secs() < 15 {
        cap.request_frame();
        let t = Instant::now();
        let got = cap.next(w, h, 50).expect("capture");
        cap_ms += t.elapsed().as_secs_f64() * 1000.0;
        let Some(f) = got.frame else { continue };
        assert_eq!((f.width, f.height), (w, h));
        let t = Instant::now();
        let out = enc.encode(&f.data).expect("encode");
        enc_ms += t.elapsed().as_secs_f64() * 1000.0;
        for o in out {
            if o.keyframe {
                keyframes += 1;
                if first_key_nals.is_empty() {
                    first_key_nals = nal_types(&o.data, codec == Codec::H265);
                }
            }
            bytes += o.data.len();
            frames += 1;
        }
    }
    eprintln!(
        "{frames} frames, {keyframes} keyframes, {bytes} bytes, avg capture+convert {:.2} ms, avg encode {:.2} ms, first key NALs {first_key_nals:?}",
        cap_ms / frames.max(1) as f64,
        enc_ms / frames.max(1) as f64
    );
    assert!(frames > 0, "encoder produced no frames");
    assert!(keyframes >= 1, "no keyframe");
    let need = if codec == Codec::H265 {
        [32u8, 33, 34]
    } else {
        [7u8, 8, 5]
    };
    if codec == Codec::H264 {
        assert!(
            first_key_nals.contains(&7) && first_key_nals.contains(&8),
            "keyframe must carry SPS/PPS: {first_key_nals:?}"
        );
    } else {
        assert!(
            first_key_nals.contains(&need[0]) || first_key_nals.contains(&33),
            "HEVC keyframe must carry parameter sets"
        );
    }
}

#[test]
fn lists_encoders() {
    for e in available_encoders() {
        eprintln!("{:?} '{}' hardware={}", e.codec, e.name, e.hardware);
    }
}

#[test]
fn h264_hardware() {
    run(Codec::H264, true);
}

#[test]
fn h264_software() {
    run(Codec::H264, false);
}

#[test]
fn h265_hardware() {
    run(Codec::H265, true);
}
