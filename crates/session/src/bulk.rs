//! Keeps file transfers from making a live remote-control session sluggish.
//!
//! The rate cap on the bulk lane follows queueing delay, LEDBAT-style: the round-trip time of the
//! (priority) ping is compared with the best seen recently. While it stays close to that floor the
//! transfer speeds up; once it inflates, meaning bulk data is filling buffers on the path, the cap is
//! cut hard. The result is that a transfer uses whatever the path can spare and steps aside for input.

const MIN_RATE: u64 = 32 * 1024;
/// Starts well below any usable link so a fresh transfer can never flood a slow connection before the first
/// delay measurement arrives; it then ramps up quickly on a link with room to spare.
const START_RATE: u64 = 64 * 1024;
const MAX_RATE: u64 = 256 * 1024 * 1024;
/// Queueing delay we are willing to add for the sake of a transfer.
const TARGET_MS: f32 = 25.0;
const BACK_OFF_MS: f32 = 60.0;

#[derive(Debug, Clone)]
pub struct BulkGovernor {
    floor_ms: Option<f32>,
    rate: u64,
}

impl Default for BulkGovernor {
    fn default() -> Self {
        Self {
            floor_ms: None,
            rate: START_RATE,
        }
    }
}

impl BulkGovernor {
    /// Feeds one round-trip sample; returns the new cap in bytes per second.
    pub fn update(&mut self, rtt_ms: f32) -> u64 {
        let floor = match self.floor_ms {
            // Follow the floor down immediately, up only slowly so route changes are tracked.
            Some(f) => f.min(rtt_ms) * 0.995 + rtt_ms.max(f.min(rtt_ms)) * 0.005,
            None => rtt_ms,
        };
        self.floor_ms = Some(floor);
        let delay = (rtt_ms - floor).max(0.0);
        self.rate = if delay > BACK_OFF_MS {
            (self.rate as f64 * 0.5) as u64
        } else if delay > TARGET_MS {
            (self.rate as f64 * 0.8) as u64
        } else if delay > TARGET_MS / 2.0 {
            // Close to the target: hold steady.
            self.rate
        } else {
            // Room to spare: multiplicative growth (samples arrive ten times a second while a file
            // moves), so LAN-speed links are used within a few seconds.
            self.rate + (self.rate / 8).max(16 * 1024)
        }
        .clamp(MIN_RATE, MAX_RATE);
        self.rate
    }

    pub fn rate(&self) -> u64 {
        self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_up_on_a_quiet_path_and_backs_off_when_delay_builds() {
        let mut g = BulkGovernor::default();
        for _ in 0..100 {
            g.update(20.0);
        }
        let fast = g.rate();
        assert!(fast > 20 * 1024 * 1024, "grew to {fast}");
        // Queueing delay appears: cap must fall quickly.
        for _ in 0..6 {
            g.update(140.0);
        }
        assert!(g.rate() < fast / 20, "backed off to {}", g.rate());
    }

    #[test]
    fn never_drops_below_a_floor_so_transfers_still_finish() {
        let mut g = BulkGovernor::default();
        g.update(10.0);
        for _ in 0..100 {
            g.update(900.0);
        }
        assert_eq!(g.rate(), MIN_RATE);
    }
}
