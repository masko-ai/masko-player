//! Draws the player's picture in a transparent window on Windows: a layered
//! window (`WS_EX_LAYERED`) updated with `UpdateLayeredWindow` from a
//! premultiplied BGRA picture. Windows composes it over the desktop with its
//! own transparency, so clicks and the pointer pass through wherever the
//! mascot is not drawn, and nothing else of the window shows.
//!
//! The app owns the window (its styles, where it sits, its messages); the
//! view only fills it. The window must have `WS_EX_LAYERED` and must not
//! call `SetLayeredWindowAttributes`.

use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

use crate::media::Frame;

/// The picture's memory: a top-down 32-bit DIB section selected into a
/// memory DC, reused while the size stays the same.
pub struct LayeredView {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl LayeredView {
    pub fn new() -> LayeredView {
        LayeredView {
            dc: HDC::default(),
            bitmap: HBITMAP::default(),
            previous: HGDIOBJ::default(),
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
        }
    }

    /// Shows `frame` in the window, at its own size, where the window is.
    pub fn present(&mut self, window: HWND, frame: &Frame) -> windows::core::Result<()> {
        self.ensure(frame.width, frame.height)?;
        let bytes = (frame.width * frame.height * 4) as usize;
        if frame.pixels.len() >= bytes {
            // SAFETY: `bits` points at width x height x 4 bytes owned by the
            // DIB section, which lives until `release`.
            unsafe {
                std::ptr::copy_nonoverlapping(frame.pixels.as_ptr(), self.bits, bytes);
            }
        }
        self.update(window)
    }

    /// Shows nothing: a fully transparent window of the same size.
    pub fn clear(&mut self, window: HWND) -> windows::core::Result<()> {
        if self.bits.is_null() {
            return Ok(());
        }
        let bytes = (self.width * self.height * 4) as usize;
        // SAFETY: as in `present`.
        unsafe {
            std::ptr::write_bytes(self.bits, 0, bytes);
        }
        self.update(window)
    }

    fn update(&self, window: HWND) -> windows::core::Result<()> {
        let size = SIZE {
            cx: self.width as i32,
            cy: self.height as i32,
        };
        let origin = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: plain Win32 calls on handles this view owns.
        unsafe {
            let screen = GetDC(None);
            let result = UpdateLayeredWindow(
                window,
                Some(screen),
                None,
                Some(&size),
                Some(self.dc),
                Some(&origin),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            ReleaseDC(None, screen);
            result
        }
    }

    fn ensure(&mut self, width: u32, height: u32) -> windows::core::Result<()> {
        if width == self.width && height == self.height && !self.bits.is_null() {
            return Ok(());
        }
        self.release();
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Negative: rows top-down, as `Frame` keeps them.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: plain Win32 calls; every handle is released in `release`.
        unsafe {
            let screen = GetDC(None);
            self.dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            self.bitmap =
                CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            self.previous = SelectObject(self.dc, self.bitmap.into());
            self.bits = bits as *mut u8;
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn release(&mut self) {
        // SAFETY: the handles were made by `ensure` and are let go once.
        unsafe {
            if !self.dc.is_invalid() {
                SelectObject(self.dc, self.previous);
                let _ = DeleteDC(self.dc);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
        }
        self.dc = HDC::default();
        self.bitmap = HBITMAP::default();
        self.previous = HGDIOBJ::default();
        self.bits = std::ptr::null_mut();
        self.width = 0;
        self.height = 0;
    }
}

impl Default for LayeredView {
    fn default() -> Self {
        LayeredView::new()
    }
}

impl Drop for LayeredView {
    fn drop(&mut self) {
        self.release();
    }
}
