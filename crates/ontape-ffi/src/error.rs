use std::fmt;

use ontape_core::deck::DeckError;
use ontape_core::library::LibraryError;
use ontape_core::session::SessionError;
use ontape_core::store::StoreError;

#[derive(Debug, PartialEq, Eq, uniffi::Error)]
pub enum OnTapeError {
    DeckEmpty,
    DeckOccupied,
    NotPlaying,
    TapeNotFound { id: i64 },
    TapeInDeck { id: i64 },
    NoAudioFile { track_id: u64 },
    Storage { reason: String },
    Import { reason: String },
}

impl fmt::Display for OnTapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OnTapeError::DeckEmpty => DeckError::Empty.fmt(f),
            OnTapeError::DeckOccupied => DeckError::Occupied.fmt(f),
            OnTapeError::NotPlaying => DeckError::NotPlaying.fmt(f),
            OnTapeError::TapeNotFound { id } => write!(f, "no tape #{id}"),
            OnTapeError::TapeInDeck { id } => write!(f, "tape #{id} is in the deck"),
            OnTapeError::NoAudioFile { track_id } => write!(f, "track {track_id} has no audio file"),
            OnTapeError::Storage { reason } | OnTapeError::Import { reason } => f.write_str(reason),
        }
    }
}

impl std::error::Error for OnTapeError {}

impl From<DeckError> for OnTapeError {
    fn from(e: DeckError) -> Self {
        match e {
            DeckError::Empty => OnTapeError::DeckEmpty,
            DeckError::Occupied => OnTapeError::DeckOccupied,
            DeckError::NotPlaying => OnTapeError::NotPlaying,
        }
    }
}

impl From<StoreError> for OnTapeError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(id) => OnTapeError::TapeNotFound { id: id.0 },
            e => OnTapeError::Storage {
                reason: e.to_string(),
            },
        }
    }
}

impl From<SessionError> for OnTapeError {
    fn from(e: SessionError) -> Self {
        match e {
            SessionError::Deck(e) => e.into(),
            SessionError::Store(e) => e.into(),
        }
    }
}

impl From<LibraryError> for OnTapeError {
    fn from(e: LibraryError) -> Self {
        match e {
            LibraryError::Store(e) => e.into(),
            e => OnTapeError::Import {
                reason: e.to_string(),
            }
        }
    }
}
