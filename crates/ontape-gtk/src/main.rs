mod app;
mod cassette;
mod cassette_widget;
mod mpris;
mod shelf;
mod transport;

use app::App;

use std::path::PathBuf;

use std::error::Error;
use std::sync::Arc;

use ontape_core::{clock::SystemClock, session::DeckSession, store::{self, Store}};
use relm4::{RelmApp, gtk::glib::ExitCode};


pub const APP_ID: &str = "tech.hejduk.OnTape";

fn main() -> ExitCode {
    let session = match open_session() {
        Ok(session) => session,
        Err(e) => {
            eprintln!("ontape-gtk: {e}");
            return ExitCode::FAILURE;
        }
    };
    let app = RelmApp::new(APP_ID).with_args(Vec::new());
    relm4::set_global_css(include_str!("style.css"));
    app.run::<App>(session);
    ExitCode::SUCCESS
}

fn open_session() -> Result<DeckSession, Box<dyn Error>> {
    let db = match std::env::var_os("ONTAPE_DB").filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => store::default_db_path()?,
    };
    let store = Store::open_creating_dir(&db)?;
    Ok(DeckSession::open(store, 0, Arc::new(SystemClock))?)
}
