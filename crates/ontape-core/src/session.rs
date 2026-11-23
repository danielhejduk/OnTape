use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::clock::Clock;
use crate::deck::{Deck, DeckError, DeckEvent};
use crate::store::{Store, StoreError};
use crate::tape::{Direction, TapeId};

pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum SessionError {
    Deck(DeckError),
    Store(StoreError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Deck(e) => e.fmt(f),
            SessionError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Deck(e) => Some(e),
            SessionError::Store(e) => Some(e),
        }
    }
}

impl From<DeckError> for SessionError {
    fn from(e: DeckError) -> Self {
        SessionError::Deck(e)
    }
}

impl From<StoreError> for SessionError {
    fn from(e: StoreError) -> Self {
        SessionError::Store(e)
    }
}

pub type Result<T, E = SessionError> = std::result::Result<T, E>;

pub struct DeckSession {
    clock: Arc<dyn Clock>,
    store: Store,
    slot: u8,
    deck: Deck,
    last_save: Instant,
}

impl DeckSession {
    pub fn open(store: Store, slot: u8, clock: Arc<dyn Clock>) -> Result<DeckSession> {
        let deck = store.load_deck(slot, clock.clone())?;
        let last_save = clock.now();
        Ok(DeckSession {
            clock,
            store,
            slot,
            deck,
            last_save,
        })
    }

    pub fn deck(&self) -> &Deck {
        &self.deck
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    // Buttons
    pub fn insert(&mut self, id: TapeId) -> Result<()> {
        if !self.deck.is_empty() {
            return Err(DeckError::Occupied.into());
        }

        let tape = self.store.load_tape(id)?;
        self.deck.insert(id, tape)?;
        self.save()
    }

    pub fn eject(&mut self) -> Result<TapeId> {
        let (id, tape) = self.deck.eject()?;
        self.store.save_position(id, &tape)?;
        self.save()?;
        Ok(id)
    }

    pub fn play(&mut self) -> Result<()> {
        self.deck.play()?;
        self.save()
    }

    pub fn stop(&mut self) -> Result<()> {
        self.deck.stop()?;
        self.save()
    }

    pub fn wind(&mut self, dir: Direction) -> Result<()> {
        self.deck.wind(dir)?;
        self.save()
    }

    pub fn cue(&mut self, dir: Direction) -> Result<()> {
        self.deck.cue(dir)?;
        self.save()
    }

    pub fn release_cue(&mut self) -> Result<()> {
        self.deck.release_cue()?;
        self.save()
    }

    pub fn flip(&mut self) -> Result<()> {
        self.deck.flip()?;
        self.save()
    }

    pub fn reset_counter(&mut self) -> Result<()> {
        self.deck.reset_counter();
        self.save()
    }

    // Time
    pub fn tick(&mut self) -> Result<Vec<DeckEvent>> {
        let events = self.deck.update();
        let due = self.clock.now().saturating_duration_since(self.last_save) >= AUTOSAVE_INTERVAL;
        if !events.is_empty() || (self.deck.is_moving() && due) {
            self.save()?;
        }
        Ok(events)
    }

    pub fn save(&mut self) -> Result<()> {
        self.store.save_deck(self.slot, &self.deck)?;
        self.last_save = self.clock.now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::deck::DeckState;
    use crate::tape::{Cassette, Tape};

    fn session() -> (Arc<ManualClock>, DeckSession, TapeId) {
        let clock = Arc::new(ManualClock::new());
        let mut store = Store::open_in_memory().unwrap();
        let id = store
            .add_tape("Blank", &Tape::blank(Cassette::C60))
            .unwrap();
        let session = DeckSession::open(store, 0, clock.clone()).unwrap();
        (clock, session, id)
    }

    /// Simulates a crash: reopens the deck from what was last written.
    fn reopen(s: DeckSession, clock: &Arc<ManualClock>) -> DeckSession {
        let DeckSession { store, slot, .. } = s;
        DeckSession::open(store, slot, clock.clone()).unwrap()
    }

    #[test]
    fn insert_saves_the_deck() {
        let (clock, mut s, id) = session();
        s.insert(id).unwrap();
        let s = reopen(s, &clock);
        assert_eq!(s.deck().tape().map(|(i, _)| i), Some(id));
    }

    #[test]
    fn insert_twice_is_occupied() {
        let (_, mut s, id) = session();
        s.insert(id).unwrap();
        assert!(matches!(
            s.insert(id),
            Err(SessionError::Deck(DeckError::Occupied))
        ));
    }

    #[test]
    fn missing_tape_is_a_store_error() {
        let (_, mut s, _) = session();
        assert!(matches!(
            s.insert(TapeId(99)),
            Err(SessionError::Store(StoreError::NotFound(_)))
        ));
    }

    #[test]
    fn eject_saves_tape_position() {
        let (clock, mut s, id) = session();
        s.insert(id).unwrap();
        s.play().unwrap();
        clock.advance(Duration::from_secs(30));
        assert_eq!(s.eject().unwrap(), id);
        assert_eq!(
            s.store().load_tape(id).unwrap().position(),
            Duration::from_secs(30)
        );
        assert!(reopen(s, &clock).deck().is_empty());
    }

    #[test]
    fn tick_autosaves_while_moving() {
        let (clock, mut s, id) = session();
        s.insert(id).unwrap();
        s.play().unwrap();
        for _ in 0..105 {
            clock.advance(Duration::from_millis(100));
            s.tick().unwrap();
        }
        // Crash at 10.5 s: the last autosave was at 10 s.
        let s = reopen(s, &clock);
        assert_eq!(s.deck().state(), DeckState::Stopped);
        assert_eq!(s.deck().position(), Some(Duration::from_secs(10)));
    }

    #[test]
    fn tick_saves_on_events() {
        let (clock, mut s, id) = session();
        s.insert(id).unwrap();
        s.wind(Direction::Forward).unwrap();
        clock.advance(Duration::from_secs(61));
        assert_eq!(
            s.tick().unwrap(),
            vec![DeckEvent::WindFinished(Direction::Forward)]
        );
        let s = reopen(s, &clock);
        assert_eq!(s.deck().position(), Some(Cassette::C60.side_len()));
    }

    #[test]
    fn stopped_deck_does_not_autosave() {
        let (clock, mut s, id) = session();
        s.insert(id).unwrap();
        let saved = s.last_save;
        clock.advance(Duration::from_secs(10));
        s.tick().unwrap();
        assert_eq!(s.last_save, saved);
    }
}
