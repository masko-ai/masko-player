//! Plays the test clips in a real transparent window on a Windows desktop
//! and checks what the screen shows: the mascot's transparent corners show
//! the desktop behind them, its middle shows the clip, and a move plays when
//! the pointer input says so. It saves the screen around the window as PNGs
//! in `target/desktop-check/`, for a person to look at too. CI runs it on
//! GitHub's Windows machines:
//!
//!   cargo run --release --example desktop_check

#[cfg(not(windows))]
fn main() {
    eprintln!("desktop_check runs on Windows only");
}

#[cfg(windows)]
fn main() {
    if let Err(problem) = windows_check::run() {
        eprintln!("desktop_check FAILED: {problem}");
        std::process::exit(1);
    }
    println!("desktop_check passed");
}

#[cfg(windows)]
mod windows_check {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use masko_player_windows::view::LayeredView;
    use masko_player_windows::{Event, MaskoPlayer, Options};
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, SRCCOPY,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
        RegisterClassW, ShowWindow, TranslateMessage, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WNDCLASSW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    const SIZE: i32 = 192;
    const X: i32 = 200;
    const Y: i32 = 200;

    extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: the default handling for every message.
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    fn fixture(name: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn delivery() -> String {
        let (lp, mv) = (fixture("loop-av1.mp4"), fixture("move-av1.mp4"));
        serde_json::json!({
            "deliverySchemaVersion": 1, "playerContract": 1, "requires": [],
            "target": "windows", "mode": "selected",
            "runtimeGraph": {
                "runtimeGraphSchemaVersion": 1, "version": "2.1", "name": "Desktop check",
                "initialNode": "idle", "autoPlay": true,
                "nodes": [{"id": "idle"}, {"id": "wave"}],
                "edges": [
                    {"id": "idle-loop", "source": "idle", "target": "idle", "isLoop": true,
                     "duration": 0.5, "videos": {"av1": lp}},
                    {"id": "wave-loop", "source": "wave", "target": "wave", "isLoop": true,
                     "duration": 0.5, "videos": {"av1": lp}},
                    {"id": "to-wave", "source": "idle", "target": "wave", "isLoop": false,
                     "duration": 0.4, "conditions": [{"input": "behavior::pointerHover"}],
                     "videos": {"av1": mv}},
                    {"id": "wave-back", "source": "wave", "target": "idle", "isLoop": false,
                     "duration": 0.4,
                     "conditions": [{"input": "behavior::pointerHover", "value": false}],
                     "videos": {"av1": mv}}
                ]
            }
        })
        .to_string()
    }

    /// The screen's pixels in a rectangle, as BGRA rows top-down.
    fn capture(x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>, String> {
        // SAFETY: plain GDI calls; every handle is released before returning.
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(dc, bitmap.into());
            // CAPTUREBLT includes layered windows.
            BitBlt(dc, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT)
                .map_err(|e| format!("BitBlt: {e}"))?;
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            let lines = GetDIBits(
                dc,
                bitmap,
                0,
                h as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            );
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            if lines == 0 {
                return Err("GetDIBits read nothing".into());
            }
            Ok(pixels)
        }
    }

    fn save_png(path: &Path, bgra: &[u8], w: u32, h: u32) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        let rgb: Vec<u8> = bgra
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        writer.write_image_data(&rgb).map_err(|e| e.to_string())
    }

    fn pixel(bgra: &[u8], w: i32, x: i32, y: i32) -> [u8; 3] {
        let i = ((y * w + x) * 4) as usize;
        [bgra[i], bgra[i + 1], bgra[i + 2]]
    }

    fn differs(a: [u8; 3], b: [u8; 3]) -> bool {
        a.iter().zip(b).any(|(x, y)| x.abs_diff(y) > 24)
    }

    /// Pumps the player and draws, as an app's loop does, until `done` says so.
    fn run_until(
        player: &mut MaskoPlayer,
        view: &mut LayeredView,
        hwnd: HWND,
        log: &mut Vec<String>,
        limit: Duration,
        done: impl Fn(&[String]) -> bool,
    ) -> Result<(), String> {
        let start = Instant::now();
        while !done(log) {
            if start.elapsed() > limit {
                return Err(format!("timed out; events: {log:?}"));
            }
            let mut msg = MSG::default();
            // SAFETY: the usual message pump for this thread's window.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if player.pump() {
                match player.frame() {
                    Some(frame) => view.present(hwnd, frame).map_err(|e| e.to_string())?,
                    None => view.clear(hwnd).map_err(|e| e.to_string())?,
                }
            }
            for event in player.take_events() {
                log.push(match event {
                    Event::Pose { pose } => format!("pose:{pose}"),
                    Event::Clip { clip, .. } => format!("clip:{clip}"),
                    Event::Error { code, .. } => format!("error:{code}"),
                    other => format!("{other:?}"),
                });
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        Ok(())
    }

    pub fn run() -> Result<(), String> {
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/desktop-check");
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let (cx, cy, cw, ch) = (X - 40, Y - 40, SIZE + 80, SIZE + 80);
        let before = capture(cx, cy, cw, ch)?;
        save_png(&out.join("0-desktop.png"), &before, cw as u32, ch as u32)?;

        // SAFETY: a window class and a window for this thread, destroyed at the end.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("MaskoDesktopCheck"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("MaskoDesktopCheck"),
                w!("Masko desktop check"),
                WS_POPUP,
                X,
                Y,
                SIZE,
                SIZE,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| format!("CreateWindowExW: {e}"))?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            hwnd
        };

        let dir = std::env::temp_dir().join("masko-desktop-check");
        let mut options = Options::new(dir, (SIZE as u32, SIZE as u32));
        options.audio = false;
        let mut player = MaskoPlayer::from_delivery(&delivery(), options);
        let mut view = LayeredView::new();
        let mut log = Vec::new();

        // The rest loop shows, then loops.
        let seen = |n: usize| {
            move |log: &[String]| log.iter().filter(|e| *e == "clip:idle-loop").count() >= n
        };
        run_until(
            &mut player,
            &mut view,
            hwnd,
            &mut log,
            Duration::from_secs(10),
            seen(2),
        )?;
        std::thread::sleep(Duration::from_millis(100));
        let shown = capture(cx, cy, cw, ch)?;
        save_png(&out.join("1-idle.png"), &shown, cw as u32, ch as u32)?;

        // The window's corner is transparent: the desktop shows there. Its
        // middle shows the clip.
        let corner = (40 + 2, 40 + 2);
        let middle = (40 + SIZE / 2, 40 + SIZE / 2);
        if differs(
            pixel(&before, cw, corner.0, corner.1),
            pixel(&shown, cw, corner.0, corner.1),
        ) {
            return Err("the window's transparent corner hides the desktop".into());
        }
        if !differs(
            pixel(&before, cw, middle.0, middle.1),
            pixel(&shown, cw, middle.0, middle.1),
        ) {
            return Err("the mascot does not show in the middle of its window".into());
        }

        // A move on the pointer input, and back.
        player.set_bool("behavior::pointerHover", true, true);
        run_until(
            &mut player,
            &mut view,
            hwnd,
            &mut log,
            Duration::from_secs(10),
            |l| l.iter().any(|e| e == "clip:wave-loop"),
        )?;
        let moved = capture(cx, cy, cw, ch)?;
        save_png(&out.join("2-wave.png"), &moved, cw as u32, ch as u32)?;
        player.set_bool("behavior::pointerHover", false, true);
        run_until(
            &mut player,
            &mut view,
            hwnd,
            &mut log,
            Duration::from_secs(10),
            seen(3),
        )?;

        if let Some(error) = log.iter().find(|e| e.starts_with("error:")) {
            return Err(format!("the player reported {error}; events: {log:?}"));
        }
        println!("events: {log:?}");
        player.destroy();
        // SAFETY: the window made above.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        Ok(())
    }
}
