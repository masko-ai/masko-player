//! Turns a decoded stacked picture into the transparent picture the window
//! draws, at the size it draws it: colour from the top half, transparency
//! from the luma of the bottom half, colour premultiplied by it. The same
//! sums as the Mac's Metal shader (AV1Slot.swift, `StackedRenderer`) and
//! Android's GL shader, done on the CPU at the output size, so a small
//! mascot costs little whatever the clip's size.
//!
//! Each half is resampled on its own, clamped to its own rows, so filtering
//! never mixes the bottom of the colour into the top of the transparency
//! (Android samples half a texel inside each half for the same reason).

use super::{simd, Frame};

/// How each output pixel along one axis is made from source pixels: `taps`
/// source indices and weights per output pixel. A tent filter: linear
/// interpolation when enlarging; when shrinking, a tent as wide as the
/// shrink, so every source pixel counts and fine detail does not shimmer.
pub(crate) struct Axis {
    pub taps: usize,
    pub index: Vec<u32>,
    pub weight: Vec<f32>,
    /// Output pixel i is source pixel i: nothing to resample.
    pub identity: bool,
}

impl Axis {
    /// `src` source pixels to `out` output pixels. Indices are clamped to
    /// the source, then moved by `offset` (a half's first row).
    pub fn new(src: usize, out: usize, offset: usize) -> Axis {
        let src = src.max(1);
        let out = out.max(1);
        let scale = src as f64 / out as f64;
        let radius = scale.max(1.0);
        let mut taps_of: Vec<Vec<(u32, f32)>> = Vec::with_capacity(out);
        for i in 0..out {
            let centre = (i as f64 + 0.5) * scale - 0.5;
            let low = (centre - radius).floor() as i64 + 1;
            let high = (centre + radius).ceil() as i64 - 1;
            let mut taps: Vec<(u32, f32)> = Vec::new();
            let mut sum = 0.0;
            for j in low..=high {
                let w = 1.0 - (j as f64 - centre).abs() / radius;
                if w <= 1e-6 {
                    continue;
                }
                let index = (j.clamp(0, src as i64 - 1) as usize + offset) as u32;
                sum += w;
                match taps.iter_mut().find(|(at, _)| *at == index) {
                    Some(tap) => tap.1 += w as f32,
                    None => taps.push((index, w as f32)),
                }
            }
            if taps.is_empty() {
                let index =
                    ((centre.round().clamp(0.0, src as f64 - 1.0)) as usize + offset) as u32;
                taps.push((index, 1.0));
                sum = 1.0;
            }
            for tap in &mut taps {
                tap.1 = (tap.1 as f64 / sum) as f32;
            }
            taps_of.push(taps);
        }
        let taps = taps_of.iter().map(Vec::len).max().unwrap_or(1);
        let mut index = Vec::with_capacity(out * taps);
        let mut weight = Vec::with_capacity(out * taps);
        for list in &taps_of {
            for t in 0..taps {
                let (i, w) = list.get(t).copied().unwrap_or((list[0].0, 0.0));
                index.push(i);
                weight.push(w);
            }
        }
        let identity = taps == 1
            && offset == 0
            && src == out
            && index.iter().enumerate().all(|(i, &at)| at as usize == i)
            && weight.iter().all(|&w| w == 1.0);
        Axis {
            taps,
            index,
            weight,
            identity,
        }
    }

    /// `out[i]` = the weighted sum of `row` at output pixel i's taps.
    fn resample(&self, row: &[f32], out: &mut [f32]) {
        match self.taps {
            1 => {
                for (o, &i) in out.iter_mut().zip(&self.index) {
                    *o = row[i as usize];
                }
            }
            2 => {
                let (index, _) = self.index.as_chunks::<2>();
                let (weight, _) = self.weight.as_chunks::<2>();
                for ((o, i), w) in out.iter_mut().zip(index).zip(weight) {
                    *o = w[0] * row[i[0] as usize] + w[1] * row[i[1] as usize];
                }
            }
            taps => {
                for ((o, i), w) in out
                    .iter_mut()
                    .zip(self.index.chunks_exact(taps))
                    .zip(self.weight.chunks_exact(taps))
                {
                    let mut sum = 0.0;
                    for (&i, &w) in i.iter().zip(w) {
                        sum += w * row[i as usize];
                    }
                    *o = sum;
                }
            }
        }
    }

    fn of(&self, out: usize) -> (&[u32], &[f32]) {
        let at = out * self.taps;
        (
            &self.index[at..at + self.taps],
            &self.weight[at..at + self.taps],
        )
    }
}

/// One plane of a decoded picture: rows of `width` bytes, `stride` apart.
#[derive(Clone, Copy)]
pub(crate) struct Plane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl<'a> Plane<'a> {
    pub fn row(&self, y: usize) -> &'a [u8] {
        &self.data[y * self.stride..y * self.stride + self.width]
    }
}

/// A decoded picture: the luma plane and, unless it is monochrome, the two
/// chroma planes, `ss_x` and `ss_y` (0 or 1) times smaller.
pub(crate) struct Picture<'a> {
    pub luma: Plane<'a>,
    pub chroma: Option<(Plane<'a>, Plane<'a>)>,
    pub ss_x: usize,
    pub ss_y: usize,
}

/// How YUV becomes RGB: the stream's matrix and range. Masko's clips are
/// BT.709, limited range; a stream that says otherwise is read as it says.
/// Luma becomes `(y - y_offset) * y_scale`; the coefficients apply to
/// chroma minus 128, its range scaling included.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Colour {
    pub y_offset: f32,
    pub y_scale: f32,
    pub r_cr: f32,
    pub g_cb: f32,
    pub g_cr: f32,
    pub b_cb: f32,
}

impl Colour {
    /// `bt601` for streams whose matrix is BT.601 (or BT.470BG), BT.709
    /// otherwise, including unspecified; `full` for full-range streams.
    pub fn new(bt601: bool, full: bool) -> Colour {
        let (kr, kb) = if bt601 {
            (0.299f64, 0.114f64)
        } else {
            (0.2126, 0.0722)
        };
        let kg = 1.0 - kr - kb;
        let (y_offset, y_scale, c_scale) = if full {
            (0.0, 1.0, 1.0)
        } else {
            (16.0, 255.0 / 219.0, 255.0 / 224.0)
        };
        Colour {
            y_offset,
            y_scale: y_scale as f32,
            r_cr: (2.0 * (1.0 - kr) * c_scale) as f32,
            g_cb: (2.0 * kb * (1.0 - kb) / kg * c_scale) as f32,
            g_cr: (2.0 * kr * (1.0 - kr) / kg * c_scale) as f32,
            b_cb: (2.0 * (1.0 - kb) * c_scale) as f32,
        }
    }

    /// One premultiplied BGRA pixel from the colour's Y, Cb and Cr and the
    /// transparency's luma `mask`, all 0 to 255 (simd.rs does the same four
    /// pixels at a time).
    #[inline(always)]
    pub fn pixel(&self, y: f32, cb: f32, cr: f32, mask: f32) -> [u8; 4] {
        let y = (y - self.y_offset) * self.y_scale;
        let cb = cb - 128.0;
        let cr = cr - 128.0;
        let r = (y + self.r_cr * cr).clamp(0.0, 255.0);
        let g = (y - self.g_cb * cb - self.g_cr * cr).clamp(0.0, 255.0);
        let b = (y + self.b_cb * cb).clamp(0.0, 255.0);
        let a = ((mask - self.y_offset) * self.y_scale).clamp(0.0, 255.0);
        let k = a * (1.0 / 255.0);
        [
            (b * k + 0.5) as u8,
            (g * k + 0.5) as u8,
            (r * k + 0.5) as u8,
            (a + 0.5) as u8,
        ]
    }
}

/// The tables and rows for one picture size and one output size, made once
/// and reused for every frame.
pub(crate) struct Stacked {
    key: (usize, usize, usize, usize, bool, u32, u32),
    luma_x: Axis,
    chroma_x: Axis,
    top: Axis,
    bottom: Axis,
    chroma_y: Axis,
    /// One row of each plane, blended vertically at the source width, then
    /// resampled to the output width.
    source: [Vec<f32>; 4],
    output: [Vec<f32>; 4],
}

impl Stacked {
    pub fn new() -> Stacked {
        Stacked {
            key: (0, 0, 0, 0, false, 0, 0),
            luma_x: Axis::new(1, 1, 0),
            chroma_x: Axis::new(1, 1, 0),
            top: Axis::new(1, 1, 0),
            bottom: Axis::new(1, 1, 0),
            chroma_y: Axis::new(1, 1, 0),
            source: Default::default(),
            output: Default::default(),
        }
    }

    fn prepare(&mut self, picture: &Picture, out_width: u32, out_height: u32) {
        let width = picture.luma.width;
        let height = picture.luma.height;
        let key = (
            width,
            height,
            picture.ss_x,
            picture.ss_y,
            picture.chroma.is_some(),
            out_width,
            out_height,
        );
        if key == self.key {
            return;
        }
        self.key = key;
        let (out_w, out_h) = (out_width as usize, out_height as usize);
        // The visible picture is the top half; the transparency starts
        // where the bottom half does.
        let half = (height / 2).max(1);
        let bottom_start = height - half;
        self.luma_x = Axis::new(width, out_w, 0);
        self.top = Axis::new(half, out_h, 0);
        self.bottom = Axis::new(half, out_h, bottom_start);
        let chroma_width = match picture.chroma {
            Some((cb, _)) => {
                let chroma_half = ((half + picture.ss_y) >> picture.ss_y).clamp(1, cb.height);
                self.chroma_x = Axis::new(cb.width, out_w, 0);
                self.chroma_y = Axis::new(chroma_half, out_h, 0);
                cb.width
            }
            None => 0,
        };
        self.source = [
            vec![0.0; width],
            vec![0.0; width],
            vec![0.0; chroma_width],
            vec![0.0; chroma_width],
        ];
        // A monochrome picture has neutral chroma.
        self.output = [
            vec![0.0; out_w],
            vec![0.0; out_w],
            vec![128.0; out_w],
            vec![128.0; out_w],
        ];
    }

    /// Draws `picture` into `into`, which already has the output size.
    pub fn compose(&mut self, picture: &Picture, colour: Colour, into: &mut Frame) {
        self.prepare(picture, into.width, into.height);
        let out_w = into.width as usize;
        let [y_in, mask_in, cb_in, cr_in] = &mut self.source;
        let [y_out, mask_out, cb_out, cr_out] = &mut self.output;
        for (oy, line) in into.pixels.chunks_exact_mut(out_w * 4).enumerate() {
            blend_rows(picture.luma, self.top.of(oy), y_in);
            blend_rows(picture.luma, self.bottom.of(oy), mask_in);
            if let Some((cb, cr)) = picture.chroma {
                let rows = self.chroma_y.of(oy);
                blend_rows(cb, rows, cb_in);
                blend_rows(cr, rows, cr_in);
                self.chroma_x.resample(cb_in, cb_out);
                self.chroma_x.resample(cr_in, cr_out);
            }
            let (y, mask): (&[f32], &[f32]) = if self.luma_x.identity {
                (y_in, mask_in)
            } else {
                self.luma_x.resample(y_in, y_out);
                self.luma_x.resample(mask_in, mask_out);
                (y_out, mask_out)
            };
            simd::convert(&colour, y, mask, cb_out, cr_out, line);
        }
    }
}

/// `out[x]` = the weighted sum of the plane's rows at `rows`.
fn blend_rows(plane: Plane, (rows, weights): (&[u32], &[f32]), out: &mut [f32]) {
    simd::blend_rows(plane, rows, weights, out);
}

/// Scales a premultiplied RGBA picture (0 to 255 per channel, as floats)
/// into `into`'s size as premultiplied BGRA, with the same tent filter.
pub(crate) fn scale_rgba(src: &[f32], width: usize, height: usize, into: &mut Frame) {
    let out_w = into.width as usize;
    let xs = Axis::new(width, out_w, 0);
    let ys = Axis::new(height, into.height as usize, 0);
    let mut row = vec![0.0f32; width * 4];
    for oy in 0..into.height as usize {
        let (rows, weights) = ys.of(oy);
        row.fill(0.0);
        for (&r, &w) in rows.iter().zip(weights) {
            if w == 0.0 {
                continue;
            }
            let line = &src[r as usize * width * 4..(r as usize + 1) * width * 4];
            for (o, &s) in row.iter_mut().zip(line) {
                *o += w * s;
            }
        }
        let line = &mut into.pixels[oy * out_w * 4..(oy + 1) * out_w * 4];
        for (ox, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (index, weight) = xs.of(ox);
            let mut rgba = [0.0f32; 4];
            for (&i, &w) in index.iter().zip(weight) {
                let at = i as usize * 4;
                for c in 0..4 {
                    rgba[c] += w * row[at + c];
                }
            }
            let a = rgba[3].clamp(0.0, 255.0);
            // Premultiplied colour never exceeds its alpha.
            let channel = |v: f32| (v.clamp(0.0, a) + 0.5) as u8;
            *pixel = [
                channel(rgba[2]),
                channel(rgba[1]),
                channel(rgba[0]),
                (a + 0.5) as u8,
            ];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_size_copies_each_pixel() {
        let axis = Axis::new(8, 8, 0);
        assert_eq!(axis.taps, 1);
        assert_eq!(axis.index, (0..8).collect::<Vec<u32>>());
        assert!(axis.weight.iter().all(|&w| (w - 1.0).abs() < 1e-6));
    }

    #[test]
    fn halving_averages_neighbours_and_weights_sum_to_one() {
        let axis = Axis::new(8, 4, 0);
        for out in 0..4 {
            let (index, weight) = axis.of(out);
            let sum: f32 = weight.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5);
            // Output 1 is centred between source pixels 2 and 3.
            if out == 1 {
                let near: f32 = index
                    .iter()
                    .zip(weight)
                    .filter(|(i, _)| **i == 2 || **i == 3)
                    .map(|(_, w)| w)
                    .sum();
                assert!(near > 0.7, "{index:?} {weight:?}");
            }
        }
    }

    #[test]
    fn indices_stay_in_their_half() {
        let axis = Axis::new(10, 3, 10);
        assert!(axis.index.iter().all(|&i| (10..20).contains(&i)));
        let up = Axis::new(4, 9, 4);
        assert!(up.index.iter().all(|&i| (4..8).contains(&i)));
    }

    #[test]
    fn bt709_limited_matches_the_mac_shader() {
        let colour = Colour::new(false, false);
        assert!((colour.y_scale - 1.164384).abs() < 1e-5);
        assert!((colour.r_cr - 1.792741).abs() < 1e-4);
        assert!((colour.g_cb - 0.213249).abs() < 1e-4);
        assert!((colour.g_cr - 0.532909).abs() < 1e-4);
        assert!((colour.b_cb - 2.112402).abs() < 1e-4);
        // Video white over an opaque mask is opaque white; a black mask is
        // fully transparent, colour included.
        assert_eq!(
            colour.pixel(235.0, 128.0, 128.0, 235.0),
            [255, 255, 255, 255]
        );
        assert_eq!(colour.pixel(235.0, 128.0, 128.0, 16.0), [0, 0, 0, 0]);
        // Half transparency premultiplies.
        let half = colour.pixel(235.0, 128.0, 128.0, 16.0 + 219.0 / 2.0);
        assert_eq!(half[3], 128);
        assert!(half[0].abs_diff(128) <= 1);
    }
}
