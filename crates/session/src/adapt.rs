//! Adaptive streaming controller.
//!
//! Congestion (queueing delay, dropped frames, rising RTT) cuts quality quickly: bitrate first,
//! then frame rate, then resolution. Recovery is slow and stepwise, with a hold-off after every
//! change, so quality does not oscillate. Input responsiveness is protected separately by the
//! prioritised send queues, so the controller only has to care about the video budget.

use remotex_common::peer::{Preset, QualitySettings};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub bitrate_kbps: u32,
    pub fps: u32,
    /// Output height as a fraction of the native height, in 1/100ths.
    pub scale_percent: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    pub rtt_ms: f32,
    /// Time the last video frame took to leave the send queue.
    pub send_delay_ms: f32,
    /// Frames skipped since the previous sample because the queue was full.
    pub dropped: u32,
}

const FPS_STEPS: [u32; 3] = [60, 30, 15];
const SCALE_STEPS: [u32; 4] = [100, 75, 50, 33];

pub struct Adapter {
    quality: QualitySettings,
    native_pixels: u64,
    max_bitrate: u32,
    min_bitrate: u32,
    bitrate: u32,
    fps_idx: usize,
    scale_idx: usize,
    baseline_rtt: Option<f32>,
    good_streak: u32,
    last_change: Instant,
}

/// Bits per pixel budget for typical desktop content.
fn preset_bpp(p: Preset) -> f64 {
    match p {
        // Auto favours responsiveness: a modest bitrate keeps frames small so they leave the queue quickly.
        Preset::Auto | Preset::LowLatency => 0.04,
        Preset::Balanced => 0.06,
        Preset::BestQuality => 0.10,
        Preset::LowBandwidth => 0.015,
    }
}

impl Adapter {
    pub fn new(quality: QualitySettings, native_w: u32, native_h: u32, now: Instant) -> Self {
        let fps_cap = match (quality.fps, quality.preset) {
            (f, _) if f > 0 => f as u32,
            (_, Preset::LowBandwidth) => 30,
            _ => 60,
        };
        let fps_idx = FPS_STEPS
            .iter()
            .position(|f| *f <= fps_cap)
            .unwrap_or(FPS_STEPS.len() - 1);
        let native_pixels = native_w as u64 * native_h as u64;
        let capped_pixels = if quality.max_height > 0 && (quality.max_height as u32) < native_h {
            (native_w as u64 * quality.max_height as u64 * quality.max_height as u64) / native_h as u64
        } else if quality.preset == Preset::LowBandwidth && native_h > 720 {
            (native_w as u64 * 720 * 720) / native_h as u64
        } else {
            native_pixels
        };
        let fps = FPS_STEPS[fps_idx];
        let ideal = (capped_pixels as f64 * fps as f64 * preset_bpp(quality.preset) / 1000.0) as u32;
        let max_bitrate = if quality.bitrate_kbps > 0 {
            quality.bitrate_kbps
        } else {
            ideal.clamp(500, 60_000)
        };
        Self {
            quality,
            native_pixels,
            max_bitrate,
            min_bitrate: (max_bitrate / 8).max(300),
            bitrate: max_bitrate,
            fps_idx,
            scale_idx: 0,
            baseline_rtt: None,
            good_streak: 0,
            last_change: now,
        }
    }

    pub fn target(&self) -> Target {
        Target {
            bitrate_kbps: self.bitrate,
            fps: FPS_STEPS[self.fps_idx],
            scale_percent: SCALE_STEPS[self.scale_idx],
        }
    }

    pub fn native_pixels(&self) -> u64 {
        self.native_pixels
    }

    fn auto_bitrate(&self) -> bool {
        self.quality.bitrate_kbps == 0
    }

    fn auto_fps(&self) -> bool {
        self.quality.fps == 0 && self.quality.preset != Preset::BestQuality
    }

    fn auto_scale(&self) -> bool {
        self.quality.max_height == 0
    }

    /// Feed one sample per second. Returns a new target when something changed.
    pub fn update(&mut self, s: Sample, now: Instant) -> Option<Target> {
        if !self.quality.adaptive {
            return None;
        }
        if s.rtt_ms > 0.0 {
            self.baseline_rtt = Some(match self.baseline_rtt {
                Some(b) => b.min(s.rtt_ms) * 0.98 + s.rtt_ms.min(b * 3.0) * 0.02,
                None => s.rtt_ms,
            });
        }
        let base = self.baseline_rtt.unwrap_or(0.0);
        let congested =
            s.send_delay_ms > 60.0 || s.dropped >= 3 || (s.rtt_ms > 0.0 && s.rtt_ms > base * 2.0 + 60.0);
        let clean =
            s.send_delay_ms < 25.0 && s.dropped == 0 && (s.rtt_ms == 0.0 || s.rtt_ms < base * 1.5 + 30.0);
        let before = self.target();
        let hold = now.duration_since(self.last_change);

        if congested && hold >= Duration::from_millis(1500) {
            self.good_streak = 0;
            self.degrade();
        } else if clean {
            self.good_streak += 1;
            if self.good_streak >= 6 && hold >= Duration::from_secs(5) {
                self.good_streak = 3;
                self.improve();
            }
        } else {
            self.good_streak = 0;
        }

        let after = self.target();
        if after != before {
            self.last_change = now;
            Some(after)
        } else {
            None
        }
    }

    fn degrade(&mut self) {
        if self.auto_bitrate() && self.bitrate > self.min_bitrate {
            self.bitrate = (self.bitrate as f64 * 0.75) as u32;
            self.bitrate = self.bitrate.max(self.min_bitrate);
        } else if self.auto_fps() && self.fps_idx + 1 < FPS_STEPS.len() {
            self.fps_idx += 1;
            self.bitrate = self.bitrate.max(self.min_bitrate);
        } else if self.auto_scale() && self.scale_idx + 1 < SCALE_STEPS.len() {
            self.scale_idx += 1;
            self.max_bitrate_for_scale();
        }
    }

    fn improve(&mut self) {
        // Restore in the reverse order of degradation: resolution, then frame rate, then bitrate.
        if self.scale_idx > 0 && self.auto_scale() {
            self.scale_idx -= 1;
            self.max_bitrate_for_scale();
        } else if self.fps_idx > 0 && self.auto_fps() && self.bitrate >= self.min_bitrate * 2 {
            self.fps_idx -= 1;
        } else if self.auto_bitrate() && self.bitrate < self.max_bitrate {
            self.bitrate = ((self.bitrate as f64 * 1.15) as u32).min(self.max_bitrate);
        }
    }

    /// A lower resolution needs proportionally fewer bits.
    fn max_bitrate_for_scale(&mut self) {
        let f = SCALE_STEPS[self.scale_idx] as f64 / 100.0;
        self.bitrate = self
            .bitrate
            .min((self.max_bitrate as f64 * f * f).max(self.min_bitrate as f64) as u32);
    }
}

/// Output dimensions for a display at `scale_percent`, even and never below 320x180.
pub fn output_size(native_w: u32, native_h: u32, scale_percent: u32, max_height: u16) -> (u32, u32) {
    let mut h = native_h * scale_percent / 100;
    if max_height > 0 {
        h = h.min(max_height as u32);
    }
    let h = h.max(180).min(native_h.max(180));
    let w = (native_w as u64 * h as u64 / native_h.max(1) as u64) as u32;
    ((w.max(320)) & !1, h & !1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bad() -> Sample {
        Sample {
            rtt_ms: 200.0,
            send_delay_ms: 150.0,
            dropped: 5,
        }
    }
    fn good() -> Sample {
        Sample {
            rtt_ms: 20.0,
            send_delay_ms: 5.0,
            dropped: 0,
        }
    }

    #[test]
    fn congestion_reduces_bitrate_before_fps_before_resolution() {
        let t0 = Instant::now();
        let mut a = Adapter::new(QualitySettings::default(), 1920, 1080, t0);
        let start = a.target();
        let mut now = t0;
        let mut order = Vec::new();
        for _ in 0..40 {
            now += Duration::from_secs(2);
            let before = a.target();
            if let Some(t) = a.update(bad(), now) {
                if t.bitrate_kbps < before.bitrate_kbps {
                    order.push('b');
                }
                if t.fps < before.fps {
                    order.push('f');
                }
                if t.scale_percent < before.scale_percent {
                    order.push('s');
                }
            }
        }
        let first_f = order.iter().position(|c| *c == 'f').unwrap();
        let first_s = order.iter().position(|c| *c == 's').unwrap();
        assert!(order[0] == 'b' && first_f < first_s, "{order:?}");
        let end = a.target();
        assert!(end.bitrate_kbps < start.bitrate_kbps && end.fps == 15 && end.scale_percent == 33);
    }

    #[test]
    fn recovers_gradually_and_never_overshoots() {
        let t0 = Instant::now();
        let mut a = Adapter::new(QualitySettings::default(), 1920, 1080, t0);
        let max = a.target();
        let mut now = t0;
        for _ in 0..20 {
            now += Duration::from_secs(2);
            a.update(bad(), now);
        }
        assert!(a.target().bitrate_kbps < max.bitrate_kbps);
        let mut changes = 0;
        for _ in 0..600 {
            now += Duration::from_secs(1);
            if a.update(good(), now).is_some() {
                changes += 1;
            }
            assert!(a.target().bitrate_kbps <= max.bitrate_kbps);
        }
        assert_eq!(a.target(), max, "should fully recover on a clean link");
        assert!(
            changes < 40,
            "recovery must be stepwise, not chattering ({changes} changes)"
        );
    }

    #[test]
    fn no_oscillation_on_alternating_samples() {
        let t0 = Instant::now();
        let mut a = Adapter::new(QualitySettings::default(), 1920, 1080, t0);
        let mut now = t0;
        let mut changes = 0;
        for i in 0..120 {
            now += Duration::from_secs(1);
            let s = if i % 2 == 0 { bad() } else { good() };
            if a.update(s, now).is_some() {
                changes += 1;
            }
        }
        assert!(changes <= 20, "quality flapped {changes} times");
    }

    #[test]
    fn manual_settings_are_respected() {
        let t0 = Instant::now();
        let q = QualitySettings {
            fps: 30,
            bitrate_kbps: 4000,
            max_height: 720,
            ..Default::default()
        };
        let mut a = Adapter::new(q, 1920, 1080, t0);
        let mut now = t0;
        for _ in 0..30 {
            now += Duration::from_secs(2);
            a.update(bad(), now);
        }
        let t = a.target();
        assert_eq!((t.bitrate_kbps, t.fps, t.scale_percent), (4000, 30, 100));
    }

    #[test]
    fn adaptation_can_be_disabled() {
        let t0 = Instant::now();
        let q = QualitySettings {
            adaptive: false,
            ..Default::default()
        };
        let mut a = Adapter::new(q, 1920, 1080, t0);
        let start = a.target();
        let mut now = t0;
        for _ in 0..30 {
            now += Duration::from_secs(2);
            assert!(a.update(bad(), now).is_none());
        }
        assert_eq!(a.target(), start);
    }

    #[test]
    fn output_size_is_even_and_bounded() {
        assert_eq!(output_size(1920, 1080, 100, 0), (1920, 1080));
        assert_eq!(output_size(1920, 1080, 50, 0), (960, 540));
        assert_eq!(output_size(3840, 2160, 100, 1080), (1920, 1080));
        assert_eq!(output_size(2559, 1439, 100, 0).0 % 2, 0);
        let (w, h) = output_size(800, 600, 33, 0);
        assert!(w >= 320 && h >= 180);
    }

    #[test]
    fn presets_scale_the_budget() {
        let now = Instant::now();
        let mk = |p| {
            Adapter::new(
                QualitySettings {
                    preset: p,
                    ..Default::default()
                },
                1920,
                1080,
                now,
            )
            .target()
            .bitrate_kbps
        };
        assert!(mk(Preset::BestQuality) > mk(Preset::Balanced));
        assert!(mk(Preset::Balanced) > mk(Preset::LowBandwidth));
    }
}
