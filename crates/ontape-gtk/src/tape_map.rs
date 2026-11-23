//! What the progress bar under the cassette shows: both sides of the tape
//! laid end to end (A on the left half, B on the right), split into songs and
//! silence. Kept free of GTK so it can be tested.

use std::time::Duration;

use ontape_core::tape::{Side, Source, Tape, TrackId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Song {
    /// 1, 2, … on its side, counted the way the song line counts them.
    pub number: usize,
    pub track: TrackId,
    pub title: Option<String>,
}

/// A stretch of one side: a song, or silence (leader, gap, blank tape).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: Duration,
    pub len: Duration,
    pub song: Option<Song>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TapeMap {
    side_len: Duration,
    sides: [Vec<Span>; 2],
}

impl TapeMap {
    pub fn new(tape: &Tape, title: impl Fn(TrackId) -> Option<String>) -> TapeMap {
        let side = |side: Side| {
            let mut number = 0;
            tape.recording(side)
                .segments()
                .iter()
                .map(|seg| Span {
                    start: seg.start,
                    len: seg.len,
                    song: match seg.source {
                        Source::Silence => None,
                        Source::Track { track, .. } => {
                            number += 1;
                            Some(Song {
                                number,
                                track,
                                title: title(track),
                            })
                        }
                    },
                })
                .collect()
        };
        TapeMap {
            side_len: tape.side_len(),
            sides: [side(Side::A), side(Side::B)],
        }
    }

    pub fn spans(&self, side: Side) -> &[Span] {
        &self.sides[index(side)]
    }

    /// Where `position` on `side` falls along the whole bar, from 0 to 1.
    pub fn fraction(&self, side: Side, position: Duration) -> f64 {
        let half = if self.side_len.is_zero() {
            0.0
        } else {
            (position.as_secs_f64() / self.side_len.as_secs_f64()).clamp(0.0, 1.0) / 2.0
        };
        match side {
            Side::A => half,
            Side::B => 0.5 + half,
        }
    }

    /// The side and the stretch of tape at `fraction` along the bar.
    pub fn span_at(&self, fraction: f64) -> Option<(Side, &Span)> {
        if !(0.0..1.0).contains(&fraction) {
            return None;
        }
        let side = if fraction < 0.5 { Side::A } else { Side::B };
        let along = (fraction - self.fraction(side, Duration::ZERO)) * 2.0;
        let position = self.side_len.mul_f64(along);
        let spans = self.spans(side);
        let i = spans.partition_point(|s| s.start + s.len <= position);
        spans.get(i).map(|span| (side, span))
    }
}

fn index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::song_number;
    use ontape_core::tape::{Cassette, LayoutOptions, TrackInfo, layout_album};

    fn tape() -> Tape {
        let tracks = [(1, 60), (2, 60), (3, 600)].map(|(id, secs)| TrackInfo {
            id: TrackId(id),
            len: Duration::from_secs(secs),
        });
        layout_album(&tracks, &LayoutOptions::default())
            .unwrap()
            .remove(0)
    }

    fn map() -> TapeMap {
        TapeMap::new(&tape(), |id| Some(format!("Track {}", id.0)))
    }

    fn song_starts(map: &TapeMap, side: Side) -> Vec<(usize, u64)> {
        map.spans(side)
            .iter()
            .filter_map(|s| s.song.as_ref().map(|song| (song.number, s.start.as_secs())))
            .collect()
    }

    #[test]
    fn marks_where_songs_start() {
        let map = map();
        // Leader of 4 s, song 1, a 3 s gap, song 2.
        assert_eq!(song_starts(&map, Side::A), [(1, 4), (2, 67)]);
        assert_eq!(song_starts(&map, Side::B), [(1, 4)]);
        let last = map.spans(Side::A).last().unwrap();
        assert_eq!(last.song, None, "the rest of side A is blank");
        assert_eq!(
            map.spans(Side::B)[1]
                .song
                .as_ref()
                .unwrap()
                .title
                .as_deref(),
            Some("Track 3")
        );
    }

    #[test]
    fn lays_side_b_after_side_a() {
        let map = map();
        let len = tape().side_len();
        assert_eq!(map.fraction(Side::A, Duration::ZERO), 0.0);
        assert_eq!(map.fraction(Side::A, len), 0.5);
        assert_eq!(map.fraction(Side::B, Duration::ZERO), 0.5);
        assert_eq!(map.fraction(Side::B, len), 1.0);
        assert_eq!(map.fraction(Side::A, len / 2), 0.25);
    }

    #[test]
    fn finds_the_song_under_the_pointer() {
        let map = map();
        let at = |side, secs| map.fraction(side, Duration::from_secs(secs));
        let song = |f| {
            map.span_at(f)
                .map(|(side, s)| (side, s.song.as_ref().map(|song| song.number)))
        };
        assert_eq!(song(at(Side::A, 2)), Some((Side::A, None)));
        assert_eq!(song(at(Side::A, 30)), Some((Side::A, Some(1))));
        assert_eq!(song(at(Side::A, 65)), Some((Side::A, None)));
        assert_eq!(song(at(Side::A, 100)), Some((Side::A, Some(2))));
        assert_eq!(song(at(Side::B, 100)), Some((Side::B, Some(1))));
        assert_eq!(song(1.0), None);
        assert_eq!(song(-0.1), None);
    }

    #[test]
    fn agrees_with_the_song_line() {
        let (tape, map) = (tape(), map());
        for side in [Side::A, Side::B] {
            for span in map.spans(side) {
                if let Some(song) = &span.song {
                    let mid = span.start + span.len / 2;
                    assert_eq!(song.number, song_number(&tape, side, mid));
                }
            }
        }
    }

    #[test]
    fn a_blank_tape_has_no_songs() {
        let map = TapeMap::new(&Tape::blank(Cassette::longest()), |_| None);
        for side in [Side::A, Side::B] {
            assert!(map.spans(side).iter().all(|s| s.song.is_none()));
        }
    }
}
