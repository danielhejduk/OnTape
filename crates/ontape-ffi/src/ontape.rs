use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use ontape_core::clock::{Clock, SystemClock};
use ontape_core::library;
use ontape_core::player;
use ontape_core::session::DeckSession;
use ontape_core::store::Store;
use ontape_core::tape::{Direction, TapeId, TrackId};

use crate::error::OnTapeError;
use crate::types::{AudibleTrack, DeckEvent, DeckStatus, ImportTrack, RecordedTape, TapeSummary, ms};

type Result<T, E = OnTapeError> = std::result::Result<T, E>;

/// One tape deck and the shelf of tapes it plays from.
///
/// Every method is safe to call from any thread. Importing is slow; call it
/// off the main thread.
#[derive(uniffi::Object)]
pub struct OnTape {
    session: Mutex<DeckSession>,
}

impl OnTape {
    fn with_clock(store: Store, clock: Arc<dyn Clock>) -> Result<OnTape> {
        Ok(OnTape {
            session: Mutex::new(DeckSession::open(store, 0, clock)?),
        })
    }

    fn lock(&self) -> MutexGuard<'_, DeckSession> {
        self.session.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn button(
        &self,
        press: impl FnOnce(&mut DeckSession) -> Result<(), ontape_core::session::SessionError>,
    ) -> Result<()> {
        Ok(press(&mut self.lock())?)
    }
}

#[uniffi::export]
impl OnTape {
    /// Opens (or creates) the library database at `db_path`, with the deck as
    /// it was left.
    #[uniffi::constructor]
    pub fn open(db_path: String) -> Result<Arc<OnTape>> {
        let store = Store::open_creating_dir(&db_path)?;
        Ok(Arc::new(OnTape::with_clock(store, Arc::new(SystemClock))?))
    }

    // Shelf

    pub fn list_tapes(&self) -> Result<Vec<TapeSummary>> {
        let session = self.lock();
        let tapes = session.store().list_tapes()?;
        Ok(tapes.into_iter().map(Into::into).collect())
    }

    pub fn tape_title(&self, id: i64) -> Result<String> {
        Ok(self.lock().store().tape_title(TapeId(id))?)
    }

    /// Throws a tape away. The tape in the deck has to be ejected first.
    pub fn delete_tape(&self, id: i64) -> Result<()> {
        let session = self.lock();
        if session
            .deck()
            .tape()
            .is_some_and(|(inserted, _)| inserted.0 == id)
        {
            return Err(OnTapeError::TapeInDeck { id });
        }
        Ok(session.store().delete_tape(TapeId(id))?)
    }

    /// Records an audio file, or a folder of them, onto one or more album
    /// tapes. `title` defaults to the folder (or file) name. Slow: reads every
    /// file first.
    pub fn import_album(&self, path: String, title: Option<String>) -> Result<Vec<RecordedTape>> {
        // Scanning reads every file, so the deck stays usable meanwhile.
        let album = library::scan(Path::new(&path), title)?;
        let recorded = library::record(self.lock().store_mut(), &album)?;
        Ok(recorded.into_iter().map(Into::into).collect())
    }

    /// Records these audio files, in this order, onto one or more album tapes
    /// titled `title`. A track without a title is named after its file. Slow:
    /// reads every file first, and records nothing if any of them is bad.
    pub fn import_tracks(&self, title: String, tracks: Vec<ImportTrack>) -> Result<Vec<RecordedTape>> {
        let mut files = Vec::with_capacity(tracks.len());
        for track in tracks {
            let path = PathBuf::from(track.path);
            let len = player::probe_len(&path).map_err(library::LibraryError::from)?;
            let title = track.title.unwrap_or_else(|| {
                path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            });
            files.push(library::ScannedFile { path, title, len });
        }
        let album = library::ScannedAlbum { title, files };
        let recorded = library::record(self.lock().store_mut(), &album)?;
        Ok(recorded.into_iter().map(Into::into).collect())
    }

    // Buttons

    pub fn insert(&self, id: i64) -> Result<()> {
        self.button(|s| s.insert(TapeId(id)))
    }

    /// Takes the tape out and returns its id.
    pub fn eject(&self) -> Result<i64> {
        Ok(self.lock().eject()?.0)
    }

    pub fn play(&self) -> Result<()> {
        self.button(DeckSession::play)
    }

    pub fn stop(&self) -> Result<()> {
        self.button(DeckSession::stop)
    }

    /// Pressing it again while winding forward stops the tape.
    pub fn fast_forward(&self) -> Result<()> {
        self.button(|s| s.wind(Direction::Forward))
    }

    /// Pressing it again while rewinding stops the tape.
    pub fn rewind(&self) -> Result<()> {
        self.button(|s| s.wind(Direction::Backward))
    }

    /// Winds forward fast while playing; `release_cue` goes back to play.
    pub fn cue(&self) -> Result<()> {
        self.button(|s| s.cue(Direction::Forward))
    }

    /// Winds backward fast while playing; `release_cue` goes back to play.
    pub fn review(&self) -> Result<()> {
        self.button(|s| s.cue(Direction::Backward))
    }

    pub fn release_cue(&self) -> Result<()> {
        self.button(DeckSession::release_cue)
    }

    pub fn flip(&self) -> Result<()> {
        self.button(DeckSession::flip)
    }

    pub fn reset_counter(&self) -> Result<()> {
        self.button(DeckSession::reset_counter)
    }

    // Time

    /// Moves the deck on to now and saves it now and then. Call it about every
    /// 100 ms while the deck is moving. Returns what the deck did by itself.
    pub fn tick(&self) -> Result<Vec<DeckEvent>> {
        let events = self.lock().tick()?;
        Ok(events.into_iter().map(Into::into).collect())
    }

    pub fn status(&self) -> DeckStatus {
        self.lock().deck().status().into()
    }

    /// The song that should be heard right now, or `None` for silence (not
    /// playing, or between songs). Play `path` from `offset_ms`, and seek when
    /// the player drifts more than `max_drift_ms()` from it.
    pub fn audible(&self) -> Result<Option<AudibleTrack>> {
        let session = self.lock();
        let Some((track, offset)) = player::audible(&session.deck().status()) else {
            return Ok(None);
        };
        let path = session
            .store()
            .track_path(track)?
            .ok_or(OnTapeError::NoAudioFile { track_id: track.0 })?;
        Ok(Some(AudibleTrack {
            track_id: track.0,
            path: path_string(path),
            title: session.store().track_title(track)?,
            offset_ms: ms(offset),
        }))
    }

    pub fn track_title(&self, track_id: u64) -> Result<Option<String>> {
        Ok(self.lock().store().track_title(TrackId(track_id))?)
    }

    pub fn track_path(&self, track_id: u64) -> Result<Option<String>> {
        let path = self.lock().store().track_path(TrackId(track_id))?;
        Ok(path.map(path_string))
    }
}

fn path_string(path: PathBuf) -> String {
    path.into_os_string()
        .into_string()
        .unwrap_or_else(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ontape_core::clock::ManualClock;

    use super::*;
    use crate::types::{DeckState, Side, Source};

    fn ontape() -> (Arc<ManualClock>, OnTape, i64) {
        let clock = Arc::new(ManualClock::new());
        let mut store = Store::open_in_memory().unwrap();
        let recorded =
            library::record_lengths(&mut store, "Album", &[Duration::from_secs(3 * 60)]).unwrap();
        let ontape = OnTape::with_clock(store, clock.clone()).unwrap();
        (clock, ontape, recorded[0].id.0)
    }

    #[test]
    fn open_creates_the_database() {
        let dir = std::env::temp_dir().join(format!("ontape-ffi-test-{}", std::process::id()));
        let db = dir.join("nested").join("ontape.db");
        let ontape = OnTape::open(db.to_string_lossy().into_owned()).unwrap();
        assert!(ontape.list_tapes().unwrap().is_empty());
        assert_eq!(ontape.status().state, DeckState::Empty);
        drop(ontape);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lists_tapes() {
        let (_, ontape, id) = ontape();
        let tapes = ontape.list_tapes().unwrap();
        assert_eq!(tapes.len(), 1);
        assert_eq!(tapes[0].id, id);
        assert_eq!(tapes[0].title, "Album");
        assert_eq!(tapes[0].facing, Side::A);
        assert_eq!(tapes[0].position_ms, 0);
        assert_eq!(ontape.tape_title(id).unwrap(), "Album");
    }

    #[test]
    fn plays_a_track_after_the_leader() {
        let (clock, ontape, id) = ontape();
        ontape.insert(id).unwrap();
        ontape.play().unwrap();
        clock.advance(Duration::from_secs(20));
        assert!(ontape.tick().unwrap().is_empty());

        let status = ontape.status();
        assert_eq!(status.state, DeckState::Playing);
        let tape = status.tape.unwrap();
        assert_eq!(tape.id, id);
        assert_eq!(tape.position_ms, 20_000);
        assert!(matches!(
            tape.source,
            Some(Source::Track { offset_ms, .. }) if offset_ms > 0 && offset_ms <= 20_000
        ));
    }

    #[test]
    fn tick_reports_end_of_side() {
        let (clock, ontape, id) = ontape();
        ontape.insert(id).unwrap();
        ontape.play().unwrap();
        let side_len = ontape.status().tape.unwrap().side_len_ms;
        clock.advance(Duration::from_millis(side_len + 1000));
        assert_eq!(
            ontape.tick().unwrap(),
            vec![DeckEvent::EndOfSide { side: Side::A }]
        );
        assert_eq!(ontape.status().state, DeckState::Stopped);
    }

    #[test]
    fn deck_errors_map_to_variants() {
        let (_, ontape, id) = ontape();
        assert_eq!(ontape.play(), Err(OnTapeError::DeckEmpty));
        assert_eq!(ontape.eject(), Err(OnTapeError::DeckEmpty));
        assert_eq!(ontape.insert(99), Err(OnTapeError::TapeNotFound { id: 99 }));
        ontape.insert(id).unwrap();
        assert_eq!(ontape.insert(id), Err(OnTapeError::DeckOccupied));
        assert_eq!(ontape.cue(), Err(OnTapeError::NotPlaying));
        assert_eq!(ontape.delete_tape(id), Err(OnTapeError::TapeInDeck { id }));
        assert_eq!(ontape.eject(), Ok(id));
        ontape.delete_tape(id).unwrap();
        assert!(ontape.list_tapes().unwrap().is_empty());
    }

    #[test]
    fn audible_follows_the_deck() {
        let (clock, ontape, id) = ontape();
        assert_eq!(ontape.audible(), Ok(None));
        ontape.insert(id).unwrap();
        ontape.play().unwrap();
        clock.advance(Duration::from_secs(20));
        // These tracks were made up, so there is no file to play.
        assert!(matches!(
            ontape.audible(),
            Err(OnTapeError::NoAudioFile { .. })
        ));
        ontape.stop().unwrap();
        assert_eq!(ontape.audible(), Ok(None));
    }

    #[test]
    fn import_reports_a_missing_folder() {
        let (_, ontape, _) = ontape();
        assert!(matches!(
            ontape.import_album("/no/such/album".into(), None),
            Err(OnTapeError::Import { .. })
        ));
    }
}

