//! Turning audio files into album tapes. Shared by the CLI and the desktop app.
//!
//! Importing is split in two so a UI can keep responsive: [`scan`] reads the
//! files and works out their lengths (slow, safe on any thread), and
//! [`record`] writes the tracks and tapes to the store (fast, on the thread
//! that owns the store).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::audio::AudioError;
use crate::player;
use crate::store::{Store, StoreError};
use crate::tape::{
    Cassette, LayoutError, LayoutOptions, Side, Source, TapeId, TrackInfo, layout_album,
};

/// File extensions treated as audio when importing a folder.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "ogg", "oga", "opus", "flac", "wav", "m4a", "mp4", "aac", "aiff", "aif", "caf", "mka",
    "mkv", "webm",
];

#[derive(Debug)]
pub enum LibraryError {
    Io(PathBuf, std::io::Error),
    /// The path holds no files with an audio extension.
    NoAudio(PathBuf),
    Audio(AudioError),
    Layout(LayoutError),
    Store(StoreError),
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LibraryError::Io(path, e) => write!(f, "{}: {e}", path.display()),
            LibraryError::NoAudio(path) => {
                write!(
                    f,
                    "no playable audio in {} (looked for {})",
                    path.display(),
                    AUDIO_EXTENSIONS.join(", ")
                )
            }
            LibraryError::Audio(e) => e.fmt(f),
            LibraryError::Layout(e) => e.fmt(f),
            LibraryError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LibraryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LibraryError::Io(_, e) => Some(e),
            LibraryError::NoAudio(_) => None,
            LibraryError::Audio(e) => Some(e),
            LibraryError::Layout(e) => Some(e),
            LibraryError::Store(e) => Some(e),
        }
    }
}

impl From<AudioError> for LibraryError {
    fn from(e: AudioError) -> Self {
        LibraryError::Audio(e)
    }
}

impl From<LayoutError> for LibraryError {
    fn from(e: LayoutError) -> Self {
        LibraryError::Layout(e)
    }
}

impl From<StoreError> for LibraryError {
    fn from(e: StoreError) -> Self {
        LibraryError::Store(e)
    }
}

pub type Result<T, E = LibraryError> = std::result::Result<T, E>;

/// An audio file ready to be recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub title: String,
    pub len: Duration,
}

/// An album's files, in track order, with their lengths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedAlbum {
    pub title: String,
    pub files: Vec<ScannedFile>,
}

/// A tape that [`record`] put on the shelf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTape {
    pub id: TapeId,
    pub title: String,
    pub cassette: Cassette,
    /// Number of songs on side A and side B.
    pub songs: [usize; 2],
}

/// Reads an audio file, or every audio file in a folder, and works out how
/// long each is. Every file is checked before anything is recorded, so a bad
/// file doesn't leave half an album behind. `title` defaults to the folder
/// (or file) name.
pub fn scan(path: &Path, title: Option<String>) -> Result<ScannedAlbum> {
    let path = path
        .canonicalize()
        .map_err(|e| LibraryError::Io(path.to_owned(), e))?;
    let files = if path.is_dir() {
        let mut files = Vec::new();
        collect_audio(&path, &mut files)?;
        files
    } else {
        vec![path.clone()]
    };
    if files.is_empty() {
        return Err(LibraryError::NoAudio(path));
    }

    let mut scanned = Vec::with_capacity(files.len());
    for file in files {
        let len = player::probe_len(&file)?;
        scanned.push(ScannedFile {
            title: stem(&file),
            path: file,
            len,
        });
    }
    let title = title.unwrap_or_else(|| {
        if path.is_dir() {
            stem(&path)
        } else {
            scanned[0].title.clone()
        }
    });
    Ok(ScannedAlbum {
        title,
        files: scanned,
    })
}

/// Adds a scanned album's tracks to the library and records it onto tapes.
pub fn record(store: &mut Store, album: &ScannedAlbum) -> Result<Vec<RecordedTape>> {
    let mut tracks = Vec::with_capacity(album.files.len());
    let ids = store.new_track_ids(album.files.len())?;
    for (id, file) in ids.into_iter().zip(&album.files) {
        store.add_track(id, &file.title, &file.path)?;
        tracks.push(TrackInfo { id, len: file.len });
    }
    record_tracks(store, &album.title, &tracks)
}

/// Records an album of made-up tracks with these lengths (no audio files).
pub fn record_lengths(
    store: &mut Store,
    title: &str,
    lens: &[Duration],
) -> Result<Vec<RecordedTape>> {
    let ids = store.new_track_ids(lens.len())?;
    let tracks: Vec<TrackInfo> = ids
        .into_iter()
        .zip(lens)
        .map(|(id, &len)| TrackInfo { id, len })
        .collect();
    record_tracks(store, title, &tracks)
}

/// Lays `tracks` out onto tapes and puts them on the shelf. An album too long
/// for one tape becomes a box set titled "Title (1/2)", "Title (2/2)"…
pub fn record_tracks(
    store: &mut Store,
    title: &str,
    tracks: &[TrackInfo],
) -> Result<Vec<RecordedTape>> {
    let tapes = layout_album(tracks, &LayoutOptions::default())?;
    let count = tapes.len();
    let mut recorded = Vec::with_capacity(count);
    for (i, tape) in tapes.iter().enumerate() {
        let name = if count == 1 {
            title.to_owned()
        } else {
            format!("{title} ({}/{count})", i + 1)
        };
        let id = store.add_tape(&name, tape)?;
        let songs = |side| {
            tape.recording(side)
                .segments()
                .iter()
                .filter(|seg| matches!(seg.source, Source::Track { .. }))
                .count()
        };
        recorded.push(RecordedTape {
            id,
            title: name,
            cassette: tape.cassette(),
            songs: [songs(Side::A), songs(Side::B)],
        });
    }
    Ok(recorded)
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

/// Appends the audio files in `dir` and, after them, those in its subfolders
/// (e.g. "CD1", "CD2", or an artist's albums), each folder in track order.
/// Hidden files and folders are skipped.
fn collect_audio(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| LibraryError::Io(dir.to_owned(), e))?;
    let (mut here, mut subdirs) = (Vec::new(), Vec::new());
    for path in entries.filter_map(|entry| entry.ok().map(|e| e.path())) {
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if path.is_dir() {
            subdirs.push(path);
        } else if path.is_file() && is_audio(&path) {
            here.push(path);
        }
    }
    here.sort_by_cached_key(|p| track_order(p));
    subdirs.sort_by_cached_key(|p| track_order(p));
    files.append(&mut here);
    for sub in subdirs {
        collect_audio(&sub, files)?;
    }
    Ok(())
}

/// Sorts "2. Foo" before "10. Bar": by leading track number, then by name.
fn track_order(path: &Path) -> (u64, String) {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
    (digits.parse().unwrap_or(u64::MAX), name)
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn min(m: u64) -> Duration {
        Duration::from_secs(m * 60)
    }

    #[test]
    fn recognises_audio_extensions() {
        assert!(is_audio(Path::new("a/01 Song.FLAC")));
        assert!(is_audio(Path::new("x.opus")));
        assert!(!is_audio(Path::new("cover.jpg")));
        assert!(!is_audio(Path::new("README")));
    }

    #[test]
    fn orders_by_track_number_then_name() {
        let mut names = vec![
            "10. Ten.mp3",
            "2. Two.mp3",
            "Bonus.mp3",
            "1. One.mp3",
            "Another.mp3",
        ];
        names.sort_by_cached_key(|n| track_order(Path::new(n)));
        assert_eq!(
            names,
            [
                "1. One.mp3",
                "2. Two.mp3",
                "10. Ten.mp3",
                "Another.mp3",
                "Bonus.mp3"
            ]
        );
    }

    #[test]
    fn back_to_back_imports_keep_their_own_tracks() {
        let mut store = Store::open_in_memory().unwrap();
        let album = |dir: &str| ScannedAlbum {
            title: dir.into(),
            files: vec![ScannedFile {
                path: format!("/{dir}/1.flac").into(),
                title: "One".into(),
                len: min(4),
            }],
        };
        let first = record(&mut store, &album("first")).unwrap();
        let made_up = record_lengths(&mut store, "Made up", &[min(4)]).unwrap();
        let second = record(&mut store, &album("second")).unwrap();

        let first_song = |id: TapeId| {
            let tape = store.load_tape(id).unwrap();
            let Source::Track { track, .. } = tape.recording(Side::A).segments()[1].source else {
                panic!("the first song follows the leader");
            };
            track
        };
        let tracks = [
            first_song(first[0].id),
            first_song(made_up[0].id),
            first_song(second[0].id),
        ];
        assert!(tracks[0] != tracks[1] && tracks[1] != tracks[2] && tracks[0] != tracks[2]);
        assert_eq!(
            store.track_path(tracks[0]).unwrap(),
            Some(PathBuf::from("/first/1.flac"))
        );
        assert_eq!(store.track_path(tracks[1]).unwrap(), None);
        assert_eq!(
            store.track_path(tracks[2]).unwrap(),
            Some(PathBuf::from("/second/1.flac"))
        );
    }

    #[test]
    fn recording_a_scanned_album_adds_tracks_and_a_tape() {
        let mut store = Store::open_in_memory().unwrap();
        let album = ScannedAlbum {
            title: "Test".into(),
            files: vec![
                ScannedFile {
                    path: "/music/1.flac".into(),
                    title: "One".into(),
                    len: min(4),
                },
                ScannedFile {
                    path: "/music/2.flac".into(),
                    title: "Two".into(),
                    len: min(3),
                },
            ],
        };
        let recorded = record(&mut store, &album).unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].title, "Test");
        assert_eq!(recorded[0].cassette, Cassette::C46);
        assert_eq!(recorded[0].songs, [1, 1]);

        let tape = store.load_tape(recorded[0].id).unwrap();
        let Source::Track { track, .. } = tape.recording(Side::A).segments()[1].source else {
            panic!("the first song follows the leader");
        };
        assert_eq!(store.track_title(track).unwrap().as_deref(), Some("One"));
        assert_eq!(
            store.track_path(track).unwrap(),
            Some(PathBuf::from("/music/1.flac"))
        );
    }

    #[test]
    fn long_albums_become_numbered_box_sets() {
        let mut store = Store::open_in_memory().unwrap();
        let recorded = record_lengths(&mut store, "Epic", &[min(50); 3]).unwrap();
        let titles: Vec<&str> = recorded.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Epic (1/2)", "Epic (2/2)"]);
        assert_eq!(store.list_tapes().unwrap().len(), 2);
    }

    #[test]
    fn collects_audio_from_subfolders_in_order() {
        let dir =
            std::env::temp_dir().join(format!("ontape-library-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["CD2", "CD1", ".hidden"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for file in [
            "CD1/2. b.ogg",
            "CD1/1. a.ogg",
            "CD2/1. c.opus",
            ".hidden/x.mp3",
            "intro.mp3",
            "cover.jpg",
        ] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        let mut files = Vec::new();
        collect_audio(&dir, &mut files).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().display().to_string())
            .collect();
        assert_eq!(
            names,
            ["intro.mp3", "CD1/1. a.ogg", "CD1/2. b.ogg", "CD2/1. c.opus"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanning_nothing_playable_fails() {
        let dir = std::env::temp_dir().join(format!("ontape-library-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "hi").unwrap();
        assert!(matches!(scan(&dir, None), Err(LibraryError::NoAudio(_))));
        assert!(matches!(
            scan(&dir.join("missing"), None),
            Err(LibraryError::Io(..))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
