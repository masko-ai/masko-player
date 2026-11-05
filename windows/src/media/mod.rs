//! Pictures and sound for the player: stacked AV1 clips decoded with dav1d
//! and drawn transparent, pose stills, a clip's own voice, edge sounds and
//! the one sound output that mixes them all.
//!
//! The Mac player (apple/Sources/MaskoPlayer/AV1Slot.swift) is the model:
//! the same files, the same picture, the same timing, the same rule that a
//! talking clip's picture waits for its voice to be heard.

mod audio;
mod compose;
mod dav1d;
mod mixer;
mod mp4;
mod simd;
mod still;
mod video;

pub use audio::{clip_audio, sound_file, Pcm};
pub use mixer::AudioOut;
pub use still::decode_still;
pub use video::{ClipFile, ClipInfo, VideoDecoder};

use std::fmt;

/// Why a file cannot be read, decoded or played.
#[derive(Debug)]
pub enum MediaError {
    /// The file cannot be read.
    Io(std::io::Error),
    /// The file is not what the player plays: not an MP4, no AV1 track, a
    /// sound in a format it does not decode.
    Format(String),
    /// The file is what it should be, but its data does not decode.
    Decode(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaError::Io(error) => write!(f, "cannot read the file: {error}"),
            MediaError::Format(detail) => write!(f, "unsupported file: {detail}"),
            MediaError::Decode(detail) => write!(f, "cannot decode the file: {detail}"),
        }
    }
}

impl std::error::Error for MediaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MediaError::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for MediaError {
    fn from(error: std::io::Error) -> Self {
        MediaError::Io(error)
    }
}

/// A picture to draw: premultiplied BGRA, top-down rows, stride = width * 4,
/// the layout a transparent window takes as it is. The player keeps one per
/// slot and decodes into it again and again.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    /// A transparent picture of this size.
    pub fn new(width: u32, height: u32) -> Frame {
        Frame {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    /// Gives the frame this size, keeping its buffer when it already fits.
    /// The pixels are left as they are: every caller overwrites them all.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        let len = width as usize * height as usize * 4;
        if self.width != width || self.height != height || self.pixels.len() != len {
            self.width = width;
            self.height = height;
            self.pixels.resize(len, 0);
        }
    }

    /// The pixel at (x, y) as `[b, g, r, a]`, for tests and checks.
    pub fn bgra(&self, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ]
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

/// An output size the player can draw: both sides at least one pixel, and
/// small enough that a frame's bytes fit in memory.
fn check_size(width: u32, height: u32) -> Result<(), MediaError> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(MediaError::Format(format!(
            "cannot draw at {width}x{height}"
        )));
    }
    Ok(())
}
