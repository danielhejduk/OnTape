use symphonia::core::{codecs::audio::AudioDecoderOptions, formats::{FormatOptions, probe::Hint, TrackType}, io::MediaSourceStream, meta::MetadataOptions};

pub struct Player {
    playing: bool
}

pub fn load(path: &str) {
    let src = std::fs::File::open(path).expect("failed to open media");
    let mss = MediaSourceStream::new(Box::new(src), Default::default());

    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();

    let mut hint = Hint::new();
    hint.with_extension("mp3");

    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, fmt_opts, meta_opts)
        .expect("unsupported format");

    let track = format.default_track(TrackType::Audio).expect("no audio track");
    let dec_opts: AudioDecoderOptions = Default::default();

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(track.codec_params.as_ref().expect("codec parameters missing").audio().unwrap(),
        &dec_opts,
    )
            .expect("unsupported codec");
    let track_id = track.id;
}
