//! Decoding audio files into a rodio `Source`, using Symphonia plus libopus
//! (Symphonia has no Opus decoder of its own).

use std::fmt;
use std::fs::File;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, SampleRate, Source};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

#[derive(Debug)]
pub enum AudioError {
    Open(PathBuf, std::io::Error),
    Decode(PathBuf, SymphoniaError),
    NoAudio(PathBuf),
    UnknownLength(PathBuf),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Open(p, e) => write!(f, "{}: {e}", p.display()),
            AudioError::Decode(p, e) => write!(f, "{}: {e}", p.display()),
            AudioError::NoAudio(p) => write!(f, "{}: no playable audio track", p.display()),
            AudioError::UnknownLength(p) => write!(f, "{}: cannot tell how long it is", p.display()),
        }
    }
}

impl std::error::Error for AudioError {}

fn codecs() -> &'static CodecRegistry {
    static CODECS: OnceLock<CodecRegistry> = OnceLock::new();
    CODECS.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
        registry
    })
}

fn to_std(time: Time) -> Duration {
    Duration::from_secs_f64(time.as_secs_f64().max(0.0))
}

/// An audio file decoded on the fly, as interleaved `f32` samples.
pub struct FileSource {
    path: PathBuf,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    len: Option<Duration>,
    rate: SampleRate,
    channels: ChannelCount,
    buf: Vec<f32>,
    pos: usize,
    /// Interleaved samples still to drop after an accurate seek landed early.
    skip: usize,
}

impl FileSource {
    pub fn open(path: &Path) -> Result<FileSource, AudioError> {
        let err = |e| AudioError::Decode(path.to_owned(), e);
        let file = File::open(path).map_err(|e| AudioError::Open(path.to_owned(), e))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(err)?;
        let track = format.default_track(TrackType::Audio).ok_or_else(|| AudioError::NoAudio(path.to_owned()))?;
        let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| AudioError::NoAudio(path.to_owned()))?;
        let decoder = codecs().make_audio_decoder(params, &AudioDecoderOptions::default()).map_err(err)?;

        let len = match (track.time_base, track.duration) {
            (Some(tb), Some(dur)) => tb.calc_duration(dur).map(to_std),
            _ => None,
        }
        .or_else(|| {
            let frames = track.num_frames?;
            let rate = params.sample_rate?;
            Some(Duration::from_secs_f64(frames as f64 / rate as f64))
        });

        let mut source = FileSource {
            path: path.to_owned(),
            track_id: track.id,
            time_base: track.time_base,
            format,
            decoder,
            len,
            rate: NonZero::new(44_100).unwrap(),
            channels: NonZero::new(2).unwrap(),
            buf: Vec::new(),
            pos: 0,
            skip: 0,
        };
        // Decode the first packet up front: it tells the real output rate and channel count.
        if !source.refill() {
            return Err(AudioError::NoAudio(path.to_owned()));
        }
        Ok(source)
    }

    pub fn len(&self) -> Result<Duration, AudioError> {
        self.len.ok_or_else(|| AudioError::UnknownLength(self.path.clone()))
    }

    /// Decodes the next packet into `buf`. Returns `false` at the end of the stream.
    fn refill(&mut self) -> bool {
        self.buf.clear();
        self.pos = 0;
        while self.buf.is_empty() {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return false,
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(_) => return false,
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) => continue, // a damaged packet: skip it
                Err(_) => return false,
            };
            let spec = decoded.spec();
            if let (Some(rate), Some(channels)) =
                (NonZero::new(spec.rate()), NonZero::new(spec.channels().count() as u16))
            {
                self.rate = rate;
                self.channels = channels;
            }
            self.buf.resize(decoded.samples_interleaved(), 0.0);
            decoded.copy_to_slice_interleaved(&mut self.buf);

            let dropped = self.skip.min(self.buf.len());
            self.buf.drain(..dropped);
            self.skip -= dropped;
        }
        true
    }

    fn time_of(&self, ts: Timestamp) -> Option<Duration> {
        self.time_base?.calc_time(ts).map(to_std)
    }
}

impl Iterator for FileSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buf.len() && !self.refill() {
            return None;
        }
        let sample = self.buf[self.pos];
        self.pos += 1;
        Some(sample)
    }
}

impl Source for FileSource {
    fn current_span_len(&self) -> Option<usize> {
        // Rate and channel count are fixed for the whole file.
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.len
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let time = Time::try_from_secs_f64(pos.as_secs_f64())
            .ok_or_else(|| SeekError::Other(Arc::new(std::io::Error::other("seek position out of range"))))?;
        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track_id) })
            .map_err(|e| SeekError::Other(Arc::new(e)))?;
        self.decoder.reset();
        self.buf.clear();
        self.pos = 0;
        // The reader lands on a packet at or before the target; drop the samples in between.
        self.skip = match (self.time_of(seeked.required_ts), self.time_of(seeked.actual_ts)) {
            (Some(want), Some(got)) if want > got => {
                (want - got).as_secs_f64().mul_add(self.rate.get() as f64, 0.5) as usize * self.channels.get() as usize
            }
            _ => 0,
        };
        Ok(())
    }
}
