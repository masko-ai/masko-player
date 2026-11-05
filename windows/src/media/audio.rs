//! Sound, decoded whole into memory: a clip's own voice (AAC in the clip's
//! MP4) and edge sounds (`sound.url`: MP3 above all, also AAC in an .m4a
//! and WAV). They are short, so the mixer plays them from memory, loops
//! them without a gap and starts them at any point.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::PacketRef;
use symphonia::core::units::{Duration, Timestamp};
use symphonia::default::codecs::AacDecoder;

use super::mp4::{self, Entry, Kind, Track};
use super::video::ClipFile;
use super::MediaError;

/// Decoded sound: interleaved samples, -1 to 1.
#[derive(Clone)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Arc<[f32]>,
}

impl Pcm {
    /// How many frames (one sample per channel) it holds.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    /// How long it plays at rate 1.
    pub fn duration_ms(&self) -> u64 {
        (self.frames() as u64 * 1000) / self.sample_rate.max(1) as u64
    }
}

impl std::fmt::Debug for Pcm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Pcm({} Hz, {} channels, {} frames)",
            self.sample_rate,
            self.channels,
            self.frames()
        )
    }
}

/// The clip's own voice, None when the file has no sound track. Its first
/// sample is the sound heard with the clip's first frame: the encoder's
/// priming and the file's offset between sound and picture
/// (AV1Clip.soundStart) are already cut, so the voice of clip time `t` is at
/// `t` into it.
pub fn clip_audio(clip: &ClipFile) -> Result<Option<Pcm>, MediaError> {
    let Some((track, movie_timescale, sound_start)) = clip.audio_track() else {
        return Ok(None);
    };
    let pcm = decode_aac_track(clip.bytes(), track, movie_timescale)?;
    let cut = (sound_start * pcm.sample_rate as f64).round() as usize * pcm.channels as usize;
    if cut == 0 {
        return Ok(Some(pcm));
    }
    Ok(Some(Pcm {
        samples: pcm.samples[cut.min(pcm.samples.len())..].into(),
        ..pcm
    }))
}

/// An edge sound file: MP3, AAC in an MP4 (.m4a), or WAV, told apart by
/// their content.
pub fn sound_file(path: &Path) -> Result<Pcm, MediaError> {
    let bytes = std::fs::read(path)?;
    if mp4::looks_like_mp4(&bytes) {
        let movie = mp4::read(&bytes)?;
        let track = movie
            .first(Kind::Audio)
            .ok_or_else(|| MediaError::Format("the file has no sound track".into()))?;
        return decode_aac_track(&bytes, track, movie.timescale);
    }
    decode_with_probe(bytes, path)
}

fn symphonia_error(error: SymphoniaError) -> MediaError {
    match error {
        SymphoniaError::IoError(error) => MediaError::Io(error),
        SymphoniaError::Unsupported(what) => MediaError::Format(what.to_string()),
        other => MediaError::Decode(other.to_string()),
    }
}

/// Appends a decoded buffer to `out`, interleaved, and returns its rate and
/// channel count.
fn append(
    buffer: &GenericAudioBufferRef,
    scratch: &mut Vec<f32>,
    out: &mut Vec<f32>,
) -> (u32, u16) {
    buffer.copy_to_vec_interleaved::<f32>(scratch);
    out.extend_from_slice(scratch);
    let spec = buffer.spec();
    (spec.rate(), spec.channels().count() as u16)
}

/// Decodes an MP4's AAC track, and keeps what its edit list plays: without
/// the encoder's priming before it, nor the padding after.
fn decode_aac_track(bytes: &[u8], track: &Track, movie_timescale: u32) -> Result<Pcm, MediaError> {
    let Entry::Aac {
        sample_rate,
        config,
    } = &track.entry
    else {
        return Err(MediaError::Format("the sound track is not AAC".into()));
    };
    let mut params = AudioCodecParameters::new();
    params
        .for_codec(CODEC_ID_AAC)
        .with_extra_data(config.clone().into_boxed_slice());
    if *sample_rate > 0 {
        params.with_sample_rate(*sample_rate);
    }
    let mut decoder =
        AacDecoder::try_new(&params, &AudioDecoderOptions::default()).map_err(symphonia_error)?;
    let mut samples = Vec::new();
    let mut scratch = Vec::new();
    let (mut rate, mut channels) = (0u32, 0u16);
    for sample in &track.samples {
        let data = &bytes[sample.offset..sample.offset + sample.size];
        let packet = PacketRef::new(0, Timestamp::new(0), Duration::new(1024), data);
        match decoder.decode_ref(&packet) {
            Ok(buffer) => (rate, channels) = append(&buffer, &mut scratch, &mut samples),
            // A damaged packet is heard as silence, so the rest stays in
            // time with the picture.
            Err(SymphoniaError::DecodeError(_)) if channels > 0 => {
                samples.resize(samples.len() + 1024 * channels as usize, 0.0);
            }
            Err(error) => return Err(symphonia_error(error)),
        }
    }
    if channels == 0 || rate == 0 {
        return Err(MediaError::Decode("the sound track has no sound".into()));
    }
    let width = channels as usize;
    let total = samples.len() / width;
    let (first, last) = match track.played_edit(movie_timescale) {
        Some((media_time, seconds)) => {
            let first = (media_time as f64 * rate as f64 / track.timescale as f64).round() as usize;
            let length = (seconds * rate as f64).round() as usize;
            let first = first.min(total);
            (
                first,
                if length > 0 {
                    (first + length).min(total)
                } else {
                    total
                },
            )
        }
        None => (0, total),
    };
    samples.truncate(last * width);
    samples.drain(..first * width);
    Ok(Pcm {
        sample_rate: rate,
        channels,
        samples: samples.into(),
    })
}

/// MP3, WAV (and ADTS AAC): symphonia finds the format.
fn decode_with_probe(bytes: Vec<u8>, path: &Path) -> Result<Pcm, MediaError> {
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|_| {
            MediaError::Format("not a sound file the player decodes (MP3, M4A, WAV)".into())
        })?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| MediaError::Format("the file has no sound track".into()))?;
    let id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .cloned()
        .ok_or_else(|| MediaError::Format("the sound track has no codec".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(symphonia_error)?;
    let mut samples = Vec::new();
    let mut scratch = Vec::new();
    let (mut rate, mut channels) = (0u32, 0u16);
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(error) => return Err(symphonia_error(error)),
        };
        if packet.track_id != id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buffer) => (rate, channels) = append(&buffer, &mut scratch, &mut samples),
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(symphonia_error(error)),
        }
    }
    if channels == 0 || rate == 0 {
        return Err(MediaError::Decode("the sound file has no sound".into()));
    }
    Ok(Pcm {
        sample_rate: rate,
        channels,
        samples: samples.into(),
    })
}
