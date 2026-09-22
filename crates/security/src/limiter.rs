//! Failure counting with temporary lockout, used for password guessing and connection floods.

use std::{
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

pub struct AttemptLimiter<K: Eq + Hash + Clone> {
    max_failures: u32,
    window: Duration,
    lockout: Duration,
    state: HashMap<K, Entry>,
}

struct Entry {
    failures: Vec<Instant>,
    locked_until: Option<Instant>,
}

impl<K: Eq + Hash + Clone> AttemptLimiter<K> {
    pub fn new(max_failures: u32, window: Duration, lockout: Duration) -> Self {
        Self {
            max_failures,
            window,
            lockout,
            state: HashMap::new(),
        }
    }

    /// Remaining lockout, if the key is currently banned.
    pub fn locked(&mut self, key: &K, now: Instant) -> Option<Duration> {
        let e = self.state.get_mut(key)?;
        match e.locked_until {
            Some(t) if t > now => Some(t - now),
            Some(_) => {
                self.state.remove(key);
                None
            }
            None => None,
        }
    }

    /// Records a failure; returns true if this failure triggered a lockout.
    pub fn record_failure(&mut self, key: &K, now: Instant) -> bool {
        let e = self.state.entry(key.clone()).or_insert(Entry {
            failures: Vec::new(),
            locked_until: None,
        });
        e.failures.retain(|t| now.duration_since(*t) < self.window);
        e.failures.push(now);
        if e.failures.len() as u32 >= self.max_failures {
            e.locked_until = Some(now + self.lockout);
            e.failures.clear();
            true
        } else {
            false
        }
    }

    pub fn record_success(&mut self, key: &K) {
        self.state.remove(key);
    }

    /// Drops expired entries to bound memory.
    pub fn sweep(&mut self, now: Instant) {
        let window = self.window;
        self.state.retain(|_, e| match e.locked_until {
            Some(t) => t > now,
            None => e.failures.iter().any(|f| now.duration_since(*f) < window),
        });
    }

    pub fn len(&self) -> usize {
        self.state.len()
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_after_threshold_and_expires() {
        let mut l = AttemptLimiter::new(3, Duration::from_secs(60), Duration::from_secs(300));
        let t0 = Instant::now();
        assert!(!l.record_failure(&"a", t0));
        assert!(!l.record_failure(&"a", t0));
        assert!(l.record_failure(&"a", t0));
        assert!(l.locked(&"a", t0 + Duration::from_secs(10)).is_some());
        assert!(l.locked(&"b", t0).is_none());
        assert!(l.locked(&"a", t0 + Duration::from_secs(301)).is_none());
    }

    #[test]
    fn old_failures_age_out() {
        let mut l = AttemptLimiter::new(2, Duration::from_secs(10), Duration::from_secs(60));
        let t0 = Instant::now();
        l.record_failure(&1, t0);
        assert!(!l.record_failure(&1, t0 + Duration::from_secs(11)));
    }

    #[test]
    fn success_clears() {
        let mut l = AttemptLimiter::new(2, Duration::from_secs(10), Duration::from_secs(60));
        let t0 = Instant::now();
        l.record_failure(&1, t0);
        l.record_success(&1);
        assert!(!l.record_failure(&1, t0));
    }
}
