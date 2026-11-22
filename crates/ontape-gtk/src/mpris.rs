//! System media controls (MPRIS): the desktop's play/pause, next and
//! previous buttons and the keyboard media keys drive the deck.
//!
//! A tape has no pause and no track skip, so "pause" stops the tape, "next"
//! fast-forwards and "previous" rewinds. Pressing next or previous again
//! stops the wind, like the deck's own buttons.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use mpris_server::{Metadata, PlaybackStatus, Player};
use ontape_core::deck::DeckState;
use ontape_core::tape::Direction;
use relm4::Sender;
use relm4::gtk::glib;

use crate::APP_ID;
use crate::app::AppMsg;

/// What the media controls show.
#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub status: PlaybackStatus,
    /// The song under the head, if it has a name.
    pub title: Option<String>,
    /// The tape in the deck.
    pub album: Option<String>,
}

/// A running MPRIS server.
pub struct Mpris {
    player: Rc<Player>,
    shown: RefCell<Option<NowPlaying>>,
}

impl fmt::Debug for Mpris {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mpris").field("shown", &self.shown).finish_non_exhaustive()
    }
}

/// Where the app finds the server once it is running. Stays empty without a
/// session bus.
pub type MprisSlot = Rc<RefCell<Option<Mpris>>>;

/// Starts the MPRIS server on the GLib main loop and puts it in `slot` once
/// it is on the bus.
pub fn start(app: Sender<AppMsg>, slot: MprisSlot) {
    glib::MainContext::default().spawn_local(async move {
        let built = Player::builder("ontape")
            .identity("OnTape")
            .desktop_entry(APP_ID)
            .can_play(true)
            .can_pause(true)
            .can_go_next(true)
            .can_go_previous(true)
            .can_seek(false)
            .can_control(true)
            .build()
            .await;
        let player = match built {
            Ok(player) => Rc::new(player),
            Err(e) => {
                eprintln!("media controls unavailable: {e}");
                return;
            }
        };

        let send = |msg: fn() -> AppMsg| {
            let app = app.clone();
            move |_: &Player| app.emit(msg())
        };
        player.connect_play(send(|| AppMsg::Play));
        player.connect_pause(send(|| AppMsg::Stop));
        player.connect_stop(send(|| AppMsg::Stop));
        player.connect_play_pause(send(|| AppMsg::PlayPause));
        player.connect_next(send(|| AppMsg::Wind(Direction::Forward)));
        player.connect_previous(send(|| AppMsg::Wind(Direction::Backward)));

        glib::MainContext::default().spawn_local(player.run());
        *slot.borrow_mut() = Some(Mpris { player, shown: RefCell::new(None) });
        // Show the deck's current state right away.
        app.emit(AppMsg::Tick);
    });
}

impl Mpris {
    /// Tells the desktop what the deck is doing, if that changed.
    pub fn show(&self, now: NowPlaying) {
        if self.shown.borrow().as_ref() == Some(&now) {
            return;
        }
        *self.shown.borrow_mut() = Some(now.clone());
        let player = self.player.clone();
        glib::MainContext::default().spawn_local(async move {
            let mut metadata = Metadata::builder();
            if let Some(title) = now.title {
                metadata = metadata.title(title);
            }
            if let Some(album) = now.album {
                metadata = metadata.album(album);
            }
            let _ = player.set_metadata(metadata.build()).await;
            let _ = player.set_playback_status(now.status).await;
        });
    }
}

/// The media-controls status for a deck state. A stopped tape counts as
/// paused (it resumes where it stopped); an empty deck is stopped.
pub fn playback_status(state: DeckState) -> PlaybackStatus {
    match state {
        DeckState::Playing | DeckState::Cueing(_) => PlaybackStatus::Playing,
        DeckState::Stopped | DeckState::Winding(_) => PlaybackStatus::Paused,
        DeckState::Empty => PlaybackStatus::Stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_states_map_to_media_status() {
        assert_eq!(playback_status(DeckState::Playing), PlaybackStatus::Playing);
        assert_eq!(playback_status(DeckState::Cueing(Direction::Forward)), PlaybackStatus::Playing);
        assert_eq!(playback_status(DeckState::Stopped), PlaybackStatus::Paused);
        assert_eq!(playback_status(DeckState::Winding(Direction::Backward)), PlaybackStatus::Paused);
        assert_eq!(playback_status(DeckState::Empty), PlaybackStatus::Stopped);
    }
}

