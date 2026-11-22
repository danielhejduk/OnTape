use std::time::Duration;

use super::Reels;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Counter {
    gear: f64,
    offset: f64,
}

impl Counter {
    pub const DEFAULT_GEAR: f64 = 0.5;

    pub fn new() -> Counter {
        Counter::with_gear(Self::DEFAULT_GEAR)
    }

    pub fn with_gear(gear: f64) -> Counter {
        Counter { gear, offset: 0.0 }
    }

    pub fn from_parts(gear: f64, offset: f64) -> Counter {
        Counter { gear, offset }
    }

    pub fn gear(&self) -> f64 {
        self.gear
    }

    pub fn offset(&self) -> f64 {
        self.offset
    }

    pub fn value(&self, reels: &Reels, position: Duration) -> f64 {
        self.gear * reels.reel_turns(position) + self.offset
    }

    pub fn reading(&self, reels: &Reels, position: Duration) -> u16 {
        let whole = self.value(reels, position).floor() as i64;
        whole.rem_euclid(1000) as u16
    }

    pub fn reset(&mut self, reels: &Reels, position: Duration) {
        self.offset = -self.gear * reels.reel_turns(position);
    }

    /// Makes the counter read `value` at `position`.
    pub fn set_value(&mut self, reels: &Reels, position: Duration, value: f64) {
        self.offset = value - self.gear * reels.reel_turns(position);
    }

    pub fn swap_reel(&mut self, before: (&Reels, Duration), after: (&Reels, Duration)) {
        let value = self.value(before.0, before.1);
        self.set_value(after.0, after.1, value);
    }
}

impl Default for Counter {
    fn default() -> Self {
        Counter::new()
    }
}
