use std::fmt;
use std::time::Duration;

use super::{Cassette, Recording, Source, Tape, TrackId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    pub id: TrackId,
    pub len: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutOptions {
    pub leader: Duration,
    pub gap: Duration,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        LayoutOptions {
            leader: Duration::from_secs(4),
            gap: Duration::from_secs(3),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    NoTracks,
    TrackTooLong {
        track: TrackId,
        len: Duration,
        max: Duration,
    },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutError::NoTracks => f.write_str("album has no tracks"),
            LayoutError::TrackTooLong { track, len, max } => write!(
                f,
                "track {} is {} s long; a side holds at most {} s",
                track.0,
                len.as_secs(),
                max.as_secs()
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

pub fn layout_album(tracks: &[TrackInfo], opts: &LayoutOptions) -> Result<Vec<Tape>, LayoutError> {
    if tracks.is_empty() {
        return Err(LayoutError::NoTracks);
    }
    let capacity = Cassette::longest().side_len();
    let sides = SideLengths::new(tracks, opts);

    for (i, t) in tracks.iter().enumerate() {
        if sides.len(i, i + 1) > capacity {
            return Err(LayoutError::TrackTooLong {
                track: t.id,
                len: t.len,
                max: capacity.saturating_sub(opts.leader),
            });
        }
    }
    let tape_count = sides.min_sides(capacity).div_ceil(2);
    let bounds = sides.balanced_split(tape_count * 2, capacity);

    let tapes = bounds
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[a, b]| {
            let (a, b) = (a.clone(), b.clone());
            let longer = sides.len(a.start, a.end).max(sides.len(b.start, b.end));
            let cassette = Cassette::smallest_fitting(longer).expect("sides are within capacity");
            Tape::album(
                cassette,
                side_recording(&tracks[a], cassette, opts),
                side_recording(&tracks[b], cassette, opts),
            )
        })
        .collect();
    Ok(tapes)
}

fn side_recording(tracks: &[TrackInfo], cassette: Cassette, opts: &LayoutOptions) -> Recording {
    let len = cassette.side_len();
    if tracks.is_empty() {
        return Recording::blank(len);
    }
    let mut pieces = vec![(opts.leader, Source::Silence)];
    for (i, t) in tracks.iter().enumerate() {
        if i > 0 {
            pieces.push((opts.gap, Source::Silence));
        }
        pieces.push((
            t.len,
            Source::Track {
                track: t.id,
                offset: Duration::ZERO,
            },
        ));
    }
    Recording::from_pieces(len, pieces)
}

struct SideLengths {
    prefix: Vec<Duration>,
    opts: LayoutOptions,
}

impl SideLengths {
    fn new(tracks: &[TrackInfo], opts: &LayoutOptions) -> Self {
        let mut prefix = Vec::with_capacity(tracks.len() + 1);
        let mut total = Duration::ZERO;
        prefix.push(total);
        for t in tracks {
            total += t.len;
            prefix.push(total);
        }
        SideLengths {
            prefix,
            opts: *opts,
        }
    }

    fn track_count(&self) -> usize {
        self.prefix.len() - 1
    }

    fn len(&self, start: usize, end: usize) -> Duration {
        if start == end {
            return Duration::ZERO;
        }
        let gaps = u32::try_from(end - start - 1).expect("track count fits in u32");
        self.opts.leader + (self.prefix[end] - self.prefix[start]) + self.opts.gap * gaps
    }

    fn min_sides(&self, capacity: Duration) -> usize {
        let n = self.track_count();
        let mut count = 0;
        let mut start = 0;
        while start < n {
            let mut end = start + 1;
            while end < n && self.len(start, end + 1) <= capacity {
                end += 1;
            }
            count += 1;
            start = end;
        }
        count
    }

    fn balanced_split(&self, parts: usize, capacity: Duration) -> Vec<std::ops::Range<usize>> {
        let n = self.track_count();
        let cost = |d: Duration| {
            let ms = d.as_millis();
            ms * ms
        };

        let mut best: Vec<Vec<Option<u128>>> = vec![vec![None; n + 1]; parts + 1];
        let mut from = vec![vec![0usize; n + 1]; parts + 1];
        best[0][0] = Some(0);
        for p in 1..=parts {
            for j in 0..=n {
                for i in 0..=j {
                    let Some(before) = best[p - 1][i] else {
                        continue;
                    };
                    let side = self.len(i, j);
                    if side > capacity {
                        continue;
                    }
                    let total = before + cost(side);
                    // `<=` picks the latest start on ties: the last side gets
                    // the least.
                    if best[p][j].is_none_or(|b| total <= b) {
                        best[p][j] = Some(total);
                        from[p][j] = i;
                    }
                }
            }
        }
        assert!(
            best[parts][n].is_some(),
            "{parts} sides cannot hold the album"
        );

        let mut bounds = Vec::with_capacity(parts);
        let mut end = n;
        for p in (1..=parts).rev() {
            let start = from[p][end];
            bounds.push(start..end);
            end = start;
        }
        bounds.reverse();
        bounds
    }
}
