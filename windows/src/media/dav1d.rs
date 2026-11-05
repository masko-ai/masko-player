//! The few functions and structures of dav1d (VideoLAN's AV1 decoder) the
//! player calls, written by hand from dav1d 1.5.4's headers (API 7), the
//! version scripts/build-dav1d-windows.sh and scripts/build-dav1d-apple.sh
//! pin. build.rs links the static library.
//!
//! The layouts are those of the C headers on 64-bit targets. The tests check
//! them against the defaults dav1d writes into its settings.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_int, c_uint, c_void};

#[repr(C)]
pub struct Dav1dContext {
    _private: [u8; 0],
}

#[repr(C)]
pub struct Dav1dRef {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Dav1dUserData {
    pub data: *const u8,
    pub r#ref: *mut Dav1dRef,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Dav1dDataProps {
    pub timestamp: i64,
    pub duration: i64,
    pub offset: i64,
    pub size: usize,
    pub user_data: Dav1dUserData,
}

#[repr(C)]
pub struct Dav1dData {
    pub data: *const u8,
    pub sz: usize,
    pub r#ref: *mut Dav1dRef,
    pub m: Dav1dDataProps,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Dav1dPicAllocator {
    pub cookie: *mut c_void,
    pub alloc_picture_callback: *mut c_void,
    pub release_picture_callback: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Dav1dLogger {
    pub cookie: *mut c_void,
    pub callback: *mut c_void,
}

#[repr(C)]
pub struct Dav1dSettings {
    pub n_threads: c_int,
    pub max_frame_delay: c_int,
    pub apply_grain: c_int,
    pub operating_point: c_int,
    pub all_layers: c_int,
    pub frame_size_limit: c_uint,
    pub allocator: Dav1dPicAllocator,
    pub logger: Dav1dLogger,
    pub strict_std_compliance: c_int,
    pub output_invisible_frames: c_int,
    pub inloop_filters: c_int,
    pub decode_frame_type: c_int,
    pub reserved: [u8; 16],
}

/// `enum Dav1dPixelLayout`.
pub const DAV1D_PIXEL_LAYOUT_I400: c_int = 0;
pub const DAV1D_PIXEL_LAYOUT_I420: c_int = 1;
pub const DAV1D_PIXEL_LAYOUT_I422: c_int = 2;
pub const DAV1D_PIXEL_LAYOUT_I444: c_int = 3;

/// `enum Dav1dMatrixCoefficients`, the values the player tells apart.
pub const DAV1D_MC_BT470BG: c_int = 5;
pub const DAV1D_MC_BT601: c_int = 6;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Dav1dPictureParameters {
    pub w: c_int,
    pub h: c_int,
    pub layout: c_int,
    pub bpc: c_int,
}

/// The start of `Dav1dSequenceHeader`, up to its colour description: the
/// player only reads it through a picture's pointer, never makes one.
#[repr(C)]
pub struct Dav1dSequenceHeaderStart {
    pub profile: u8,
    pub max_width: c_int,
    pub max_height: c_int,
    pub layout: c_int,
    pub pri: c_int,
    pub trc: c_int,
    pub mtrx: c_int,
    pub chr: c_int,
    pub hbd: u8,
    pub color_range: u8,
}

#[repr(C)]
pub struct Dav1dPicture {
    pub seq_hdr: *const Dav1dSequenceHeaderStart,
    pub frame_hdr: *const c_void,
    pub data: [*mut c_void; 3],
    pub stride: [isize; 2],
    pub p: Dav1dPictureParameters,
    pub m: Dav1dDataProps,
    pub content_light: *mut c_void,
    pub mastering_display: *mut c_void,
    pub itut_t35: *mut c_void,
    pub n_itut_t35: usize,
    pub reserved: [usize; 4],
    pub frame_hdr_ref: *mut Dav1dRef,
    pub seq_hdr_ref: *mut Dav1dRef,
    pub content_light_ref: *mut Dav1dRef,
    pub mastering_display_ref: *mut Dav1dRef,
    pub itut_t35_ref: *mut Dav1dRef,
    pub reserved_ref: [usize; 4],
    pub r#ref: *mut Dav1dRef,
    pub allocator_data: *mut c_void,
}

impl Dav1dPicture {
    pub fn zeroed() -> Self {
        // SAFETY: every field is an integer or a raw pointer, for which all
        // zero bits are valid; dav1d expects a zeroed picture to fill.
        unsafe { std::mem::zeroed() }
    }
}

impl Dav1dData {
    pub fn empty() -> Self {
        // SAFETY: as above, and an all-zero Dav1dData is dav1d's empty data.
        unsafe { std::mem::zeroed() }
    }
}

/// `DAV1D_ERR(EAGAIN)`: dav1d negates the C library's errno values, and
/// EAGAIN is 35 on Apple systems and 11 on Windows (MSVC) and Linux.
#[cfg(target_vendor = "apple")]
pub const DAV1D_EAGAIN: c_int = -35;
#[cfg(not(target_vendor = "apple"))]
pub const DAV1D_EAGAIN: c_int = -11;

pub type FreeCallback = unsafe extern "C" fn(buf: *const u8, cookie: *mut c_void);

unsafe extern "C" {
    pub fn dav1d_version() -> *const std::ffi::c_char;
    pub fn dav1d_default_settings(s: *mut Dav1dSettings);
    pub fn dav1d_open(c_out: *mut *mut Dav1dContext, s: *const Dav1dSettings) -> c_int;
    pub fn dav1d_data_wrap(
        data: *mut Dav1dData,
        buf: *const u8,
        sz: usize,
        free_callback: FreeCallback,
        cookie: *mut c_void,
    ) -> c_int;
    pub fn dav1d_data_unref(data: *mut Dav1dData);
    pub fn dav1d_send_data(c: *mut Dav1dContext, data: *mut Dav1dData) -> c_int;
    pub fn dav1d_get_picture(c: *mut Dav1dContext, out: *mut Dav1dPicture) -> c_int;
    pub fn dav1d_picture_unref(p: *mut Dav1dPicture);
    pub fn dav1d_flush(c: *mut Dav1dContext);
    pub fn dav1d_close(c_out: *mut *mut Dav1dContext);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_layout_matches_the_library() {
        // dav1d writes its documented defaults; reading them back at the
        // right places shows the structure matches the C one.
        let mut settings: Dav1dSettings = unsafe { std::mem::zeroed() };
        settings.reserved = [0xAB; 16];
        unsafe { dav1d_default_settings(&mut settings) };
        assert_eq!(settings.n_threads, 0);
        assert_eq!(settings.max_frame_delay, 0);
        assert_eq!(settings.apply_grain, 1);
        assert_eq!(settings.operating_point, 0);
        assert_eq!(settings.all_layers, 1);
        assert_eq!(settings.frame_size_limit, 0);
        assert!(!settings.allocator.alloc_picture_callback.is_null());
        assert_eq!(settings.strict_std_compliance, 0);
        assert_eq!(settings.output_invisible_frames, 0);
        assert_eq!(settings.inloop_filters, 7);
        assert_eq!(settings.decode_frame_type, 0);
        assert_eq!(std::mem::size_of::<Dav1dSettings>(), 96);
        assert_eq!(std::mem::size_of::<Dav1dPicture>(), 272);
        assert_eq!(std::mem::size_of::<Dav1dData>(), 72);
    }

    #[test]
    fn it_is_the_pinned_version() {
        let version = unsafe { std::ffi::CStr::from_ptr(dav1d_version()) };
        assert!(
            version.to_str().unwrap().starts_with("1.5.4"),
            "{version:?}"
        );
    }
}
