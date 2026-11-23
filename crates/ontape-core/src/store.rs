use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::clock::Clock;
use crate::deck::Deck;
use crate::tape::{Cassette, Recording, Side, Source, Tape, TapeError, TapeId, TapeKind, TrackId};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tapes (
    id        INTEGER PRIMARY KEY,
    title     TEXT    NOT NULL,
    kind      TEXT    NOT NULL CHECK (kind IN ('album', 'mixtape')),
    cassette  INTEGER NOT NULL,
    head_ns   INTEGER NOT NULL,
    facing    TEXT    NOT NULL CHECK (facing IN ('A', 'B'))
);
CREATE TABLE IF NOT EXISTS segments (
    tape_id   INTEGER NOT NULL REFERENCES tapes(id) ON DELETE CASCADE,
    side      INTEGER NOT NULL CHECK (side IN (0, 1)),
    idx       INTEGER NOT NULL,
    len_ns    INTEGER NOT NULL,
    track_id  INTEGER,
    offset_ns INTEGER NOT NULL,
    PRIMARY KEY (tape_id, side, idx)
);
CREATE TABLE IF NOT EXISTS tracks (
    id        INTEGER PRIMARY KEY,
    title     TEXT    NOT NULL,
    path      TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS decks (
    slot          INTEGER PRIMARY KEY,
    tape_id       INTEGER REFERENCES tapes(id) ON DELETE SET NULL,
    counter_gear  REAL    NOT NULL,
    counter_value REAL    NOT NULL
);
";

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    NotFound(TapeId),
    Corrupt(String),
    /// The database's folder could not be created.
    Io(PathBuf, std::io::Error),
    /// Neither `XDG_DATA_HOME` nor `HOME` says where to keep the database.
    NoDataDir,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(path, e) => write!(f, "{}: {e}", path.display()),
            StoreError::NoDataDir => f.write_str("cannot locate data directory: set XDG_DATA_HOME or HOME"),
            StoreError::Sqlite(e) => write!(f, "database error: {e}"),
            StoreError::NotFound(id) => write!(f, "no tape#{}", id.0),
            StoreError::Corrupt(why) => write!(f, "corrupt data: {why}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<TapeError> for StoreError {
    fn from(e: TapeError) -> Self {
        StoreError::Corrupt(e.to_string())
    }
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TapeSummary {
    pub id: TapeId,
    pub title: String,
    pub kind: TapeKind,
    pub cassette: Cassette,
    pub facing: Side,
    pub position: Duration,
}

/// Default database location per the XDG Base Directory spec: `$XDG_DATA_HOME/ontape/ontape.db`,
/// or `$HOME/.local/share/ontape/ontape.db` when `XDG_DATA_HOME` is unset, empty, or relative.
pub fn default_db_path() -> Result<PathBuf> {
    let absolute = |var: &str| std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute());
    let data_home = match absolute("XDG_DATA_HOME") {
        Some(dir) => dir,
        None => absolute("HOME").ok_or(StoreError::NoDataDir)?.join(".local/share"),
    };
    Ok(data_home.join("ontape").join("ontape.db"))
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens the database at `path`, creating its folder first if needed.
    pub fn open_creating_dir(path: impl AsRef<Path>) -> Result<Store> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::Io(dir.to_owned(), e))?;
        }
        Store::open(path)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        Store::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    // Tapes
    pub fn add_tape(&mut self, title: &str, tape: &Tape) -> Result<TapeId> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO tapes (title, kind, cassette, head_ns, facing) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                title,
                kind_to_sql(tape.kind()),
                tape.cassette().total_minutes() as i64,
                to_ns(tape.head()),
                side_to_sql(tape.facing())
            ],
        )?;
        let id = TapeId(tx.last_insert_rowid());
        insert_segments(&tx, id, tape)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn update_tape(&mut self, id: TapeId, tape: &Tape) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE tapes SET kind = ?2, cassette = ?3, head_ns = ?4, facing = ?5 WHERE id = ?1",
            params![
                id.0,
                kind_to_sql(tape.kind()),
                tape.cassette().total_minutes() as i64,
                to_ns(tape.head()),
                side_to_sql(tape.facing())
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(id));
        }
        tx.execute("DELETE FROM segments WHERE tape_id = ?1", params![id.0])?;
        insert_segments(&tx, id, tape)?;
        tx.commit()?;
        Ok(())
    }

    pub fn save_position(&self, id: TapeId, tape: &Tape) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tapes SET head_ns = ?2, facing = ?3 WHERE id = ?1",
            params![id.0, to_ns(tape.head()), side_to_sql(tape.facing())],
        )?;
        if changed == 0 { Err(StoreError::NotFound(id)) } else { Ok(()) }
    }

    pub fn load_tape(&self, id: TapeId) -> Result<Tape> {
        let (kind, cassette, head, facing): (String, i64, i64, String) = self.conn.query_row(
            "SELECT kind, cassette, head_ns, facing FROM tapes WHERE id = ?1",
            params![id.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
            .optional()?
            .ok_or(StoreError::NotFound(id))?;
        let cassette = cassette_from_sql(cassette)?;

        let mut pieces: [Vec<(Duration, Source)>; 2] = [Vec::new(), Vec::new()];
        let mut stmt = self.conn.prepare(
            "SELECT side, len_ns, track_id, offset_ns FROM segments WHERE tape_id = ?1 ORDER BY side, idx",
        )?;
        let rows = stmt.query_map(params![id.0], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<i64>>(2)?, row.get::<_, i64>(3)?))
        })?;
        for row in rows {
            let (side, len, track, offset) = row?;
            let side = usize::try_from(side).ok().filter(|&s| s < 2).ok_or_else(|| corrupt("bad side"))?;
            let source = match track {
                None => Source::Silence,
                Some(track) => Source::Track { track: track_from_sql(track)?, offset: from_ns(offset)? },
            };
            pieces[side].push((from_ns(len)?, source));
        }

        let len = cassette.side_len();
        let [a, b] = pieces;
        let sides = [Recording::try_from_pieces(len, a)?, Recording::try_from_pieces(len, b)?];
        Ok(Tape::from_parts(kind_from_sql(&kind)?, cassette, sides, from_ns(head)?, side_from_sql(&facing)?)?)
    }

    pub fn tape_title(&self, id: TapeId) -> Result<String> {
        self.conn.query_row("SELECT title FROM tapes WHERE id = ?1", params![id.0], |row| row.get(0))
        .optional()?
        .ok_or(StoreError::NotFound(id))
    }

    pub fn list_tapes(&self) -> Result<Vec<TapeSummary>> {
        let mut stmt = self.conn.prepare("SELECT id, title, kind, cassette, head_ns, facing FROM tapes ORDER by id")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut tapes = Vec::new();
        for row in rows {
            let (id, title, kind, cassette, head, facing) = row?;
            let cassette = cassette_from_sql(cassette)?;
            let facing = side_from_sql(&facing)?;
            let head = from_ns(head)?;
            let position = match facing {
                Side::A => head,
                Side::B => cassette.side_len().saturating_sub(head),
            };
            tapes.push(TapeSummary {
                id: TapeId(id),
                title,
                kind: kind_from_sql(&kind)?,
                cassette,
                facing,
                position,
            });
        }
        Ok(tapes)
    }
    pub fn delete_tape(&self, id: TapeId) -> Result<()> {
        let changed = self.conn.execute("DELETE FROM tapes WHERE id = ?1", params![id.0])?;
        if changed == 0 { Err(StoreError::NotFound(id)) } else { Ok(()) }
    }

    // Tracks
    pub fn new_track_ids(&self, count: usize) -> Result<Vec<TrackId>> {
        let highest: Option<i64> = self.conn.query_row(
            "SELECT MAX(id) FROM (SELECT MAX(id) AS id FROM tracks UNION ALL SELECT MAX(track_id) FROM segments)", 
        [], |row| row.get(0),
        )?;
        let first = match highest {
            Some(id) => track_from_sql(id)?.0 + 1,
            None => 1,
        };

        Ok((first..).take(count).map(TrackId).collect())
    }
    /// Records where the audio for `id` lives, replacing any earlier entry.
    pub fn add_track(&self, id: TrackId, title: &str, path: &Path) -> Result<()> {
        let path = path.to_str().ok_or_else(|| corrupt("track path is not valid UTF-8"))?;
        self.conn.execute(
            "INSERT OR REPLACE INTO tracks (id, title, path) VALUES (?1, ?2, ?3)",
            params![track_to_sql(id)?, title, path],
        )?;
        Ok(())
    }

    /// The audio file for `id`, or `None` for tracks without one (e.g. from `album`).
    pub fn track_path(&self, id: TrackId) -> Result<Option<PathBuf>> {
        let path: Option<String> = self
            .conn
            .query_row("SELECT path FROM tracks WHERE id = ?1", params![track_to_sql(id)?], |row| row.get(0))
            .optional()?;
        Ok(path.map(PathBuf::from))
    }

    pub fn track_title(&self, id: TrackId) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT title FROM tracks WHERE id = ?1", params![track_to_sql(id)?], |row| row.get(0))
            .optional()?)
    }

    // Decks
    pub fn save_deck(&self, slot: u8, deck: &Deck) -> Result<()> {
        let tape_id = deck.tape().map(|(id, _)| id.0);
        self.conn.execute(
            "INSERT INTO decks (slot, tape_id, counter_gear, counter_value) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (slot) DO UPDATE SET
                tape_id = excluded.tape_id,
                counter_gear = excluded.counter_gear,
                counter_value = excluded.counter_value",
            params![slot, tape_id, deck.counter_gear(), deck.counter_value()],
        )?;
        if let Some((id, tape)) = deck.tape() {
            self.save_position(id, tape)?;
        }
        Ok(())
    }

    pub fn load_deck(&self, slot: u8, clock: Arc<dyn Clock>) -> Result<Deck> {
        let saved: Option<(Option<i64>, f64, f64)> = self
            .conn
            .query_row(
                "SELECT tape_id, counter_gear, counter_value FROM decks WHERE slot = ?1",
                params![slot],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((tape_id, gear, value)) = saved else {
            return Ok(Deck::new(clock));
        };
        let tape = match tape_id {
            Some(id) => Some((TapeId(id), self.load_tape(TapeId(id))?)),
            None => None,
        };
        Ok(Deck::restore(clock, tape, gear, value))
    }
}

fn insert_segments(conn: &Connection, id: TapeId, tape: &Tape) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO segments (tape_id, side, idx, len_ns, track_id, offset_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for side in [Side::A, Side::B] {
        for (idx, seg) in tape.recording(side).segments().iter().enumerate() {
            let (track, offset) = match seg.source {
                Source::Silence => (None, 0),
                Source::Track { track, offset } => (Some(track_to_sql(track)?), to_ns(offset)),
            };
            stmt.execute(params![id.0, side.index() as i64, idx as i64, to_ns(seg.len), track, offset])?;
        }
    }
    Ok(())
}

fn corrupt(why: &str) -> StoreError {
    StoreError::Corrupt(why.to_owned())
}

fn to_ns(d: Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}

fn from_ns(ns: i64) -> Result<Duration> {
    u64::try_from(ns).map(Duration::from_nanos).map_err(|_| corrupt("negative time"))
}

fn track_to_sql(track: TrackId) -> Result<i64> {
    i64::try_from(track.0).map_err(|_| corrupt("track id too large to store"))
}

fn track_from_sql(track: i64) -> Result<TrackId> {
    u64::try_from(track).map(TrackId).map_err(|_| corrupt("negative track id"))
}

fn kind_to_sql(kind: TapeKind) -> &'static str {
    match kind {
        TapeKind::Album => "album",
        TapeKind::Mixtape => "mixtape",
    }
}

fn kind_from_sql(kind: &str) -> Result<TapeKind> {
    match kind {
        "album" => Ok(TapeKind::Album),
        "mixtape" => Ok(TapeKind::Mixtape),
        _ => Err(corrupt("unknown tape kind")),
    }
}

fn side_to_sql(side: Side) -> &'static str {
    match side {
        Side::A => "A",
        Side::B => "B",
    }
}

fn side_from_sql(side: &str) -> Result<Side> {
    match side {
        "A" => Ok(Side::A),
        "B" => Ok(Side::B),
        _ => Err(corrupt("unknown side")),
    }
}

fn cassette_from_sql(minutes: i64) -> Result<Cassette> {
    Cassette::ALL
        .into_iter()
        .find(|c| c.total_minutes() as i64 == minutes)
        .ok_or_else(|| corrupt("unknown cassette length"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::tape::{LayoutOptions, TrackInfo, layout_album};

    fn album() -> Tape {
        let tracks: Vec<_> = (0..4).map(|i| TrackInfo { id: TrackId(i), len: Duration::from_secs(200 + i) }).collect();
        layout_album(&tracks, &LayoutOptions::default()).unwrap().remove(0)
    }

    #[test]
    fn album_round_trips_exactly() {
        let mut store = Store::open_in_memory().unwrap();
        let mut tape = album();
        tape.set_position(Duration::from_millis(123_456));
        tape.flip();
        let id = store.add_tape("Test", &tape).unwrap();
        assert_eq!(store.load_tape(id).unwrap(), tape);
        assert_eq!(store.tape_title(id).unwrap(), "Test");
    }

    #[test]
    fn update_tape_rewrites_recording() {
        let mut store = Store::open_in_memory().unwrap();
        let mut tape = Tape::blank(Cassette::C46);
        let id = store.add_tape("Mix", &tape).unwrap();
        tape.record(Duration::from_secs(30), Source::Track { track: TrackId(9), offset: Duration::ZERO }).unwrap();
        store.update_tape(id, &tape).unwrap();
        assert_eq!(store.load_tape(id).unwrap(), tape);
    }

    #[test]
    fn save_position_only_moves_head() {
        let mut store = Store::open_in_memory().unwrap();
        let mut tape = album();
        let id = store.add_tape("Test", &tape).unwrap();
        tape.set_position(Duration::from_secs(90));
        store.save_position(id, &tape).unwrap();
        assert_eq!(store.load_tape(id).unwrap(), tape);
    }

    #[test]
    fn list_and_delete() {
        let mut store = Store::open_in_memory().unwrap();
        let a = store.add_tape("A", &Tape::blank(Cassette::C60)).unwrap();
        let b = store.add_tape("B", &album()).unwrap();
        let list = store.list_tapes().unwrap();
        assert_eq!(list.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(list[1].kind, TapeKind::Album);
        store.delete_tape(a).unwrap();
        assert!(matches!(store.load_tape(a), Err(StoreError::NotFound(_))));
        assert!(matches!(store.delete_tape(a), Err(StoreError::NotFound(_))));
        assert_eq!(store.list_tapes().unwrap().len(), 1);
    }

    #[test]
    fn unsaved_deck_slot_is_empty() {
        let store = Store::open_in_memory().unwrap();
        let deck = store.load_deck(0, Arc::new(ManualClock::new())).unwrap();
        assert!(deck.is_empty());
    }

    #[test]
    fn deck_round_trips_and_delete_empties_it() {
        let mut store = Store::open_in_memory().unwrap();
        let clock = Arc::new(ManualClock::new());
        let id = store.add_tape("Test", &album()).unwrap();
        let mut deck = Deck::new(clock.clone());
        deck.insert(id, store.load_tape(id).unwrap()).unwrap();
        deck.play().unwrap();
        clock.advance(Duration::from_secs(100));
        deck.stop().unwrap();
        store.save_deck(0, &deck).unwrap();

        let back = store.load_deck(0, clock.clone()).unwrap();
        assert_eq!(back.position(), Some(Duration::from_secs(100)));
        assert_eq!(back.counter_reading(), deck.counter_reading());

        drop(deck);
        let mut deck = store.load_deck(0, clock.clone()).unwrap();
        deck.eject().unwrap();
        store.save_deck(0, &deck).unwrap();
        store.delete_tape(id).unwrap();
        assert!(store.load_deck(0, clock).unwrap().is_empty());
    }

    #[test]
    fn corrupt_segments_are_reported() {
        let mut store = Store::open_in_memory().unwrap();
        let id = store.add_tape("Test", &album()).unwrap();
        store.conn.execute("DELETE FROM segments WHERE tape_id = ?1 AND idx = 0", params![id.0]).unwrap();
        assert!(matches!(store.load_tape(id), Err(StoreError::Corrupt(_))));
    }

    #[test]
    fn tracks_remember_their_files() {
        let store = Store::open_in_memory().unwrap();
        let id = TrackId(7);
        assert_eq!(store.track_path(id).unwrap(), None);
        store.add_track(id, "Song", Path::new("/music/song.ogg")).unwrap();
        assert_eq!(store.track_path(id).unwrap(), Some(PathBuf::from("/music/song.ogg")));
        assert_eq!(store.track_title(id).unwrap().as_deref(), Some("Song"));
    }
    #[test]
    fn new_track_ids_start_at_one_and_are_distinct() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.new_track_ids(3).unwrap(), [TrackId(1), TrackId(2), TrackId(3)]);
    }

    #[test]
    fn new_track_ids_skip_ids_in_use() {
        let mut store = Store::open_in_memory().unwrap();
        store.add_track(TrackId(7), "Song", Path::new("/music/song.ogg")).unwrap();
        assert_eq!(store.new_track_ids(1).unwrap(), [TrackId(8)]);

        // A made-up track has no row in `tracks`, only in a tape's segments.
        let mut tape = Tape::blank(Cassette::C46);
        tape.record(Duration::from_secs(30), Source::Track { track: TrackId(20), offset: Duration::ZERO }).unwrap();
        store.add_tape("Mix", &tape).unwrap();
        assert_eq!(store.new_track_ids(2).unwrap(), [TrackId(21), TrackId(22)]);
    }

    #[test]
    fn adding_a_track_twice_fails() {
        let store = Store::open_in_memory().unwrap();
        store.add_track(TrackId(1), "One", Path::new("/music/1.ogg")).unwrap();
        assert!(store.add_track(TrackId(1), "Other", Path::new("/music/other.ogg")).is_err());
        assert_eq!(store.track_path(TrackId(1)).unwrap(), Some(PathBuf::from("/music/1.ogg")));
    }

    #[test]
    fn file_database_survives_reopen() {
        let dir = std::env::temp_dir().join(format!("ontape-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        let _ = std::fs::remove_file(&path);
        let tape = album();
        let id = Store::open(&path).unwrap().add_tape("Kept", &tape).unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(store.load_tape(id).unwrap(), tape);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
