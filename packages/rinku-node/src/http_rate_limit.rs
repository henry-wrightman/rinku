//! Simple per-key sliding-window HTTP rate limiter.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Sliding-window limiter: at most `max` events per `window`.
#[derive(Debug)]
pub struct SlidingWindowLimiter {
    max: usize,
    window: Duration,
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl SlidingWindowLimiter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            max: max.max(1) as usize,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true if the key is allowed (and records the hit).
    pub fn check_and_record(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = match self.hits.lock() {
            Ok(g) => g,
            // Fail-closed on poison would DoS the API; fail-open is safer for availability.
            Err(_) => return true,
        };

        let queue = map.entry(key.to_string()).or_default();
        while let Some(front) = queue.front() {
            if now.duration_since(*front) > self.window {
                queue.pop_front();
            } else {
                break;
            }
        }

        if queue.len() >= self.max {
            return false;
        }
        queue.push_back(now);

        // Opportunistic cleanup of idle keys
        if map.len() > 10_000 {
            map.retain(|_, q| q.front().is_some_and(|t| now.duration_since(*t) <= self.window));
        }

        true
    }

    pub fn max(&self) -> usize {
        self.max
    }

    pub fn window_secs(&self) -> u64 {
        self.window.as_secs()
    }
}

#[derive(Debug)]
pub struct HttpRateLimiters {
    pub tx: SlidingWindowLimiter,
    pub contract: SlidingWindowLimiter,
    pub general: SlidingWindowLimiter,
}

impl HttpRateLimiters {
    pub fn from_config(tx_max: u32, contract_max: u32, general_max: u32) -> Self {
        let window = Duration::from_secs(60);
        Self {
            tx: SlidingWindowLimiter::new(tx_max, window),
            contract: SlidingWindowLimiter::new(contract_max, window),
            general: SlidingWindowLimiter::new(general_max, window),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_under_limit() {
        let lim = SlidingWindowLimiter::new(3, Duration::from_secs(60));
        assert!(lim.check_and_record("a"));
        assert!(lim.check_and_record("a"));
        assert!(lim.check_and_record("a"));
    }

    #[test]
    fn rejects_over_limit() {
        let lim = SlidingWindowLimiter::new(2, Duration::from_secs(60));
        assert!(lim.check_and_record("b"));
        assert!(lim.check_and_record("b"));
        assert!(!lim.check_and_record("b"));
    }

    #[test]
    fn keys_are_independent() {
        let lim = SlidingWindowLimiter::new(1, Duration::from_secs(60));
        assert!(lim.check_and_record("x"));
        assert!(lim.check_and_record("y"));
        assert!(!lim.check_and_record("x"));
    }
}
