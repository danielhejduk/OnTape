use std::f64::consts::PI;
use std::time::{Duration, Instant};

use super::{Cassette, Direction};

pub const TAPE_SPEED: f64 = 0.047_625;

const HUB_RADIUS: f64 = 0.011;

const CUE_SLOWDOWN: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reels {
    hub: f64,
    thickness: f64,
    side_len: Duration,
    full_wind: Duration,
}

impl Reels {
    pub fn for_cassette(cassette: Cassette) -> Reels {
        let thickness_um = match cassette {
            Cassette::C46 | Cassette::C60 => 18.0,
            Cassette::C90 => 12.0,
            Cassette::C120 => 9.0,
        };
        Reels {
            hub: HUB_RADIUS,
            thickness: thickness_um * 1e-6,
            side_len: cassette.side_len(),
            full_wind: Duration::from_secs(cassette.total_minutes()),
        }
    }

    pub fn with_full_wind_time(self, full_wind: Duration) -> Reels {
        assert!(!full_wind.is_zero(), "a full wind must take some time");
        Reels { full_wind, ..self }
    }

    pub fn cueing(self) -> Reels {
        Reels {
            full_wind: self.full_wind * CUE_SLOWDOWN,
            ..self
        }
    }

    pub fn side_len(self) -> Duration {
        self.side_len
    }

    pub fn full_wind_time(&self) -> Duration {
        self.full_wind
    }

    pub fn reel_radius(&self, wound: Duration) -> f64 {
        let metres = wound.min(self.side_len).as_secs_f64() * TAPE_SPEED;
        (self.hub * self.hub + metres * self.thickness / PI).sqrt()
    }

    pub fn reel_turns(&self, wound: Duration) -> f64 {
        (self.reel_radius(wound) - self.hub) / self.thickness
    }

    pub fn reel_radii(&self, position: Duration) -> (f64, f64) {
        let position = position.min(self.side_len);
        (
            self.reel_radius(self.side_len - position),
            self.reel_radius(position),
        )
    }

    pub fn wind_time(&self, from: Duration, to: Duration) -> Duration {
        let (from, to) = (from.min(self.side_len), to.min(self.side_len));
        let growth = if to >= from {
            self.reel_radius(to) - self.reel_radius(from)
        } else {
            self.reel_radius(self.side_len - to) - self.reel_radius(self.side_len - from)
        };
        secs(growth / self.growth_rate())
    }

    pub fn wind_position(&self, from: Duration, dir: Direction, elapsed: Duration) -> Duration {
        let from = from.min(self.side_len);
        let grown = self.growth_rate() * elapsed.as_secs_f64();
        match dir {
            Direction::Forward => {
                if elapsed >= self.wind_time(from, self.side_len) {
                    return self.side_len;
                }
                self.wound_for_radius(self.reel_radius(from) + grown)
                    .min(self.side_len)
            }
            Direction::Backward => {
                if elapsed >= self.wind_time(from, Duration::ZERO) {
                    return Duration::ZERO;
                }
                let left = self.wound_for_radius(self.reel_radius(self.side_len - from) + grown);
                self.side_len.saturating_sub(left)
            }
        }
    }

    pub fn wind_speed(&self, position: Duration, dir: Direction) -> f64 {
        let position = position.min(self.side_len);
        let driven = match dir {
            Direction::Forward => self.reel_radius(position),
            Direction::Backward => self.reel_radius(self.side_len - position),
        };

        2.0 * PI * driven * self.growth_rate() / self.thickness / TAPE_SPEED
    }

    fn growth_rate(&self) -> f64 {
        (self.reel_radius(self.side_len) - self.hub) / self.full_wind.as_secs_f64()
    }

    fn wound_for_radius(&self, r: f64) -> Duration {
        let metres = PI * (r * r - self.hub * self.hub) / self.thickness;
        secs(metres / TAPE_SPEED)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wind {
    reels: Reels,
    from: Duration,
    dir: Direction,
    started: Instant,
}

impl Wind {
    pub fn start(reels: Reels, from: Duration, dir: Direction, now: Instant) -> Wind {
        Wind {
            reels,
            from: from.min(reels.side_len),
            dir,
            started: now,
        }
    }

    pub fn reels(&self) -> &Reels {
        &self.reels
    }

    pub fn from(&self) -> Duration {
        self.from
    }

    pub fn dir(&self) -> Direction {
        self.dir
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    pub fn position(&self, now: Instant) -> Duration {
        self.reels.wind_position(
            self.from,
            self.dir,
            now.saturating_duration_since(self.started),
        )
    }

    pub fn finishes_at(&self) -> Instant {
        let end = match self.dir {
            Direction::Forward => self.reels.side_len,
            Direction::Backward => Duration::ZERO,
        };
        self.started + self.reels.wind_time(self.from, end)
    }

    pub fn is_finished(&self, now: Instant) -> bool {
        now >= self.finishes_at()
    }

    pub fn speed(&self, now: Instant) -> f64 {
        if self.is_finished(now) {
            0.0
        } else {
            self.reels.wind_speed(self.position(now), self.dir)
        }
    }
}

fn secs(s: f64) -> Duration {
    if s.is_nan() || s <= 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(s)
    }
}
