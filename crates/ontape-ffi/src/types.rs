use std::time::Duration;

use ontape_core::{deck, library, store, tape};

pub(crate) fn ms(d: Duration) -> u64 {
    d.as_millis().try_into().unwrap_or(u64::MAX)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Side {
    A,
    B,
}

impl From<tape::Side> for Side {
    fn from(s: tape::Side) -> Self {
        match s {
            tape::Side::A => Side::A,
            tape::Side::B => Side::B,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Direction {
    Forward,
    Backward,
}

impl From<tape::Direction> for Direction {
    fn from(d: tape::Direction) -> Self {
        match d {
            tape::Direction::Forward => Direction::Forward,
            tape::Direction::Backward => Direction::Backward,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Cassette {
    C46,
    C60,
    C90,
    C120,
}

impl From<tape::Cassette> for Cassette {
    fn from(c: tape::Cassette) -> Self {
        match c {
            tape::Cassette::C46 => Cassette::C46,
            tape::Cassette::C60 => Cassette::C60,
            tape::Cassette::C90 => Cassette::C90,
            tape::Cassette::C120 => Cassette::C120,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TapeKind {
    Album,
    Mixtape,
}

impl From<tape::TapeKind> for TapeKind {
    fn from(k: tape::TapeKind) -> Self {
        match k {
            tape::TapeKind::Album => TapeKind::Album,
            tape::TapeKind::Mixtape => TapeKind::Mixtape,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DeckState {
    Empty,
    Stopped,
    Playing,
    /// Fast-forward or rewind.
    Winding {
        direction: Direction,
    },
    /// Cue or review: winding fast while the play button is down.
    Cueing {
        direction: Direction,
    },
}

impl From<deck::DeckState> for DeckState {
    fn from(s: deck::DeckState) -> Self {
        match s {
            deck::DeckState::Empty => DeckState::Empty,
            deck::DeckState::Stopped => DeckState::Stopped,
            deck::DeckState::Playing => DeckState::Playing,
            deck::DeckState::Winding(d) => DeckState::Winding {
                direction: d.into(),
            },
            deck::DeckState::Cueing(d) => DeckState::Cueing {
                direction: d.into(),
            },
        }
    }
}

/// Something the deck did by itself, returned by `tick`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DeckEvent {
    /// Play or cue reached the end of this side and stopped.
    EndOfSide { side: Side },
    /// Fast-forward, rewind or review reached the end and stopped.
    WindFinished { direction: Direction },
}

impl From<deck::DeckEvent> for DeckEvent {
    fn from(e: deck::DeckEvent) -> Self {
        match e {
            deck::DeckEvent::EndOfSide(s) => DeckEvent::EndOfSide { side: s.into() },
            deck::DeckEvent::WindFinished(d) => DeckEvent::WindFinished {
                direction: d.into(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Source {
    Silence,
    Track { track_id: u64, offset_ms: u64 },
}

impl From<tape::Source> for Source {
    fn from(s: tape::Source) -> Self {
        match s {
            tape::Source::Silence => Source::Silence,
            tape::Source::Track { track, offset } => Source::Track {
                track_id: track.0,
                offset_ms: ms(offset),
            },
        }
    }
}

/// A tape on the shelf.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TapeSummary {
    pub id: i64,
    pub title: String,
    pub kind: TapeKind,
    pub cassette: Cassette,
    pub facing: Side,
    pub position_ms: u64,
}

impl From<store::TapeSummary> for TapeSummary {
    fn from(t: store::TapeSummary) -> Self {
        TapeSummary {
            id: t.id.0,
            title: t.title,
            kind: t.kind.into(),
            cassette: t.cassette.into(),
            facing: t.facing.into(),
            position_ms: ms(t.position),
        }
    }
}

/// A tape that an import put on the shelf.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecordedTape {
    pub id: i64,
    pub title: String,
    pub cassette: Cassette,
    pub songs_a: u32,
    pub songs_b: u32,
}

impl From<library::RecordedTape> for RecordedTape {
    fn from(t: library::RecordedTape) -> Self {
        let count = |n: usize| n.try_into().unwrap_or(u32::MAX);
        RecordedTape {
            id: t.id.0,
            title: t.title,
            cassette: t.cassette.into(),
            songs_a: count(t.songs[0]),
            songs_b: count(t.songs[1]),
        }
    }
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct TapeStatus {
    pub id: i64,
    pub cassette: Cassette,
    pub facing: Side,
    /// Position on the side facing out.
    pub position_ms: u64,
    pub side_len_ms: u64,
    /// Tape speed relative to normal play (0 when stopped).
    pub speed: f64,
    /// What is under the head; `None` at the very end of the side.
    pub source: Option<Source>,
    /// Radii of the tape packs on the left and right reels, for drawing.
    pub left_reel_radius: f64,
    pub right_reel_radius: f64,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DeckStatus {
    pub state: DeckState,
    /// The three-digit counter reading.
    pub counter: u16,
    pub tape: Option<TapeStatus>,
}

impl From<deck::DeckStatus> for DeckStatus {
    fn from(s: deck::DeckStatus) -> Self {
        DeckStatus {
            state: s.state.into(),
            counter: s.counter,
            tape: s.tape.map(|t| TapeStatus {
                id: t.id.0,
                cassette: t.cassette.into(),
                facing: t.facing.into(),
                position_ms: ms(t.position),
                side_len_ms: ms(t.side_len),
                speed: t.speed,
                source: t.source.map(Into::into),
                left_reel_radius: t.reel_radii.0,
                right_reel_radius: t.reel_radii.1,
            }),
        }
    }
}

/// The song that should be coming out of the speakers, and from where.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AudibleTrack {
    pub track_id: u64,
    /// The audio file to play.
    pub path: String,
    pub title: Option<String>,
    /// Where in the file the head is.
    pub offset_ms: u64,
}

#[uniffi::export]
pub fn max_drift_ms() -> u64 {
    ms(ontape_core::player::MAX_DRIFT)
}
