//! Audio output that follows a deck: whatever track is under the head while
//! the deck plays comes out of the speakers, at the same offset.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use rodio::{DeviceSinkBuilder, MixerDeviceSink};

use crate::audio::{AudioError, FileSource};
use crate::deck::{DeckState, DeckStatus};
use crate::session::DeckSession;
use crate::store::StoreError;
use crate::tape::{Source, TrackId};

/// How far the audio may drift from the tape before it is re-seeked.
const MAX_DRIFT: Duration = Duration::from_millis(400);

#[derive(Debug)]
pub enum PlaybackError {
    Device(rodio::DeviceSinkError),
    NoFile(TrackId),
    Audio(AudioError),
    Store(StoreError),
}

impl fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlaybackError::Device(e) => write!(f, "no audio output: {e}"),
            PlaybackError::NoFile(id) => write!(f, "track {} has no audio file", id.0),
            PlaybackError::Audio(e) => e.fmt(f),
            PlaybackError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for PlaybackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PlaybackError::Device(e) => Some(e),
            PlaybackError::NoFile(_) => None,
            PlaybackError::Audio(e) => Some(e),
            PlaybackError::Store(e) => Some(e),
        }
    }
}

impl From<StoreError> for PlaybackError {
    fn from(e: StoreError) -> Self {
        PlaybackError::Store(e)
    }
}

impl From<AudioError> for PlaybackError {
    fn from(e: AudioError) -> Self {
        PlaybackError::Audio(e)
    }
}

/// Length of an audio file, read from its headers.
pub fn probe_len(path: &Path) -> Result<Duration, AudioError> {
    FileSource::open(path)?.len()
}

/// The track and offset that should be audible for this deck status, if any.
pub fn audible(status: &DeckStatus) -> Option<(TrackId, Duration)> {
    if status.state != DeckState::Playing {
        return None;
    }
    match status.tape.as_ref()?.source? {
        Source::Track { track, offset } => Some((track, offset)),
        Source::Silence => None,
    }
}

pub struct Player {
    // Dropping the sink closes the audio device, so it lives as long as the player.
    _sink: MixerDeviceSink,
    player: rodio::Player,
    rate: f64,
    /// The track loaded into `player` (also set after a failed load, so it is not retried every tick).
    current: Option<TrackId>,
}

impl Player {
    /// Opens the default audio device. `rate` is how fast the deck's clock runs
    /// relative to real time; the audio is sped up to match.
    pub fn open(rate: f64) -> Result<Player, PlaybackError> {
        let mut sink = DeviceSinkBuilder::open_default_sink().map_err(PlaybackError::Device)?;
        sink.log_on_drop(false);
        let player = rodio::Player::connect_new(sink.mixer());
        player.set_speed(rate as f32);
        Ok(Player { _sink: sink, player, rate, current: None })
    }

    /// Brings the audio in line with the deck. Call it often (every ~100 ms).
    pub fn sync(&mut self, session: &DeckSession) -> Result<(), PlaybackError> {
        let Some((track, offset)) = audible(&session.deck().status()) else {
            if self.current.take().is_some() {
                self.player.clear();
            }
            return Ok(());
        };

        if self.current == Some(track) {
            // Finished early (file shorter than its slot on tape) or failed to load: stay quiet.
            if self.player.empty() {
                return Ok(());
            }
            let pos = self.player.get_pos();
            let drift = pos.abs_diff(offset);
            if drift <= MAX_DRIFT.mul_f64(self.rate.max(1.0)) || self.player.try_seek(offset).is_ok() {
                return Ok(());
            }
        }

        self.player.clear();
        self.current = Some(track);
        let path = session.store().track_path(track)?.ok_or(PlaybackError::NoFile(track))?;
        self.player.append(FileSource::open(&path)?);
        if !offset.is_zero() {
            // Not every format can seek; then the song just plays from the top.
            let _ = self.player.try_seek(offset);
        }
        self.player.play();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::clock::ManualClock;
    use crate::deck::Deck;
    use crate::tape::{LayoutOptions, TapeId, TrackInfo, layout_album};

    fn deck() -> (Arc<ManualClock>, Deck) {
        let clock = Arc::new(ManualClock::new());
        let tracks = [TrackInfo { id: TrackId(1), len: Duration::from_secs(60) }];
        let tape = layout_album(&tracks, &LayoutOptions::default()).unwrap().remove(0);
        let mut deck = Deck::new(clock.clone());
        deck.insert(TapeId(1), tape).unwrap();
        (clock, deck)
    }

    #[test]
    fn silent_unless_playing_a_track() {
        let (clock, mut deck) = deck();
        assert_eq!(audible(&deck.status()), None);

        deck.play().unwrap();
        // Past whatever leader the layout put before the first song.
        clock.advance(Duration::from_secs(20));
        let (track, offset) = audible(&deck.status()).expect("a song is under the head");
        assert_eq!(track, TrackId(1));
        assert!(!offset.is_zero() && offset <= Duration::from_secs(20));

        deck.stop().unwrap();
        assert_eq!(audible(&deck.status()), None);
    }
}
