use super::{TapeError, TrackId};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Silence,
    Track { track: TrackId, offset: Duration },
}

impl Source {
    pub fn advanced(self, by: Duration) -> Source {
        match self {
            Source::Silence => Source::Silence,
            Source::Track { track, offset } => Source::Track {
                track,
                offset: offset + by,
            },
        }
    }

    fn continues_with(self, len: Duration, next: Source) -> bool {
        match (self, next) {
            (Source::Silence, Source::Silence) => true,
            (
                Source::Track {
                    track: a,
                    offset: o,
                },
                Source::Track {
                    track: b,
                    offset: p,
                },
            ) => a == b && o + len == p,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: Duration,
    pub len: Duration,
    pub source: Source,
}

impl Segment {
    pub fn end(&self) -> Duration {
        self.start + self.len
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recording {
    len: Duration,
    segments: Vec<Segment>,
}

impl Recording {
    pub fn blank(len: Duration) -> Recording {
        let segments = if len.is_zero() {
            Vec::new()
        } else {
            vec![Segment {
                start: Duration::ZERO,
                len,
                source: Source::Silence,
            }]
        };
        Recording { len, segments }
    }

    pub fn from_pieces(
        len: Duration,
        pieces: impl IntoIterator<Item = (Duration, Source)>,
    ) -> Recording {
        let mut recording = Recording::blank(len);
        let mut at = Duration::ZERO;
        for (piece_len, source) in pieces {
            if at >= len {
                break;
            }
            at += recording.write(at, piece_len, source);
        }
        recording
    }

    /// Rebuilds a recording from back-to-back pieces that must cover `len` exactly.
    pub fn try_from_pieces(
        len: Duration,
        pieces: impl IntoIterator<Item = (Duration, Source)>,
    ) -> Result<Recording, TapeError> {
        let mut segments = Vec::new();
        let mut at = Duration::ZERO;
        for (piece_len, source) in pieces {
            if piece_len.is_zero() {
                return Err(TapeError::Invalid("empty segment"));
            }
            segments.push(Segment {
                start: at,
                len: piece_len,
                source,
            });
            at += piece_len;
        }
        if at != len {
            return Err(TapeError::Invalid("segments do not cover the side exactly"));
        }
        let mut recording = Recording { len, segments };
        recording.normalize();
        Ok(recording)
    }

    pub fn len(&self) -> Duration {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len.is_zero()
    }

    pub fn is_blank(&self) -> bool {
        self.segments.iter().all(|s| s.source == Source::Silence)
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn segment_index_at(&self, pos: Duration) -> Option<usize> {
        if pos >= self.len {
            return None;
        }
        Some(self.segments.partition_point(|s| s.end() <= pos))
    }

    pub fn segment_at(&self, pos: Duration) -> Option<&Segment> {
        self.segment_index_at(pos).map(|i| &self.segments[i])
    }

    pub fn source_at(&self, pos: Duration) -> Option<Source> {
        self.segment_at(pos)
            .map(|s| s.source.advanced(pos - s.start))
    }

    pub fn write(&mut self, at: Duration, len: Duration, source: Source) -> Duration {
        if at >= self.len || len.is_zero() {
            return Duration::ZERO;
        }
        let end = (at + len).min(self.len);
        let first = self.split_at(at);
        let past = self.split_at(end);

        self.segments.splice(
            first..past,
            [Segment {
                start: at,
                len: end - at,
                source,
            }],
        );

        self.normalize();
        end - at
    }

    pub fn erase(&mut self, at: Duration, len: Duration) -> Duration {
        self.write(at, len, Source::Silence)
    }

    fn split_at(&mut self, pos: Duration) -> usize {
        let Some(i) = self.segment_index_at(pos) else {
            return self.segments.len();
        };
        let seg = self.segments[i];
        if seg.start == pos {
            return i;
        }
        let head_len = pos - seg.start;
        self.segments[i].len = head_len;
        self.segments.insert(
            i + 1,
            Segment {
                start: pos,
                len: seg.len - head_len,
                source: seg.source.advanced(head_len),
            },
        );
        i + 1
    }

    fn normalize(&mut self) {
        let mut merged: Vec<Segment> = Vec::with_capacity(self.segments.len());
        for seg in self.segments.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.source.continues_with(last.len, seg.source)
            {
                last.len += seg.len;
            } else {
                merged.push(seg);
            }
        }
        self.segments = merged;
    }
}
