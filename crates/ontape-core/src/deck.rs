use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::clock::Clock;
use crate::tape::{Cassette, Counter, Direction, Side, Source, Tape, TapeId, Wind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeckState {
    Empty,
    Stopped,
    Playing,
    Winding(Direction),
    Cueing(Direction),
}

/// Something the deck did by itself, reported by [`Deck::update`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeckEvent {
    /// Play or cue reached the end of this side and stopped.
    EndOfSide(Side),
    /// Fast-forward, rewind or review reached the end and stopped.
    WindFinished(Direction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckError {
    Empty,
    Occupied,
    NotPlaying,
}

impl fmt::Display for DeckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DeckError::Empty => "no tape in the deck",
            DeckError::Occupied => "a tape is already in the deck",
            DeckError::NotPlaying => "cue and review only work while playing",
        })
    }
}

impl std::error::Error for DeckError {}

#[derive(Clone, Debug, PartialEq)]
pub struct DeckStatus {
    pub state: DeckState,
    pub counter: u16,
    pub tape: Option<TapeStatus>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TapeStatus {
    pub id: TapeId,
    pub cassette: Cassette,
    pub facing: Side,
    pub position: Duration,
    pub side_len: Duration,
    pub speed: f64,
    pub source: Option<Source>,
    pub reel_radii: (f64, f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Transport {
    Stopped,
    Playing { from: Duration, started: Instant },
    Winding(Wind),
    Cueing(Wind),
}

#[derive(Clone, Debug)]
struct Loaded {
    id: TapeId,
    tape: Tape,
    transport: Transport,
}

pub struct Deck {
    clock: Arc<dyn Clock>,
    loaded: Option<Loaded>,
    counter: Counter,
    /// What the counter shows while the deck is empty.
    held_counter: f64,
    events: Vec<DeckEvent>,
}

impl Deck {
    pub fn new(clock: Arc<dyn Clock>) -> Deck {
        Deck {
            clock,
            loaded: None,
            counter: Counter::new(),
            held_counter: 0.0,
            events: Vec::new(),
        }
    }

    pub fn restore(
        clock: Arc<dyn Clock>,
        tape: Option<(TapeId, Tape)>,
        counter_gear: f64,
        counter_value: f64,
    ) -> Deck {
        let mut deck = Deck {
            clock,
            loaded: None,
            counter: Counter::with_gear(counter_gear),
            held_counter: counter_value,
            events: Vec::new(),
        };
        if let Some((id, tape)) = tape {
            deck.insert(id, tape).expect("a new deck is empty");
        }
        deck
    }

    // Buttons

    pub fn insert(&mut self, id: TapeId, tape: Tape) -> Result<(), DeckError> {
        if self.loaded.is_some() {
            return Err(DeckError::Occupied);
        }
        self.counter
            .set_value(&tape.reels(), tape.position(), self.held_counter);
        self.loaded = Some(Loaded {
            id,
            tape,
            transport: Transport::Stopped,
        });
        Ok(())
    }

    pub fn eject(&mut self) -> Result<(TapeId, Tape), DeckError> {
        self.stop()?;
        self.held_counter = self.counter_value();
        let loaded = self.loaded.take().ok_or(DeckError::Empty)?;
        Ok((loaded.id, loaded.tape))
    }

    pub fn play(&mut self) -> Result<(), DeckError> {
        let now = self.settle()?;
        let loaded = self.loaded.as_mut().expect("settle checked");
        if !matches!(loaded.transport, Transport::Playing { .. }) {
            loaded.transport = Transport::Playing {
                from: loaded.tape.position(),
                started: now,
            };
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), DeckError> {
        self.settle()?;
        self.loaded.as_mut().expect("settle checked").transport = Transport::Stopped;
        Ok(())
    }

    /// Fast-forward or rewind. Pressing the same direction again stops; the
    /// other direction reverses.
    pub fn wind(&mut self, dir: Direction) -> Result<(), DeckError> {
        let now = self.settle()?;
        let loaded = self.loaded.as_mut().expect("settle checked");
        loaded.transport = match loaded.transport {
            Transport::Winding(w) if w.dir() == dir => Transport::Stopped,
            _ => Transport::Winding(Wind::start(
                loaded.tape.reels(),
                loaded.tape.position(),
                dir,
                now,
            )),
        };
        Ok(())
    }

    pub fn cue(&mut self, dir: Direction) -> Result<(), DeckError> {
        let now = self.settle()?;
        let loaded = self.loaded.as_mut().expect("settle checked");
        match loaded.transport {
            Transport::Playing { .. } | Transport::Cueing(_) => {
                let reels = loaded.tape.reels().cueing();
                loaded.transport =
                    Transport::Cueing(Wind::start(reels, loaded.tape.position(), dir, now));
                Ok(())
            }
            Transport::Stopped | Transport::Winding(_) => Err(DeckError::NotPlaying),
        }
    }

    pub fn release_cue(&mut self) -> Result<(), DeckError> {
        let now = self.settle()?;
        let loaded = self.loaded.as_mut().expect("settle checked");
        if let Transport::Cueing(_) = loaded.transport {
            loaded.transport = Transport::Playing {
                from: loaded.tape.position(),
                started: now,
            };
        }
        Ok(())
    }

    pub fn flip(&mut self) -> Result<(), DeckError> {
        self.stop()?;
        let value = self.counter_value();
        let loaded = self.loaded.as_mut().expect("stop checked");
        loaded.tape.flip();

        self.counter
            .set_value(&loaded.tape.reels(), loaded.tape.position(), value);
        Ok(())
    }

    pub fn reset_counter(&mut self) {
        let _ = self.settle();
        match &self.loaded {
            Some(l) => self.counter.reset(&l.tape.reels(), l.tape.position()),
            None => self.held_counter = 0.0,
        }
    }

    pub fn update(&mut self) -> Vec<DeckEvent> {
        let _ = self.settle();
        std::mem::take(&mut self.events)
    }

    fn settle(&mut self) -> Result<Instant, DeckError> {
        let now = self.clock.now();
        let loaded = self.loaded.as_mut().ok_or(DeckError::Empty)?;
        let side_len = loaded.tape.side_len();
        match loaded.transport {
            Transport::Stopped => {}
            Transport::Playing { from, started } => {
                let pos = (from + now.saturating_duration_since(started)).min(side_len);
                loaded.tape.set_position(pos);
                if pos == side_len {
                    loaded.transport = Transport::Stopped;

                    self.events.push(DeckEvent::EndOfSide(loaded.tape.facing()));
                } else {
                    loaded.transport = Transport::Playing {
                        from: pos,
                        started: now,
                    };
                }
            }
            Transport::Winding(wind) | Transport::Cueing(wind) => {
                loaded.tape.set_position(wind.position(now));
                if wind.is_finished(now) {
                    let cueing = matches!(loaded.transport, Transport::Cueing(_));
                    loaded.transport = Transport::Stopped;
                    self.events.push(match (cueing, wind.dir()) {
                        (true, Direction::Forward) => DeckEvent::EndOfSide(loaded.tape.facing()),
                        _ => DeckEvent::WindFinished(wind.dir()),
                    });
                }
            }
        }
        Ok(now)
    }

    pub fn state(&self) -> DeckState {
        match &self.loaded {
            None => DeckState::Empty,
            Some(l) => match l.transport {
                Transport::Stopped => DeckState::Stopped,
                Transport::Playing { .. } => DeckState::Playing,
                Transport::Winding(w) => DeckState::Winding(w.dir()),
                Transport::Cueing(w) => DeckState::Cueing(w.dir()),
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.loaded.is_none()
    }

    pub fn is_moving(&self) -> bool {
        matches!(
            self.state(),
            DeckState::Playing | DeckState::Winding(_) | DeckState::Cueing(_)
        )
    }

    pub fn tape(&self) -> Option<(TapeId, &Tape)> {
        self.loaded.as_ref().map(|l| (l.id, &l.tape))
    }

    pub fn position(&self) -> Option<Duration> {
        let now = self.clock.now();
        self.loaded.as_ref().map(|l| live_position(l, now))
    }

    pub fn speed(&self) -> f64 {
        let now = self.clock.now();
        match self.loaded.as_ref().map(|l| l.transport) {
            Some(Transport::Playing { from, started }) => {
                let side_len = self
                    .loaded
                    .as_ref()
                    .map_or(Duration::ZERO, |l| l.tape.side_len());
                if from + now.saturating_duration_since(started) >= side_len {
                    0.0
                } else {
                    1.0
                }
            }
            Some(Transport::Winding(w) | Transport::Cueing(w)) => w.speed(now),
            Some(Transport::Stopped) | None => 0.0,
        }
    }

    pub fn counter_gear(&self) -> f64 {
        self.counter.gear()
    }

    pub fn counter_value(&self) -> f64 {
        match &self.loaded {
            None => self.held_counter,
            Some(l) => self
                .counter
                .value(&l.tape.reels(), live_position(l, self.clock.now())),
        }
    }

    pub fn counter_reading(&self) -> u16 {
        (self.counter_value().floor() as i64).rem_euclid(1000) as u16
    }

    pub fn status(&self) -> DeckStatus {
        let now = self.clock.now();
        let tape = self.loaded.as_ref().map(|l| {
            let position = live_position(l, now);
            TapeStatus {
                id: l.id,
                cassette: l.tape.cassette(),
                facing: l.tape.facing(),
                position,
                side_len: l.tape.side_len(),
                speed: self.speed(),
                source: l.tape.recording(l.tape.facing()).source_at(position),
                reel_radii: l.tape.reels().reel_radii(position),
            }
        });
        DeckStatus {
            state: self.state(),
            counter: self.counter_reading(),
            tape,
        }
    }
}

impl fmt::Debug for Deck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Deck")
            .field("loaded", &self.loaded)
            .field("counter", &self.counter)
            .field("held_counter", &self.held_counter)
            .finish_non_exhaustive()
    }
}

fn live_position(loaded: &Loaded, now: Instant) -> Duration {
    match loaded.transport {
        Transport::Stopped => loaded.tape.position(),
        Transport::Playing { from, started } => {
            (from + now.saturating_duration_since(started)).min(loaded.tape.side_len())
        }
        Transport::Winding(w) | Transport::Cueing(w) => w.position(now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::tape::{LayoutOptions, TrackId, TrackInfo, layout_album};

    fn deck() -> (Arc<ManualClock>, Deck) {
        let clock = Arc::new(ManualClock::new());
        let deck = Deck::new(clock.clone());
        (clock, deck)
    }

    fn loaded(cassette: Cassette) -> (Arc<ManualClock>, Deck) {
        let (clock, mut deck) = deck();
        deck.insert(TapeId(1), Tape::blank(cassette)).unwrap();
        (clock, deck)
    }

    const fn mins(m: u64) -> Duration {
        Duration::from_secs(m * 60)
    }

    #[test]
    fn empty_deck_rejects_buttons() {
        let (_, mut deck) = deck();
        assert_eq!(deck.state(), DeckState::Empty);
        assert_eq!(deck.play(), Err(DeckError::Empty));
        assert_eq!(deck.eject().unwrap_err(), DeckError::Empty);
    }

    #[test]
    fn insert_twice_is_occupied() {
        let (_, mut deck) = loaded(Cassette::C60);
        assert_eq!(
            deck.insert(TapeId(2), Tape::blank(Cassette::C60)),
            Err(DeckError::Occupied)
        );
    }

    #[test]
    fn playing_moves_at_normal_speed_and_stop_resumes() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.play().unwrap();
        clock.advance(Duration::from_secs(5));
        assert_eq!(deck.position(), Some(Duration::from_secs(5)));
        deck.stop().unwrap();
        clock.advance(Duration::from_secs(5));
        assert_eq!(deck.position(), Some(Duration::from_secs(5)));
        deck.play().unwrap();
        clock.advance(Duration::from_secs(1));
        assert_eq!(deck.position(), Some(Duration::from_secs(6)));
    }

    #[test]
    fn many_small_updates_land_exactly() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.play().unwrap();
        for _ in 0..6000 {
            clock.advance(Duration::from_millis(100));
            assert!(deck.update().is_empty());
        }
        assert_eq!(deck.tape().unwrap().1.position(), mins(10));
    }

    #[test]
    fn play_stops_at_end_of_side() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.play().unwrap();
        clock.advance(mins(31));
        assert_eq!(deck.update(), vec![DeckEvent::EndOfSide(Side::A)]);
        assert_eq!(deck.state(), DeckState::Stopped);
        assert_eq!(deck.position(), Some(mins(30)));
        assert!(deck.update().is_empty());
    }

    #[test]
    fn fast_forward_c60_takes_about_a_minute() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.wind(Direction::Forward).unwrap();
        clock.advance(Duration::from_secs(59));
        assert!(deck.update().is_empty());
        clock.advance(Duration::from_secs(2));
        assert_eq!(
            deck.update(),
            vec![DeckEvent::WindFinished(Direction::Forward)]
        );
        assert_eq!(deck.position(), Some(mins(30)));
    }

    #[test]
    fn rewind_returns_to_start() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.play().unwrap();
        clock.advance(mins(10));
        deck.wind(Direction::Backward).unwrap();
        clock.advance(mins(2));
        assert_eq!(
            deck.update(),
            vec![DeckEvent::WindFinished(Direction::Backward)]
        );
        assert_eq!(deck.position(), Some(Duration::ZERO));
    }

    #[test]
    fn wind_same_direction_stops_other_reverses() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.wind(Direction::Forward).unwrap();
        clock.advance(Duration::from_secs(10));
        deck.wind(Direction::Backward).unwrap();
        assert_eq!(deck.state(), DeckState::Winding(Direction::Backward));
        deck.wind(Direction::Backward).unwrap();
        assert_eq!(deck.state(), DeckState::Stopped);
    }

    #[test]
    fn cue_needs_play_and_release_returns_to_play() {
        let (clock, mut deck) = loaded(Cassette::C60);
        assert_eq!(deck.cue(Direction::Forward), Err(DeckError::NotPlaying));
        deck.play().unwrap();
        clock.advance(mins(1));
        deck.cue(Direction::Forward).unwrap();
        clock.advance(Duration::from_secs(5));
        let cued = deck.position().unwrap();
        assert!(cued > mins(1) + Duration::from_secs(5));
        deck.release_cue().unwrap();
        assert_eq!(deck.state(), DeckState::Playing);
        clock.advance(Duration::from_secs(1));
        assert_eq!(deck.position(), Some(cued + Duration::from_secs(1)));

        deck.cue(Direction::Backward).unwrap();
        clock.advance(Duration::from_secs(1));
        assert!(deck.position().unwrap() < cued);
    }

    #[test]
    fn flip_keeps_counter_and_mirrors_position() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.play().unwrap();
        clock.advance(mins(10));
        let before = deck.counter_value();
        deck.flip().unwrap();
        assert_eq!(deck.state(), DeckState::Stopped);
        assert_eq!(deck.tape().unwrap().1.facing(), Side::B);
        assert_eq!(deck.position(), Some(mins(20)));
        assert!((deck.counter_value() - before).abs() < 1e-9);
    }

    #[test]
    fn eject_and_insert_carry_the_counter() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.play().unwrap();
        clock.advance(mins(5));
        let reading = deck.counter_reading();
        assert!(reading > 0);
        let (id, tape) = deck.eject().unwrap();
        assert_eq!(id, TapeId(1));
        assert_eq!(deck.counter_reading(), reading);
        deck.insert(TapeId(2), Tape::blank(Cassette::C90)).unwrap();
        assert_eq!(deck.counter_reading(), reading);
        assert_eq!(tape.position(), mins(5));
    }

    #[test]
    fn reset_counter_reads_zero() {
        let (clock, mut deck) = loaded(Cassette::C60);
        deck.play().unwrap();
        clock.advance(mins(5));
        deck.reset_counter();
        assert_eq!(deck.counter_reading(), 0);
    }

    #[test]
    fn restore_puts_tape_back_with_counter() {
        let clock = Arc::new(ManualClock::new());
        let mut tape = Tape::blank(Cassette::C60);
        tape.set_position(mins(3));
        let deck = Deck::restore(clock, Some((TapeId(7), tape)), 0.5, 42.5);
        assert_eq!(deck.state(), DeckState::Stopped);
        assert_eq!(deck.position(), Some(mins(3)));
        assert_eq!(deck.counter_reading(), 42);
    }

    #[test]
    fn status_shows_song_offset_after_leader() {
        let tracks = [TrackInfo {
            id: TrackId(1),
            len: mins(3),
        }];
        let tape = layout_album(&tracks, &LayoutOptions::default())
            .unwrap()
            .remove(0);
        let (clock, mut deck) = deck();
        deck.insert(TapeId(1), tape).unwrap();
        deck.play().unwrap();
        clock.advance(Duration::from_secs(14));
        let status = deck.status();
        assert_eq!(status.state, DeckState::Playing);
        let t = status.tape.unwrap();
        assert_eq!(t.speed, 1.0);
        assert_eq!(
            t.source,
            Some(Source::Track {
                track: TrackId(1),
                offset: Duration::from_secs(10)
            })
        );
    }
}
