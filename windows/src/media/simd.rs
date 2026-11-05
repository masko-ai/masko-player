//! The two loops the composer spends its time in, written with SIMD
//! instructions: SSE2 on x86_64 (every x86_64 processor has it) and NEON on
//! aarch64. The release profile optimises for size, and rustc then turns
//! automatic vectorisation off, so these are written out; the scalar
//! versions are the reference, and the other targets use them.

use super::compose::{Colour, Plane};

/// `out[x]` = the sum of `weights[t]` times the plane's row `rows[t]` at x,
/// for x up to the plane's width.
pub(crate) fn blend_rows(plane: Plane, rows: &[u32], weights: &[f32], out: &mut [f32]) {
    let n = plane.width.min(out.len());
    let out = &mut out[..n];
    let done = arch::blend_rows(plane, rows, weights, out);
    for (x, o) in out.iter_mut().enumerate().skip(done) {
        *o = rows
            .iter()
            .zip(weights)
            .map(|(&row, w)| w * plane.row(row as usize)[x] as f32)
            .sum();
    }
}

/// Premultiplied BGRA pixels into `out` (4 bytes each) from the colour's Y,
/// Cb, Cr and the transparency's luma, one value per pixel in each slice.
pub(crate) fn convert(
    colour: &Colour,
    y: &[f32],
    mask: &[f32],
    cb: &[f32],
    cr: &[f32],
    out: &mut [u8],
) {
    let n = y
        .len()
        .min(mask.len())
        .min(cb.len())
        .min(cr.len())
        .min(out.len() / 4);
    let (y, mask, cb, cr) = (&y[..n], &mask[..n], &cb[..n], &cr[..n]);
    let done = arch::convert(colour, y, mask, cb, cr, &mut out[..n * 4]);
    for i in done..n {
        out[i * 4..i * 4 + 4].copy_from_slice(&colour.pixel(y[i], cb[i], cr[i], mask[i]));
    }
}

#[cfg(target_arch = "x86_64")]
mod arch {
    use super::{Colour, Plane};
    use std::arch::x86_64::*;

    /// Returns how many values it wrote (a multiple of 4).
    pub fn blend_rows(plane: Plane, rows: &[u32], weights: &[f32], out: &mut [f32]) -> usize {
        let n = out.len() / 4 * 4;
        // SAFETY: SSE2 is part of x86_64. Every row is the plane's, at least
        // out.len() bytes long; loads read 4 bytes at x < n, stores write 4
        // floats at x < n.
        unsafe {
            let zero = _mm_setzero_si128();
            for x in (0..n).step_by(4) {
                let mut sum = _mm_setzero_ps();
                for (&row, &w) in rows.iter().zip(weights) {
                    let bytes =
                        (plane.row(row as usize).as_ptr().add(x) as *const i32).read_unaligned();
                    let wide =
                        _mm_unpacklo_epi16(_mm_unpacklo_epi8(_mm_cvtsi32_si128(bytes), zero), zero);
                    sum = _mm_add_ps(sum, _mm_mul_ps(_mm_set1_ps(w), _mm_cvtepi32_ps(wide)));
                }
                _mm_storeu_ps(out.as_mut_ptr().add(x), sum);
            }
        }
        n
    }

    /// Returns how many pixels it wrote (a multiple of 4).
    pub fn convert(
        c: &Colour,
        y: &[f32],
        mask: &[f32],
        cb: &[f32],
        cr: &[f32],
        out: &mut [u8],
    ) -> usize {
        let n = y.len() / 4 * 4;
        // SAFETY: SSE2 is part of x86_64; the slices are all n long (out 4n
        // bytes), and every access is at i < n.
        unsafe {
            let offset = _mm_set1_ps(c.y_offset);
            let scale = _mm_set1_ps(c.y_scale);
            let mid = _mm_set1_ps(128.0);
            let (r_cr, g_cb, g_cr, b_cb) = (
                _mm_set1_ps(c.r_cr),
                _mm_set1_ps(c.g_cb),
                _mm_set1_ps(c.g_cr),
                _mm_set1_ps(c.b_cb),
            );
            let zero = _mm_setzero_ps();
            let max = _mm_set1_ps(255.0);
            let inverse = _mm_set1_ps(1.0 / 255.0);
            let clamp = |v| _mm_min_ps(_mm_max_ps(v, zero), max);
            for i in (0..n).step_by(4) {
                let luma = _mm_mul_ps(_mm_sub_ps(_mm_loadu_ps(y.as_ptr().add(i)), offset), scale);
                let blue = _mm_sub_ps(_mm_loadu_ps(cb.as_ptr().add(i)), mid);
                let red = _mm_sub_ps(_mm_loadu_ps(cr.as_ptr().add(i)), mid);
                let r = clamp(_mm_add_ps(luma, _mm_mul_ps(r_cr, red)));
                let g = clamp(_mm_sub_ps(
                    _mm_sub_ps(luma, _mm_mul_ps(g_cb, blue)),
                    _mm_mul_ps(g_cr, red),
                ));
                let b = clamp(_mm_add_ps(luma, _mm_mul_ps(b_cb, blue)));
                let a = clamp(_mm_mul_ps(
                    _mm_sub_ps(_mm_loadu_ps(mask.as_ptr().add(i)), offset),
                    scale,
                ));
                let k = _mm_mul_ps(a, inverse);
                let pixels = _mm_or_si128(
                    _mm_or_si128(
                        _mm_cvtps_epi32(_mm_mul_ps(b, k)),
                        _mm_slli_epi32::<8>(_mm_cvtps_epi32(_mm_mul_ps(g, k))),
                    ),
                    _mm_or_si128(
                        _mm_slli_epi32::<16>(_mm_cvtps_epi32(_mm_mul_ps(r, k))),
                        _mm_slli_epi32::<24>(_mm_cvtps_epi32(a)),
                    ),
                );
                _mm_storeu_si128(out.as_mut_ptr().add(i * 4) as *mut __m128i, pixels);
            }
        }
        n
    }
}

#[cfg(target_arch = "aarch64")]
mod arch {
    use super::{Colour, Plane};
    use std::arch::aarch64::*;

    /// Returns how many values it wrote (a multiple of 8).
    pub fn blend_rows(plane: Plane, rows: &[u32], weights: &[f32], out: &mut [f32]) -> usize {
        let n = out.len() / 8 * 8;
        // SAFETY: NEON is part of aarch64. Every row is the plane's, at least
        // out.len() bytes long; loads read 8 bytes at x < n, stores write 8
        // floats at x < n.
        unsafe {
            for x in (0..n).step_by(8) {
                let mut low = vdupq_n_f32(0.0);
                let mut high = vdupq_n_f32(0.0);
                for (&row, &w) in rows.iter().zip(weights) {
                    let wide = vmovl_u8(vld1_u8(plane.row(row as usize).as_ptr().add(x)));
                    low = vfmaq_n_f32(low, vcvtq_f32_u32(vmovl_u16(vget_low_u16(wide))), w);
                    high = vfmaq_n_f32(high, vcvtq_f32_u32(vmovl_high_u16(wide)), w);
                }
                vst1q_f32(out.as_mut_ptr().add(x), low);
                vst1q_f32(out.as_mut_ptr().add(x + 4), high);
            }
        }
        n
    }

    /// Returns how many pixels it wrote (a multiple of 4).
    pub fn convert(
        c: &Colour,
        y: &[f32],
        mask: &[f32],
        cb: &[f32],
        cr: &[f32],
        out: &mut [u8],
    ) -> usize {
        let n = y.len() / 4 * 4;
        // SAFETY: NEON is part of aarch64; the slices are all n long (out 4n
        // bytes), and every access is at i < n.
        unsafe {
            let offset = vdupq_n_f32(c.y_offset);
            let scale = vdupq_n_f32(c.y_scale);
            let mid = vdupq_n_f32(128.0);
            let zero = vdupq_n_f32(0.0);
            let max = vdupq_n_f32(255.0);
            let clamp = |v| vminq_f32(vmaxq_f32(v, zero), max);
            for i in (0..n).step_by(4) {
                let luma = vmulq_f32(vsubq_f32(vld1q_f32(y.as_ptr().add(i)), offset), scale);
                let blue = vsubq_f32(vld1q_f32(cb.as_ptr().add(i)), mid);
                let red = vsubq_f32(vld1q_f32(cr.as_ptr().add(i)), mid);
                let r = clamp(vfmaq_n_f32(luma, red, c.r_cr));
                let g = clamp(vfmsq_n_f32(vfmsq_n_f32(luma, blue, c.g_cb), red, c.g_cr));
                let b = clamp(vfmaq_n_f32(luma, blue, c.b_cb));
                let a = clamp(vmulq_f32(
                    vsubq_f32(vld1q_f32(mask.as_ptr().add(i)), offset),
                    scale,
                ));
                let k = vmulq_n_f32(a, 1.0 / 255.0);
                let pixels = vorrq_u32(
                    vorrq_u32(
                        vcvtnq_u32_f32(vmulq_f32(b, k)),
                        vshlq_n_u32::<8>(vcvtnq_u32_f32(vmulq_f32(g, k))),
                    ),
                    vorrq_u32(
                        vshlq_n_u32::<16>(vcvtnq_u32_f32(vmulq_f32(r, k))),
                        vshlq_n_u32::<24>(vcvtnq_u32_f32(a)),
                    ),
                );
                vst1q_u32(out.as_mut_ptr().add(i * 4) as *mut u32, pixels);
            }
        }
        n
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod arch {
    use super::{Colour, Plane};

    pub fn blend_rows(_: Plane, _: &[u32], _: &[f32], _: &mut [f32]) -> usize {
        0
    }

    pub fn convert(_: &Colour, _: &[f32], _: &[f32], _: &[f32], _: &[f32], _: &mut [u8]) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_simd_loops_give_what_the_scalar_ones_give() {
        let (width, height, stride) = (37, 3, 40);
        let data: Vec<u8> = (0..stride * height)
            .map(|i| ((i * 37 + 91) % 256) as u8)
            .collect();
        let plane = Plane {
            data: &data,
            stride,
            width,
            height,
        };
        let rows = [2u32, 0, 1];
        let weights = [0.25, 0.5, 0.25];
        let mut out = vec![0.0; width];
        blend_rows(plane, &rows, &weights, &mut out);
        for (x, &o) in out.iter().enumerate() {
            let expected: f32 = rows
                .iter()
                .zip(weights)
                .map(|(&r, w)| w * data[r as usize * stride + x] as f32)
                .sum();
            assert!((o - expected).abs() < 1e-3, "{x}: {o} {expected}");
        }

        for colour in [Colour::new(false, false), Colour::new(true, true)] {
            let n = 4 * 64 + 3;
            let value = |i: usize, k: usize| ((i * (31 + k * 17) + k * 50) % 256) as f32 + 0.25;
            let y: Vec<f32> = (0..n).map(|i| value(i, 0)).collect();
            let mask: Vec<f32> = (0..n).map(|i| value(i, 1)).collect();
            let cb: Vec<f32> = (0..n).map(|i| value(i, 2)).collect();
            let cr: Vec<f32> = (0..n).map(|i| value(i, 3)).collect();
            let mut out = vec![0u8; n * 4];
            convert(&colour, &y, &mask, &cb, &cr, &mut out);
            for i in 0..n {
                let got = &out[i * 4..i * 4 + 4];
                let expected = colour.pixel(y[i], cb[i], cr[i], mask[i]);
                for c in 0..4 {
                    assert!(
                        got[c].abs_diff(expected[c]) <= 1,
                        "pixel {i}: {got:?} {expected:?}"
                    );
                }
                assert!(got[0] <= got[3] && got[1] <= got[3] && got[2] <= got[3]);
            }
        }
    }
}
