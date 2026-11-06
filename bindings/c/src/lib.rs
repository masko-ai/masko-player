//! The engine for C firmware (ESP32 boards). Every call that can change what
//! plays returns the commands as a JSON array, in order, which the caller
//! frees with `masko_string_free`. Play time is in milliseconds.
// These are C entry points: the caller owns every pointer, and each function
// checks for null before reading one.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, CStr};
use core::ptr;
use masko_player_engine::{load, Engine, Value};

/// Without std, memory and a crash come from the firmware's C library.
#[cfg(not(feature = "std"))]
mod firmware {
    use core::alloc::{GlobalAlloc, Layout};
    use core::ffi::c_void;

    extern "C" {
        fn malloc(size: usize) -> *mut c_void;
        fn memalign(align: usize, size: usize) -> *mut c_void;
        fn free(ptr: *mut c_void);
        fn abort() -> !;
    }

    struct FirmwareHeap;

    // malloc aligns to 4 bytes on every ESP32; memalign does the rest.
    unsafe impl GlobalAlloc for FirmwareHeap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if layout.align() <= 4 {
                malloc(layout.size()) as *mut u8
            } else {
                memalign(layout.align(), layout.size()) as *mut u8
            }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
            free(ptr as *mut c_void)
        }
    }

    #[global_allocator]
    static HEAP: FirmwareHeap = FirmwareHeap;

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        unsafe { abort() }
    }
}

fn text(s: *const c_char) -> Option<&'static str> {
    if s.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(s) }.to_str().ok()
}

fn owned(s: String) -> *mut c_char {
    CString::new(s).map_or(ptr::null_mut(), CString::into_raw)
}

fn commands(list: Vec<masko_player_engine::Command>) -> *mut c_char {
    owned(serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()))
}

fn engine<'a>(e: *mut Engine) -> Option<&'a mut Engine> {
    unsafe { e.as_mut() }
}

/// Reads a release delivery (`data.delivery`, or the bundle's mascot.json).
/// Returns null when it is refused; `error`, when not null, then receives
/// "code: detail", to free with `masko_string_free`.
#[no_mangle]
pub extern "C" fn masko_engine_load(
    json: *const c_char,
    len: usize,
    error: *mut *mut c_char,
) -> *mut Engine {
    if json.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { core::slice::from_raw_parts(json as *const u8, len) };
    // C strings and ESP-IDF's embedded files end with a zero byte: not JSON.
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let bytes = &bytes[..end];
    let refuse = |message: String| {
        if !error.is_null() {
            unsafe { *error = owned(message) };
        }
        ptr::null_mut()
    };
    let Ok(source) = core::str::from_utf8(bytes) else {
        return refuse("delivery.invalid: not UTF-8".into());
    };
    match load(source) {
        Ok(engine) => Box::into_raw(Box::new(engine)),
        Err(e) => refuse(format!("{}: {}", e.code, e.detail)),
    }
}

#[no_mangle]
pub extern "C" fn masko_engine_free(e: *mut Engine) {
    if !e.is_null() {
        drop(unsafe { Box::from_raw(e) });
    }
}

#[no_mangle]
pub extern "C" fn masko_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

macro_rules! call {
    ($e:expr, $body:expr) => {
        match engine($e) {
            Some(engine) => commands($body(engine)),
            None => ptr::null_mut(),
        }
    };
}

#[no_mangle]
pub extern "C" fn masko_engine_start(e: *mut Engine, now: u64) -> *mut c_char {
    call!(e, |x: &mut Engine| x.start(now))
}

#[no_mangle]
pub extern "C" fn masko_engine_reset(e: *mut Engine, now: u64) -> *mut c_char {
    call!(e, |x: &mut Engine| x.reset(now))
}

#[no_mangle]
pub extern "C" fn masko_engine_set_bool(
    e: *mut Engine,
    name: *const c_char,
    value: bool,
    instant: bool,
    now: u64,
) -> *mut c_char {
    let Some(name) = text(name) else {
        return ptr::null_mut();
    };
    call!(e, |x: &mut Engine| x.set(
        name,
        Value::Bool(value),
        instant,
        now
    ))
}

#[no_mangle]
pub extern "C" fn masko_engine_set_number(
    e: *mut Engine,
    name: *const c_char,
    value: f64,
    instant: bool,
    now: u64,
) -> *mut c_char {
    let Some(name) = text(name) else {
        return ptr::null_mut();
    };
    call!(e, |x: &mut Engine| x.set(
        name,
        Value::Number(value),
        instant,
        now
    ))
}

#[no_mangle]
pub extern "C" fn masko_engine_fire(
    e: *mut Engine,
    name: *const c_char,
    instant: bool,
    now: u64,
) -> *mut c_char {
    let Some(name) = text(name) else {
        return ptr::null_mut();
    };
    call!(e, |x: &mut Engine| x.fire(name, instant, now))
}

/// Call at `masko_engine_next_wake`, or on every frame: it is cheap.
#[no_mangle]
pub extern "C" fn masko_engine_advance(e: *mut Engine, now: u64) -> *mut c_char {
    call!(e, |x: &mut Engine| x.advance(now))
}

/// The clip of this edge showed its first frame.
#[no_mangle]
pub extern "C" fn masko_engine_clip_shown(
    e: *mut Engine,
    edge: *const c_char,
    now: u64,
) -> *mut c_char {
    let Some(edge) = text(edge) else {
        return ptr::null_mut();
    };
    call!(e, |x: &mut Engine| x.clip_shown(edge, now))
}

/// The clip of this edge showed its last frame.
#[no_mangle]
pub extern "C" fn masko_engine_clip_ended(
    e: *mut Engine,
    edge: *const c_char,
    now: u64,
) -> *mut c_char {
    let Some(edge) = text(edge) else {
        return ptr::null_mut();
    };
    call!(e, |x: &mut Engine| x.clip_ended(edge, now))
}

/// The clip of this edge cannot play (missing or broken pack).
#[no_mangle]
pub extern "C" fn masko_engine_clip_failed(
    e: *mut Engine,
    edge: *const c_char,
    now: u64,
) -> *mut c_char {
    let Some(edge) = text(edge) else {
        return ptr::null_mut();
    };
    call!(e, |x: &mut Engine| x.clip_failed(edge, now))
}

/// When the engine next needs `masko_engine_advance`, if ever.
#[no_mangle]
pub extern "C" fn masko_engine_next_wake(e: *const Engine, out: *mut u64) -> bool {
    match (unsafe { e.as_ref() }, unsafe { out.as_mut() }) {
        (Some(engine), Some(out)) => match engine.next_wake() {
            Some(at) => {
                *out = at;
                true
            }
            None => false,
        },
        _ => false,
    }
}
