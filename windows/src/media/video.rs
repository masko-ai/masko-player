//! Stacked AV1 clips: the file read once into memory (AV1Slot.swift,
//! `AV1Clip`), and a decoder that turns its frames into transparent
//! pictures in order, with dav1d.

use std::ffi::{c_int, c_void};
use std::path::Path;
use std::sync::Arc;

use super::compose::{Colour, Picture, Plane, Stacked};
use super::dav1d::*;
use super::mp4::{self, Entry, Kind, Track};
use super::{check_size, Frame, MediaError};

/// What a clip is, read from its file without decoding it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipInfo {
    /// How long the clip plays: its video track's length (its edit list,
    /// else its media), as the Mac reads it. A file whose length cannot be
    /// read gets its last frame's time plus 1/24 s.
    pub duration_ms: u64,
    pub frame_count: usize,
    /// The file has a sound track: the clip's own voice.
    pub has_audio: bool,
    /// The visible picture: the coded width and half the coded height.
    pub width: u32,
    pub height: u32,
    /// When each frame shows, in ms from the first one, in the order the
    /// decoder gives them (AV1 in MP4 shows its frames in decoding order).
    pub frame_times_ms: Vec<u64>,
}

impl ClipInfo {
    /// The last frame due at `time_ms` into the clip (AV1Clip.frameIndex):
    /// 0 before the first, the last one after the end.
    pub fn frame_at(&self, time_ms: u64) -> usize {
        self.frame_times_ms
            .partition_point(|&t| t <= time_ms)
            .saturating_sub(1)
    }
}

/// A parsed stacked AV1 MP4, cheap to clone, shared between threads.
#[derive(Clone)]
pub struct ClipFile {
    inner: Arc<Inner>,
}

struct Inner {
    info: ClipInfo,
    bytes: Box<[u8]>,
    /// Each video sample's bytes in `bytes`.
    frames: Vec<(usize, usize)>,
    audio: Option<Track>,
    movie_timescale: u32,
    /// Seconds into the sound when the first frame shows (AV1Clip.soundStart).
    sound_start: f64,
}

impl ClipFile {
    /// Reads the file and its tables. Fails when it is not an MP4 whose
    /// video track is AV1.
    pub fn open(path: &Path) -> Result<ClipFile, MediaError> {
        ClipFile::from_bytes(std::fs::read(path)?.into_boxed_slice())
    }

    /// The same, from the file's bytes.
    pub fn from_bytes(bytes: Box<[u8]>) -> Result<ClipFile, MediaError> {
        let movie = mp4::read(&bytes)?;
        let video = movie
            .first(Kind::Video)
            .ok_or_else(|| MediaError::Format("the file has no video track".into()))?;
        let Entry::Av1 { width, height } = video.entry else {
            return Err(MediaError::Format("the video track is not AV1".into()));
        };
        if video.samples.is_empty() {
            return Err(MediaError::Format("the video track has no frames".into()));
        }
        let timescale = video.timescale as f64;
        let first = video.samples[0].pts();
        let frame_times_ms: Vec<u64> = video
            .samples
            .iter()
            .map(|s| (((s.pts() - first).max(0) as f64) * 1000.0 / timescale).round() as u64)
            .collect();
        let mut seconds = video.duration_seconds(movie.timescale);
        if !seconds.is_finite() || seconds <= 0.0 {
            seconds = *frame_times_ms.last().unwrap() as f64 / 1000.0 + 1.0 / 24.0;
        }
        let audio = movie.first(Kind::Audio).cloned();
        // When the picture and the sound start on the movie's timeline: the
        // sound keeps the file's own offset to the picture.
        let video_media_start = video.played_edit(movie.timescale).map_or(0, |(t, _)| t);
        let video_start =
            video.start_seconds(movie.timescale) + (first - video_media_start) as f64 / timescale;
        let sound_start = audio.as_ref().map_or(0.0, |a| {
            (video_start - a.start_seconds(movie.timescale)).max(0.0)
        });
        let info = ClipInfo {
            duration_ms: (seconds * 1000.0).round() as u64,
            frame_count: video.samples.len(),
            has_audio: audio.is_some(),
            width,
            height: height / 2,
            frame_times_ms,
        };
        let frames = video.samples.iter().map(|s| (s.offset, s.size)).collect();
        Ok(ClipFile {
            inner: Arc::new(Inner {
                info,
                frames,
                audio,
                movie_timescale: movie.timescale,
                sound_start: if sound_start.is_finite() {
                    sound_start
                } else {
                    0.0
                },
                bytes,
            }),
        })
    }

    pub fn info(&self) -> &ClipInfo {
        &self.inner.info
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.inner.bytes
    }

    pub(super) fn audio_track(&self) -> Option<(&Track, u32, f64)> {
        let inner = &self.inner;
        inner
            .audio
            .as_ref()
            .map(|track| (track, inner.movie_timescale, inner.sound_start))
    }
}

/// How many threads each decoder gives dav1d: enough to keep a 720x1440
/// clip at 30 frames a second on a modest PC, few enough that two slots
/// decoding at once leave the rest of the computer alone.
fn threads() -> c_int {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 4))
        .unwrap_or(2) as c_int
}

/// Decodes one clip's frames in order and composes each picture at the
/// output size. It holds the clip's bytes, so dav1d reads them in place.
pub struct VideoDecoder {
    clip: ClipFile,
    context: *mut Dav1dContext,
    /// The frame being sent to dav1d, until it takes all of it.
    data: Dav1dData,
    next_frame: usize,
    /// The last call into dav1d asked for a picture: asking again drains
    /// the frames still being decoded on its threads.
    asked: bool,
    out_width: u32,
    out_height: u32,
    stacked: Stacked,
}

// SAFETY: a dav1d context may be used from any thread, one at a time, which
// `&mut self` guarantees; the data it reads belongs to `clip`.
unsafe impl Send for VideoDecoder {}

/// dav1d's data wraps the clip's bytes; they live as long as the decoder.
unsafe extern "C" fn keep(_buf: *const u8, _cookie: *mut c_void) {}

/// A picture dav1d gave, given back when dropped.
struct Decoded(Dav1dPicture);

impl Drop for Decoded {
    fn drop(&mut self) {
        // SAFETY: the picture came from dav1d_get_picture.
        unsafe { dav1d_picture_unref(&mut self.0) };
    }
}

fn dav1d_error(what: &str, code: c_int) -> MediaError {
    MediaError::Decode(format!("dav1d {what} failed ({code})"))
}

impl VideoDecoder {
    pub fn new(
        clip: &ClipFile,
        out_width: u32,
        out_height: u32,
    ) -> Result<VideoDecoder, MediaError> {
        check_size(out_width, out_height)?;
        // SAFETY: dav1d fills the settings, and opens a context from them.
        let context = unsafe {
            let mut settings: Dav1dSettings = std::mem::zeroed();
            dav1d_default_settings(&mut settings);
            settings.n_threads = threads();
            let mut context = std::ptr::null_mut();
            let result = dav1d_open(&mut context, &settings);
            if result < 0 || context.is_null() {
                return Err(dav1d_error("open", result));
            }
            context
        };
        Ok(VideoDecoder {
            clip: clip.clone(),
            context,
            data: Dav1dData::empty(),
            next_frame: 0,
            asked: false,
            out_width,
            out_height,
            stacked: Stacked::new(),
        })
    }

    /// Composes the next picture into `into` (resized to the output size if
    /// needed) and returns its presentation time in ms from the clip start;
    /// None after the last picture.
    pub fn next(&mut self, into: &mut Frame) -> Result<Option<u64>, MediaError> {
        let Some(picture) = self.decode()? else {
            return Ok(None);
        };
        into.resize(self.out_width, self.out_height);
        self.compose(&picture.0, into)?;
        Ok(Some(self.time_of(&picture.0)))
    }

    /// Decodes the next picture without composing it, for a player running
    /// late that skips ahead (AV1Slot shows the latest decoded frame that is
    /// due): returns its time, None after the last picture.
    pub fn skip(&mut self) -> Result<Option<u64>, MediaError> {
        Ok(self.decode()?.map(|picture| self.time_of(&picture.0)))
    }

    /// Back to the first picture, for a loop restart, without reopening the
    /// file.
    pub fn rewind(&mut self) -> Result<(), MediaError> {
        // SAFETY: the context is open; the data is ours or empty.
        unsafe {
            if !self.data.data.is_null() {
                dav1d_data_unref(&mut self.data);
            }
            dav1d_flush(self.context);
        }
        self.data = Dav1dData::empty();
        self.next_frame = 0;
        self.asked = false;
        Ok(())
    }

    /// The output size the pictures are composed at.
    pub fn out_size(&self) -> (u32, u32) {
        (self.out_width, self.out_height)
    }

    /// Composes at another size from the next picture on, for a window that
    /// was resized.
    pub fn set_out_size(&mut self, out_width: u32, out_height: u32) -> Result<(), MediaError> {
        check_size(out_width, out_height)?;
        self.out_width = out_width;
        self.out_height = out_height;
        Ok(())
    }

    fn time_of(&self, picture: &Dav1dPicture) -> u64 {
        let times = &self.clip.info().frame_times_ms;
        usize::try_from(picture.m.timestamp)
            .ok()
            .and_then(|i| times.get(i).copied())
            .unwrap_or(0)
    }

    /// Feeds frames to dav1d until it gives a picture.
    fn decode(&mut self) -> Result<Option<Decoded>, MediaError> {
        let frames = &self.clip.inner.frames;
        let bytes = self.clip.bytes();
        loop {
            if self.data.sz == 0 && self.next_frame < frames.len() {
                let (offset, size) = frames[self.next_frame];
                // SAFETY: the range lies in the clip's bytes (mp4.rs checks
                // it), which outlive the context (see Drop).
                let result = unsafe {
                    dav1d_data_wrap(
                        &mut self.data,
                        bytes.as_ptr().add(offset),
                        size,
                        keep,
                        std::ptr::null_mut(),
                    )
                };
                if result < 0 {
                    return Err(dav1d_error("data", result));
                }
                // The picture made from this frame carries its index.
                self.data.m.timestamp = self.next_frame as i64;
                self.next_frame += 1;
            }
            if self.data.sz > 0 {
                // SAFETY: the context is open and the data valid.
                let result = unsafe { dav1d_send_data(self.context, &mut self.data) };
                self.asked = false;
                if result < 0 && result != DAV1D_EAGAIN {
                    unsafe { dav1d_data_unref(&mut self.data) };
                    self.data = Dav1dData::empty();
                    return Err(dav1d_error("decoding", result));
                }
            }
            let draining = self.asked;
            let mut picture = Dav1dPicture::zeroed();
            // SAFETY: as above; on success the picture is ours to unref.
            let result = unsafe { dav1d_get_picture(self.context, &mut picture) };
            self.asked = true;
            if result == 0 {
                return Ok(Some(Decoded(picture)));
            }
            if result != DAV1D_EAGAIN {
                return Err(dav1d_error("decoding", result));
            }
            let sent_all = self.data.sz == 0 && self.next_frame >= frames.len();
            if sent_all && draining {
                return Ok(None);
            }
        }
    }

    fn compose(&mut self, picture: &Dav1dPicture, into: &mut Frame) -> Result<(), MediaError> {
        let p = picture.p;
        if p.bpc != 8 {
            return Err(MediaError::Format(format!(
                "{}-bit pictures are not supported",
                p.bpc
            )));
        }
        let (ss_x, ss_y, chroma) = match p.layout {
            DAV1D_PIXEL_LAYOUT_I420 => (1, 1, true),
            DAV1D_PIXEL_LAYOUT_I422 => (1, 0, true),
            DAV1D_PIXEL_LAYOUT_I444 => (0, 0, true),
            DAV1D_PIXEL_LAYOUT_I400 => (0, 0, false),
            other => return Err(MediaError::Format(format!("pixel layout {other}"))),
        };
        let (w, h) = (p.w.max(0) as usize, p.h.max(0) as usize);
        let (luma_stride, chroma_stride) = (picture.stride[0], picture.stride[1]);
        if w == 0 || h < 2 || luma_stride < w as isize || picture.data[0].is_null() {
            return Err(MediaError::Decode("dav1d gave an empty picture".into()));
        }
        let plane = |data: *mut c_void, stride: isize, width: usize, height: usize| {
            // SAFETY: dav1d's planes hold `height` rows of `width` bytes,
            // `stride` apart; the picture outlives these slices.
            Plane {
                data: unsafe {
                    std::slice::from_raw_parts(
                        data as *const u8,
                        (height - 1) * stride as usize + width,
                    )
                },
                stride: stride as usize,
                width,
                height,
            }
        };
        let luma = plane(picture.data[0], luma_stride, w, h);
        let chroma = if chroma {
            let (cw, ch) = ((w + ss_x) >> ss_x, (h + ss_y) >> ss_y);
            if chroma_stride < cw as isize || picture.data[1].is_null() || picture.data[2].is_null()
            {
                return Err(MediaError::Decode(
                    "dav1d gave a picture without chroma".into(),
                ));
            }
            Some((
                plane(picture.data[1], chroma_stride, cw, ch),
                plane(picture.data[2], chroma_stride, cw, ch),
            ))
        } else {
            None
        };
        // SAFETY: a picture's sequence header lives as long as the picture.
        let colour = match unsafe { picture.seq_hdr.as_ref() } {
            Some(seq) => Colour::new(
                matches!(seq.mtrx, DAV1D_MC_BT470BG | DAV1D_MC_BT601),
                seq.color_range == 1,
            ),
            None => Colour::new(false, false),
        };
        let picture = Picture {
            luma,
            chroma,
            ss_x,
            ss_y,
        };
        self.stacked.compose(&picture, colour, into);
        Ok(())
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        // SAFETY: the context is open; closing it lets go of every frame it
        // still reads, before `clip` and its bytes are dropped.
        unsafe {
            if !self.data.data.is_null() {
                dav1d_data_unref(&mut self.data);
            }
            dav1d_close(&mut self.context);
        }
    }
}
