//! Decisions and wording behind the deck's buttons, kept free of GTK so they
//! can be tested.

use std::time::Duration;

use ontape_core::deck::{DeckEvent, DeckState};
use ontape_core::tape::{Direction, Side, Source, Tape};

/// What pressing ◀◀ or ▶▶ does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindAction {
    /// Fast-forward or rewind; pressing again stops.
    Wind,
    /// While playing: wind with the sound on until the button is let go.
    Cue,
}

pub fn wind_press(state: DeckState) -> WindAction {
    match state {
        DeckState::Playing | DeckState::Cueing(_) => WindAction::Cue,
        DeckState::Empty | DeckState::Stopped | DeckState::Winding(_) => WindAction::Wind,
    }
}

/// What a play/pause toggle (space bar, media key) does. A tape has no
/// pause, so "pause" stops it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Play,
    Stop,
}

pub fn play_pause(state: DeckState) -> Toggle {
    match state {
        DeckState::Playing | DeckState::Cueing(_) => Toggle::Stop,
        DeckState::Empty | DeckState::Stopped | DeckState::Winding(_) => Toggle::Play,
    }
}

pub fn describe(event: DeckEvent) -> String {
    match event {
        DeckEvent::EndOfSide(side) => format!("End of side {}. Flip the tape to carry on.", side_name(side)),
        DeckEvent::WindFinished(Direction::Forward) => "Fast-forwarded to the end".to_owned(),
        DeckEvent::WindFinished(Direction::Backward) => "Rewound to the start".to_owned(),
    }
}

pub fn side_name(side: Side) -> &'static str {
    match side {
        Side::A => "A",
        Side::B => "B",
    }
}

pub fn fmt_time(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Which song on the side (1, 2, …) is under the head at `position`.
pub fn song_number(tape: &Tape, side: Side, position: Duration) -> usize {
    tape.recording(side)
        .segments()
        .iter()
        .take_while(|seg| seg.start <= position)
        .filter(|seg| matches!(seg.source, Source::Track { .. }))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontape_core::tape::{LayoutOptions, TrackId, TrackInfo, layout_album};

    #[test]
    fn wind_buttons_cue_only_while_playing() {
        assert_eq!(wind_press(DeckState::Playing), WindAction::Cue);
        assert_eq!(wind_press(DeckState::Cueing(Direction::Forward)), WindAction::Cue);
        assert_eq!(wind_press(DeckState::Stopped), WindAction::Wind);
        assert_eq!(wind_press(DeckState::Winding(Direction::Backward)), WindAction::Wind);
        assert_eq!(wind_press(DeckState::Empty), WindAction::Wind);
    }

    #[test]
    fn play_pause_stops_a_moving_tape_and_plays_otherwise() {
        assert_eq!(play_pause(DeckState::Playing), Toggle::Stop);
        assert_eq!(play_pause(DeckState::Cueing(Direction::Backward)), Toggle::Stop);
        assert_eq!(play_pause(DeckState::Stopped), Toggle::Play);
        // Play during a wind plays from wherever the tape got to.
        assert_eq!(play_pause(DeckState::Winding(Direction::Forward)), Toggle::Play);
    }

    #[test]
    fn counts_songs_on_a_side() {
        let tracks = [
            TrackInfo { id: TrackId(1), len: Duration::from_secs(60) },
            TrackInfo { id: TrackId(2), len: Duration::from_secs(60) },
            TrackInfo { id: TrackId(3), len: Duration::from_secs(600) },
        ];
        let tape = layout_album(&tracks, &LayoutOptions::default()).unwrap().remove(0);
        // Leader, then song 1, then song 2 (the long song goes on side B).
        assert_eq!(song_number(&tape, Side::A, Duration::ZERO), 0);
        assert_eq!(song_number(&tape, Side::A, Duration::from_secs(30)), 1);
        assert_eq!(song_number(&tape, Side::A, Duration::from_secs(100)), 2);
    }

    #[test]
    fn formats_times() {
        assert_eq!(fmt_time(Duration::from_millis(754_900)), "12:34");
        assert_eq!(describe(DeckEvent::EndOfSide(Side::B)), "End of side B. Flip the tape to carry on.");
    }
}

