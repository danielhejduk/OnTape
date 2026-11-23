use std::sync::Mutex;
use std::time::{Duration, Instant};

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<Instant>,
}

impl ManualClock {
    pub fn new() -> ManualClock {
        ManualClock {
            now: Mutex::new(Instant::now()),
        }
    }

    pub fn advance(&self, by: Duration) {
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) += by;
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        ManualClock::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}
