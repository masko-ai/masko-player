//! Pose stills (`showStill`): a pose's transparent PNG, scaled into a
//! premultiplied frame.

use std::io::{BufReader, Cursor};
use std::path::Path;

use png::{ColorType, Transformations};

use super::compose::scale_rgba;
use super::{check_size, Frame, MediaError};

/// Decodes a PNG and scales it to `out_width` x `out_height`, premultiplied
/// BGRA like the video frames. The caller picks the size (the still fits the
/// mascot's box as a clip does).
pub fn decode_still(path: &Path, out_width: u32, out_height: u32) -> Result<Frame, MediaError> {
    check_size(out_width, out_height)?;
    let bytes = std::fs::read(path)?;
    let decode = |e: png::DecodingError| match e {
        png::DecodingError::IoError(error) => MediaError::Io(error),
        png::DecodingError::Format(_) => MediaError::Format(format!("not a PNG: {e}")),
        other => MediaError::Decode(other.to_string()),
    };
    let mut decoder = png::Decoder::new(BufReader::new(Cursor::new(bytes)));
    // Palettes and transparency chunks become channels, 16 bits become 8.
    decoder.set_transformations(Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(decode)?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| MediaError::Format("the PNG is too large".into()))?;
    let mut buffer = vec![0u8; size];
    let info = reader.next_frame(&mut buffer).map_err(decode)?;
    let (width, height) = (info.width as usize, info.height as usize);
    if width == 0 || height == 0 {
        return Err(MediaError::Format("the PNG is empty".into()));
    }
    let channels = match info.color_type {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        ColorType::Indexed => {
            return Err(MediaError::Format("an indexed PNG was not expanded".into()))
        }
    };
    // Premultiplied RGBA, so scaling never bleeds the colour of transparent
    // pixels into the edges.
    let mut rgba = vec![0.0f32; width * height * 4];
    for y in 0..height {
        let line = &buffer[y * info.line_size..y * info.line_size + width * channels];
        for (x, px) in line.chunks_exact(channels).enumerate() {
            let (r, g, b, a) = match channels {
                1 => (px[0], px[0], px[0], 255),
                2 => (px[0], px[0], px[0], px[1]),
                3 => (px[0], px[1], px[2], 255),
                _ => (px[0], px[1], px[2], px[3]),
            };
            let k = a as f32 / 255.0;
            let at = (y * width + x) * 4;
            rgba[at] = r as f32 * k;
            rgba[at + 1] = g as f32 * k;
            rgba[at + 2] = b as f32 * k;
            rgba[at + 3] = a as f32;
        }
    }
    let mut frame = Frame::new(out_width, out_height);
    scale_rgba(&rgba, width, height, &mut frame);
    Ok(frame)
}
