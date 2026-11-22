use std::error::Error;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ontape_core::clock::Clock;
use ontape_core::deck::{DeckEvent, DeckState};
use ontape_core::player::{self, Player};
use ontape_core::session::{DeckSession, SessionError};
use ontape_core::store::Store;
use ontape_core::tape::{
    Cassette, Direction, LayoutOptions, Side, Source, Tape, TapeId, TapeKind, TrackId, TrackInfo, layout_album,
};

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version)]
struct Options {
    /// Library database [default: $XDG_DATA_HOME/ontape/ontape.db, falling back to ~/.local/share/ontape/ontape.db]
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,

    #[arg(long, value_name = "N", default_value_t = 1.0, value_parser = parse_speed,
    allow_negative_numbers = true)]
    speed: f64,

    /// Don't open an audio device; the deck runs silently.
    #[arg(long)]
    mute: bool,
}

#[derive(Debug, Parser)]
#[command(
    name = "ontape",
    no_binary_name = true,
    disable_version_flag = true,
    help_template = "{all-args}",
    subcommand_value_name = "COMMAND"
)]
struct Prompt {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(visible_alias = "ls")]
    Tapes,
    Album {
        title: String,
        
        #[arg(required = true, value_name = "LENGTH", value_parser = parse_len)]
        lengths: Vec<Duration>,
    },
    /// Record an album tape from an audio file or a folder of them.
    Import {
        path: PathBuf,
        title: Option<String>,
    },
    Blank {
        #[arg(value_parser = parse_cassette)]
        cassette: Cassette,
        title: Option<String>,
    },

    #[command(visible_alias = "rm")]
    Delete {
        #[arg(value_parser = parse_id)]
        id: TapeId,
    },

    #[command(visible_alias = "in")]
    Insert {
        #[arg(value_parser = parse_id)]
        id: TapeId,
    },
    Eject,

    #[command(visible_alias = "p")]
    Play,
    
    #[command(visible_alias = "s")]
    Stop,
    Ff,
    
    #[command(visible_alias = "rw")]
    Rew,
    Cue,
    Review,
    
    #[command(visible_alias = "rel")]
    Release,
    Flip,
    Reset,

    #[command(visible_alias = "st")]
    Status,

    #[command(visible_alias = "w")]
    Watch {
        #[arg(value_parser = parse_seconds)]
        seconds: Option<Duration>,
    },

    #[command(visible_aliases = ["exit", "q"])]
    Quit,
}

struct ScaledClock {
    origin: Instant,
    speed: f64,
}

impl Clock for ScaledClock {
    fn now(&self) -> Instant {
        self.origin + self.origin.elapsed().mul_f64(self.speed)
    }
}

fn main() -> ExitCode {
    match run(Options::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Default database location per the XDG Base Directory spec: `$XDG_DATA_HOME/ontape/ontape.db`,
/// or `$HOME/.local/share/ontape/ontape.db` when `XDG_DATA_HOME` is unset, empty, or relative.
fn default_db_path() -> Result<PathBuf, Box<dyn Error>> {
    let absolute = |var: &str| std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute());
    let data_home = match absolute("XDG_DATA_HOME") {
        Some(dir) => dir,
        None => absolute("HOME")
            .ok_or("cannot locate data directory: set XDG_DATA_HOME or HOME, or pass --db")?
            .join(".local/share"),
    };
    Ok(data_home.join("ontape").join("ontape.db"))
}

enum Flow {
    Continue,
    Watch(Option<Duration>),
    Quit,
}

fn run(opts: Options) -> Result<(), Box<dyn Error>> {
    let clock: Arc<dyn Clock> = Arc::new(ScaledClock { origin: Instant::now(), speed: opts.speed });
    let db = match opts.db {
        Some(path) => path,
        None => default_db_path()?,
    };
    if let Some(dir) = db.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let store = Store::open(&db)?;
    let session = Arc::new(Mutex::new(DeckSession::open(store, 0, clock)?));

    let speed_note = if opts.speed == 1.0 { String::new() } else { format!(", clock ×{}", opts.speed) };
    println!("OnTape deck ({}{speed_note}). Type 'help' for commands.", db.display());
    println!("{}", status_line(&lock(&session)));

    let running = Arc::new(AtomicBool::new(true));
    let ticker = {
        let session = session.clone();
        let running = running.clone();
        let (speed, mute) = (opts.speed, opts.mute);
        thread::spawn(move || {
            // The audio device is opened on this thread: rodio's output stream may not be `Send`.
            let mut player = if mute {
                None
            } else {
                Player::open(speed).inspect_err(|e| eprintln!("* {e}; playing silently")).ok()
            };
            while running.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
                let mut session = lock(&session);
                match session.tick() {
                    Ok(events) => {
                        for event in events {
                            println!("\n* {}", describe(event));
                        }
                    }
                    Err(e) => eprintln!("\n* saving failed: {e}"),
                }
                if let Some(player) = &mut player
                    && let Err(e) = player.sync(&session)
                {
                    eprintln!("\n* playback: {e}");
                }
            }
        })
    };

    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        print!("ontape> ");
        io::stdout().flush()?;
        line.clear();
        if stdin.lock().read_line(&mut line)? == 0 {
            println!();
            break;
        }
        let args = match split_args(&line) {
            Ok(args) if args.is_empty() => continue,
            Ok(args) => args,
            Err(e) => {
                println!("error: {e}");
                continue;
            }
        };
        let prompt = match Prompt::try_parse_from(&args) {
            Ok(prompt) => prompt,
            Err(e) => {
                let _ = e.print();
                continue;
            }
        };
        let flow = command(&mut lock(&session), prompt.command);
        match flow {
            Ok(Flow::Continue) => {}
            Ok(Flow::Watch(limit)) => watch(&session, limit),
            Ok(Flow::Quit) => break,
            Err(e) => println!("error: {e}"),
        }
    }

    running.store(false, Ordering::Relaxed);
    let _ = ticker.join();
    let mut session = lock(&session);
    session.stop().or_else(|e| match e {
        SessionError::Deck(_) => Ok(()),
        e => Err(e),
    })?;
    session.save()?;
    Ok(())
}

fn lock(session: &Mutex<DeckSession>) -> MutexGuard<'_, DeckSession> {
    session.lock().unwrap_or_else(|e| e.into_inner())
}

fn command(s: &mut DeckSession, command: Command) -> Result<Flow, Box<dyn Error>> {
    match command {
        Command::Quit => return Ok(Flow::Quit),

        Command::Tapes => list_tapes(s)?,
        Command::Album { title, lengths } => add_album(s, &title, &lengths)?,
        Command::Import { path, title } => import(s, &path, title)?,
        Command::Blank { cassette, title } => {
            let title = title.unwrap_or_else(|| format!("Blank {cassette}"));
            let id = s.store_mut().add_tape(&title, &Tape::blank(cassette))?;
            println!("added #{} \"{title}\"", id.0);
        }
        Command::Delete { id } => {
            if s.deck().tape().is_some_and(|(inserted, _)| inserted == id) {
                return Err("eject the tape first".into());
            }
            s.store().delete_tape(id)?;
            println!("deleted #{}", id.0);
        }

        Command::Insert { id } => {
            s.insert(id)?;
            println!("{}", status_line(s));
        }
        Command::Eject => {
            let id = s.eject()?;
            println!("ejected #{}", id.0);
        }
        Command::Play => button(s, DeckSession::play)?,
        Command::Stop => button(s, DeckSession::stop)?,
        Command::Ff => button(s, |s| s.wind(Direction::Forward))?,
        Command::Rew => button(s, |s| s.wind(Direction::Backward))?,
        Command::Cue => button(s, |s| s.cue(Direction::Forward))?,
        Command::Review => button(s, |s| s.cue(Direction::Backward))?,
        Command::Release => button(s, DeckSession::release_cue)?,
        Command::Flip => button(s, DeckSession::flip)?,
        Command::Reset => button(s, DeckSession::reset_counter)?,
        Command::Status => println!("{}", status_line(s)),
        Command::Watch { seconds } => return Ok(Flow::Watch(seconds)),
    }
    Ok(Flow::Continue)
}

fn button<F>(s: &mut DeckSession, press: F) -> Result<(), Box<dyn Error>>
where
    F: FnOnce(&mut DeckSession) -> ontape_core::session::Result<()>,
{
    press(s)?;
    println!("{}", status_line(s));
    Ok(())
}

fn watch(session: &Mutex<DeckSession>, limit: Option<Duration>) {
    let started = Instant::now();
    let limit = limit.unwrap_or(Duration::from_secs(600));
    loop {
        let (line, moving) = {
            let s = lock(session);
            (status_line(&s), s.deck().is_moving())
        };
        print!("\r{line}\x1b[K");
        let _ = io::stdout().flush();
        if !moving || started.elapsed() >= limit {
            break;
        }

        thread::sleep(Duration::from_millis(250));
    }
    println!();
}

fn new_track_ids(count: usize) -> Result<impl Iterator<Item = TrackId>, Box<dyn Error>> {
    let base = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64 * 100;
    Ok((0..count as u64).map(move |i| TrackId(base + i)))
}

fn add_album(s: &mut DeckSession, title: &str, lens: &[Duration]) -> Result<(), Box<dyn Error>> {
    let tracks: Vec<TrackInfo> = new_track_ids(lens.len())?.zip(lens).map(|(id, &len)| TrackInfo { id, len }).collect();
    record_album(s, title, &tracks)
}

const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "ogg", "oga", "opus", "flac", "wav", "m4a", "mp4", "aac", "aiff", "aif", "caf", "mka", "mkv", "webm"];

fn import(s: &mut DeckSession, path: &Path, title: Option<String>) -> Result<(), Box<dyn Error>> {
    let path = path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?;
    let files = if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && is_audio(p))
            .collect();
        files.sort_by_cached_key(|p| track_order(p));
        files
    } else {
        vec![path.clone()]
    };
    if files.is_empty() {
        return Err(format!("no playable audio in {} (looked for {})", path.display(), AUDIO_EXTENSIONS.join(", ")).into());
    }

    // Probe every file before writing anything, so a bad file doesn't leave half an album behind.
    let mut probed = Vec::new();
    for file in &files {
        probed.push((file, player::probe_len(file)?));
    }
    let mut tracks = Vec::new();
    for (id, (file, len)) in new_track_ids(files.len())?.zip(probed) {
        let name = file.file_stem().unwrap_or_default().to_string_lossy();
        s.store().add_track(id, &name, file)?;
        tracks.push(TrackInfo { id, len });
    }
    let title = title.unwrap_or_else(|| {
        let named = if path.is_dir() { &path } else { &files[0] };
        named.file_stem().unwrap_or_default().to_string_lossy().into_owned()
    });
    record_album(s, &title, &tracks)
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

/// Sorts "2. Foo" before "10. Bar": by leading track number, then by name.
fn track_order(path: &Path) -> (u64, String) {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
    (digits.parse().unwrap_or(u64::MAX), name)
}

fn record_album(s: &mut DeckSession, title: &str, tracks: &[TrackInfo]) -> Result<(), Box<dyn Error>> {
    let tapes = layout_album(tracks, &LayoutOptions::default())?;
    let count = tapes.len();
    for (i, tape) in tapes.iter().enumerate() {
        let name = if count == 1 { title.to_owned() } else { format!("{title} ({}/{count})", i + 1) };
        let id = s.store_mut().add_tape(&name, tape)?;
        let side_tracks = |side| {
            tape.recording(side).segments().iter().filter(|seg| matches!(seg.source, Source::Track { .. })).count()
        };
        println!(
            "added #{} \"{name}\" on a {}: side A {} tracks, side B {} tracks",
            id.0,
            tape.cassette(),
            side_tracks(Side::A),
            side_tracks(Side::B)
        );
    }
    Ok(())
}

fn list_tapes(s: &DeckSession) -> Result<(), Box<dyn Error>> {
    let tapes = s.store().list_tapes()?;
    if tapes.is_empty() {
        println!("the shelf is empty; try 'album' or 'blank'");
    }
    let inserted = s.deck().tape().map(|(id, _)| id);
    for t in tapes {
        let kind = match t.kind {
            TapeKind::Album => "album",
            TapeKind::Mixtape => "mixtape",
        };
        let mark = if inserted == Some(t.id) { "  [in deck]" } else { "" };
        println!(
            "#{:<4} {:<5} {:<8} side {}  {:>5}  \"{}\"{mark}",
            t.id.0,
            t.cassette.to_string(),
            kind,
            side_name(t.facing),
            fmt_time(t.position),
            t.title
        );
    }
    Ok(())
}

fn status_line(s: &DeckSession) -> String {
    let status = s.deck().status();
    let counter = format!("[{:03}]", status.counter);
    let Some(t) = status.tape else {
        return format!("{counter} deck empty");
    };
    let title = s.store().tape_title(t.id).unwrap_or_default();
    let state = match status.state {
        DeckState::Empty => "",
        DeckState::Stopped => "■ stop",
        DeckState::Playing => "▶ play",
        DeckState::Winding(Direction::Forward) => "▶▶ ff",
        DeckState::Winding(Direction::Backward) => "◀◀ rew",
        DeckState::Cueing(Direction::Forward) => "▶▶ cue",
        DeckState::Cueing(Direction::Backward) => "◀◀ review",
    };
    let under = match t.source {
        None => "end of side".to_owned(),
        Some(Source::Silence) => "blank tape".to_owned(),
        Some(Source::Track { track, offset }) => {
            let song = s.deck().tape().map_or(0, |(_, tape)| song_number(tape, t.facing, t.position));
            match s.store().track_title(track).ok().flatten() {
                Some(name) => format!("song {song} \"{name}\" at {}", fmt_time(offset)),
                None => format!("song {song} at {}", fmt_time(offset)),
            }
        }
    };
    format!(
        "{counter} #{} \"{title}\" {} side {}  {state:<9}  {} / {}  {:>4.1}×  {under}",
        t.id.0,
        t.cassette,
        side_name(t.facing),
        fmt_time(t.position),
        fmt_time(t.side_len),
        t.speed
    )
}

fn song_number(tape: &Tape, side: Side, position: Duration) -> usize {
    tape.recording(side)
        .segments()
        .iter()
        .take_while(|seg| seg.start <= position)
        .filter(|seg| matches!(seg.source, Source::Track { .. }))
        .count()
}

fn describe(event: DeckEvent) -> String {
    match event {
        DeckEvent::EndOfSide(side) => format!("end of side {}, deck stopped (flip the tape?)", side_name(side)),
        DeckEvent::WindFinished(Direction::Forward) => "fast-forward reached the end".to_owned(),
        DeckEvent::WindFinished(Direction::Backward) => "rewound to the start".to_owned(),
    }
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::A => "A",
        Side::B => "B",
    }
}

fn fmt_time(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn parse_len(text: &str) -> Result<Duration, String> {
    let bad = || "expected m:ss or seconds, e.g. 4:10".to_owned();
    let secs = match text.split_once(':') {
        Some((m, s)) => {
            let m: u64 = m.parse().map_err(|_| bad())?;
            let s: f64 = s.parse().map_err(|_| bad())?;
            if !(0.0..60.0).contains(&s) {
                return Err(bad());
            }
            m as f64 * 60.0 + s
        }
        None => text.parse().map_err(|_| bad())?,
    };
    if !(secs > 0.0 && secs.is_finite()) {
        return Err(bad());
    }
    Ok(Duration::from_secs_f64(secs))
}

fn parse_cassette(text: &str) -> Result<Cassette, String> {
    Cassette::ALL
        .into_iter()
        .find(|c| c.to_string().eq_ignore_ascii_case(text))
        .ok_or_else(|| "expected C46, C60, C90 or C120".to_owned())
}

fn parse_id(text: &str) -> Result<TapeId, String> {
    text.trim_start_matches('#').parse().map(TapeId).map_err(|_| "expected a tape number, e.g. 3".to_owned())
}

fn parse_speed(text: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|v| *v > 0.0 && v.is_finite())
        .ok_or_else(|| "expected a number above 0, e.g. 10".to_owned())
}

fn parse_seconds(text: &str) -> Result<Duration, String> {
    parse_speed(text).map(Duration::from_secs_f64).map_err(|_| "expected a number of seconds above 0".to_owned())
}

fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quoted {
        return Err("missing closing quote".into());
    }
    if in_word {
        args.push(current);
    }
    Ok(args)
}
