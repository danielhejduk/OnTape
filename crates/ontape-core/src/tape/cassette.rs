use std::fmt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cassette {
    C46,
    C60,
    C90,
    C120,
}
impl Cassette {
    pub const ALL: [Cassette; 4] = [Cassette::C46, Cassette::C60, Cassette::C90, Cassette::C120];
    pub const fn total_minutes(self) -> u64 {
        match self {
            Cassette::C46 => 46,
            Cassette::C60 => 60,
            Cassette::C90 => 90,
            Cassette::C120 => 120,
        }
    }

    pub const fn side_len(self) -> Duration {
        Duration::from_secs(self.total_minutes() * 60 / 2)
    }

    pub const fn longest() -> Cassette {
        Cassette::C120
    }

    pub fn smallest_fitting(side: Duration) -> Option<Cassette> {
        Self::ALL.into_iter().find(|c| c.side_len() >= side)
    }
}

impl fmt::Display for Cassette {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "C{}", self.total_minutes())
    }
}
