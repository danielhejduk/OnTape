mod cassette;
mod counter;
mod layout;
mod recording;
mod reels;

use std::fmt;
use std::time::Duration;

pub use cassette::Cassette;
pub use counter::Counter;
pub use layout::{LayoutError, LayoutOptions, TrackInfo, layout_album};
pub use recording::{Recording, Segment, Source};
pub use reels::{Reels, TAPE_SPEED, Wind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrackId(pub u64);

/// A tape's id in the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TapeId(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    A,
    B,
}

impl Side {
    pub const fn flipped(self) -> Side {
        match self {
            Side::A => Side::B,
            Side::B => Side::A,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Side::A => 0,
            Side::B => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TapeKind {
    Album,
    Mixtape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapeError {
    WriteProtected,
    Invalid(&'static str),
}

impl fmt::Display for TapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TapeError::WriteProtected => f.write_str("tape is write-protected"),
            TapeError::Invalid(why) => write!(f, "invalid tape data: {why}"),
        }
    }
}

impl std::error::Error for TapeError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tape {
    kind: TapeKind,
    cassette: Cassette,
    sides: [Recording; 2],
    head: Duration,
    facing: Side,
}

impl Tape {
    /// A new blank mixtape, wound to the start of Side A.
    pub fn blank(cassette: Cassette) -> Tape {
        let len = cassette.side_len();
        Tape {
            kind: TapeKind::Mixtape,
            cassette,
            sides: [Recording::blank(len), Recording::blank(len)],
            head: Duration::ZERO,
            facing: Side::A,
        }
    }

    /// An album tape, wound to the start of Side A.
    pub(crate) fn album(cassette: Cassette, side_a: Recording, side_b: Recording) -> Tape {
        debug_assert_eq!(side_a.len(), cassette.side_len());
        debug_assert_eq!(side_b.len(), cassette.side_len());
        Tape {
            kind: TapeKind::Album,
            cassette,
            sides: [side_a, side_b],
            head: Duration::ZERO,
            facing: Side::A,
        }
    }

    /// Rebuilds a tape from saved parts, checking that they fit together.
    pub fn from_parts(
        kind: TapeKind,
        cassette: Cassette,
        sides: [Recording; 2],
        head: Duration,
        facing: Side,
    ) -> Result<Tape, TapeError> {
        let side_len = cassette.side_len();
        if sides.iter().any(|r| r.len() != side_len) {
            return Err(TapeError::Invalid(
                "side length does not match the cassette",
            ));
        }
        if head > side_len {
            return Err(TapeError::Invalid("head is past the end of the tape"));
        }
        Ok(Tape {
            kind,
            cassette,
            sides,
            head,
            facing,
        })
    }

    pub fn kind(&self) -> TapeKind {
        self.kind
    }

    pub fn cassette(&self) -> Cassette {
        self.cassette
    }

    pub fn side_len(&self) -> Duration {
        self.cassette.side_len()
    }

    /// Reel geometry and default wind speed of this tape.
    pub fn reels(&self) -> Reels {
        Reels::for_cassette(self.cassette)
    }

    pub fn is_write_protected(&self) -> bool {
        self.kind == TapeKind::Album
    }

    pub fn recording(&self, side: Side) -> &Recording {
        &self.sides[side.index()]
    }

    /// The side facing out (the one the deck plays).
    pub fn facing(&self) -> Side {
        self.facing
    }
    pub fn flip(&mut self) {
        self.facing = self.facing.flipped();
    }

    /// Raw head position, measured from the start of Side A.
    pub fn head(&self) -> Duration {
        self.head
    }

    /// Position on the side facing out.
    pub fn position(&self) -> Duration {
        self.position_on(self.facing)
    }

    /// Position as seen from `side`.
    pub fn position_on(&self, side: Side) -> Duration {
        match side {
            Side::A => self.head,
            Side::B => self.side_len() - self.head,
        }
    }
    pub fn set_position(&mut self, pos: Duration) {
        let pos = pos.min(self.side_len());
        self.head = match self.facing {
            Side::A => pos,
            Side::B => self.side_len() - pos,
        };
    }

    /// Tape left before the end of the side facing out.
    pub fn remaining(&self) -> Duration {
        self.side_len() - self.position()
    }

    pub fn at_start(&self) -> bool {
        self.position().is_zero()
    }

    pub fn at_end(&self) -> bool {
        self.remaining().is_zero()
    }
    pub fn wind(&mut self, dir: Direction, by: Duration) -> Duration {
        let pos = self.position();
        let moved = match dir {
            Direction::Forward => by.min(self.remaining()),
            Direction::Backward => by.min(pos),
        };
        self.set_position(match dir {
            Direction::Forward => pos + moved,
            Direction::Backward => pos - moved,
        });
        moved
    }

    /// What the head is over on the side facing out, or `None` at the end.
    pub fn source_under_head(&self) -> Option<Source> {
        self.recording(self.facing).source_at(self.position())
    }
    pub fn record(&mut self, len: Duration, source: Source) -> Result<Duration, TapeError> {
        if self.is_write_protected() {
            return Err(TapeError::WriteProtected);
        }
        let pos = self.position();
        let written = self.sides[self.facing.index()].write(pos, len, source);
        self.wind(Direction::Forward, written);
        Ok(written)
    }
}
