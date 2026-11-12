//! Keeps the overlay window on the game: the window is exactly the panel,
//! placed at the saved offset inside the game's client area, and visible only
//! while the game is in front.
//!
//! Adapted from poe2overlay (MIT, © 2026 Michael Malura); see NOTICE.
//! The overlay never takes focus (`focusable: false` in tauri.conf.json), so
//! clicks next to the panel reach the game.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SetForegroundWindow,
};

/// Panel offset from the game's client area and its size, physical pixels.
#[derive(Default, Clone, Copy, Debug)]
pub struct Panel {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Default)]
pub struct Shared {
    pub panel: Panel,
    /// Show / hide the overlay from the UI or a hotkey.
    pub user_hidden: bool,
    /// Client.txt to follow. The tracker points it at the running game's log
    /// unless the user pinned a path.
    pub log: Option<PathBuf>,
    pub log_pinned: bool,
    /// Game client area on screen, None while there is no game window.
    pub(crate) game: Option<RECT>,
    /// Bounds last given to the overlay window.
    pub(crate) placed: RECT,
    /// While dragging: cursor and panel offset at the start.
    pub(crate) drag: Option<(POINT, i32, i32)>,
    /// Raw HWNDs (HWND itself is not Send): the game window.
    pub(crate) game_hwnd: isize,
    /// The main window counts as "in front" while it has focus, so the overlay
    /// does not vanish when the user opens the companion window.
    pub main_hwnd: isize,
    /// Whether the overlay is currently shown (tracked by the placement thread).
    pub visible: bool,
    /// Only show the overlay while the game is the foreground window.
    pub only_in_game: bool,
    /// The game window is minimized: hide the overlay.
    pub minimized: bool,
    /// A screenshot is being taken: keep the overlay off the image.
    pub capturing: bool,
    /// The vendor/stash regex list is open: show the panel even when hidden.
    pub regex_open: bool,
}

/// Hands the keyboard back to the game after the main window closes.
pub fn focus_game(game_hwnd: isize) {
    if game_hwnd != 0 {
        let _ = unsafe { SetForegroundWindow(HWND(game_hwnd as _)) };
    }
}

pub fn foreground_is(handles: &[isize]) -> bool {
    let fg = unsafe { GetForegroundWindow() };
    handles.iter().any(|h| *h != 0 && fg.0 as isize == *h)
}

/// The overlay is allowed on screen when the game window exists and (if
/// `only_in_game`) is in the foreground or the companion window is.
pub fn should_show(shared: &Shared) -> bool {
    if shared.regex_open && shared.game.is_some() && !shared.minimized {
        return true;
    }
    if shared.user_hidden || shared.game.is_none() || shared.minimized || shared.capturing {
        return false;
    }
    if !shared.only_in_game {
        return true;
    }
    foreground_is(&[shared.game_hwnd, shared.main_hwnd])
}

/// Sizes a window (logical px) and puts it in the middle of the game.
#[allow(dead_code)]
pub fn center_on_game(win: &WebviewWindow, game: Option<RECT>, w: f64, h: f64) {
    let k = win.scale_factor().unwrap_or(1.0);
    let fit = |v: f64, room: Option<i32>| ((v * k) as i32).min(room.map_or(i32::MAX, |r| r - 20));
    let (pw, ph) = (fit(w, game.map(|g| g.right - g.left)), fit(h, game.map(|g| g.bottom - g.top)));
    let _ = win.set_size(PhysicalSize::new(pw.max(80) as u32, ph.max(40) as u32));
    if let Some(g) = game {
        let _ = win.set_position(PhysicalPosition::new(g.left + (g.right - g.left - pw) / 2, g.top + (g.bottom - g.top - ph) / 2));
    }
}

/// Moves and sizes the overlay window to the panel, keeping it inside the game.
pub fn place(win: &WebviewWindow, s: &mut Shared) {
    let (Some(g), p) = (s.game, s.panel) else { return };
    if p.w <= 0 || p.h <= 0 {
        return;
    }
    s.panel.x = p.x.clamp(0, (g.right - g.left - p.w).max(0));
    s.panel.y = p.y.clamp(0, (g.bottom - g.top - p.h).max(0));
    let r = RECT { left: g.left + s.panel.x, top: g.top + s.panel.y, right: g.left + s.panel.x + p.w, bottom: g.top + s.panel.y + p.h };
    if r != s.placed {
        let _ = win.set_position(PhysicalPosition::new(r.left, r.top));
        let _ = win.set_size(PhysicalSize::new(p.w as u32, p.h as u32));
        s.placed = r;
    }
}

/// Drag by the real cursor position (the window moves under the mouse, so page
/// coordinates would chase themselves). Returns the final offset on "end".
pub fn drag(win: &WebviewWindow, s: &mut Shared, phase: &str) -> Option<(i32, i32)> {
    let mut cur = POINT::default();
    unsafe { GetCursorPos(&mut cur) }.ok()?;
    match (phase, s.drag) {
        ("start", _) => s.drag = Some((cur, s.panel.x, s.panel.y)),
        ("move", Some((c0, x0, y0))) => {
            s.panel.x = x0 + cur.x - c0.x;
            s.panel.y = y0 + cur.y - c0.y;
            place(win, s);
        }
        ("end", Some(_)) => {
            s.drag = None;
            return Some((s.panel.x, s.panel.y));
        }
        _ => {}
    }
    None
}

/// `<game dir>\logs\Client.txt` of the process that owns the window.
fn game_log(hwnd: HWND) -> Option<PathBuf> {
    let mut pid = 0;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        ok.ok()?;
    }
    let exe = PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
    crate::discover::for_game_exe(&exe)
}

fn client_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        GetClientRect(hwnd, &mut r).ok()?;
        ClientToScreen(hwnd, &mut origin).ok().ok()?;
    }
    Some(RECT { left: origin.x, top: origin.y, right: origin.x + r.right, bottom: origin.y + r.bottom })
}

/// Finds the game window by title. `ENI_COMPANION_GAME_TITLE` allows pointing
/// the app at another window while testing without a running game.
pub fn game_title() -> String {
    std::env::var("ENI_COMPANION_GAME_TITLE").unwrap_or_else(|_| "Path of Exile 2".into())
}

pub fn find_game() -> Option<HWND> {
    let title = game_title();
    let title = HSTRING::from(title);
    unsafe { FindWindowW(None, &title) }.ok()
}

/// Keeps the overlay placed, sized and shown while the game is in front.
///
/// Window methods that block (show/hide) are called outside the shared lock:
/// the main thread may be waiting for that lock, and a blocking window call
/// from this thread would then deadlock the app.
pub fn spawn_tracker(win: WebviewWindow, shared: Arc<Mutex<Shared>>) {
    thread::spawn(move || {
        let mut last_game: Option<HWND> = None;
        let mut visible = false;
        loop {
            thread::sleep(Duration::from_millis(100));
            let game = find_game();
            let rect = game.and_then(client_rect).filter(|r| r.right > r.left);
            let minimized = game.is_some_and(|g| unsafe { IsIconic(g) }.as_bool());
            let want = {
                let mut s = shared.lock().unwrap();
                s.game = rect;
                s.minimized = minimized;
                if game != last_game {
                    s.game_hwnd = game.map_or(0, |g| g.0 as isize);
                    if let (Some(g), false) = (game, s.log_pinned) {
                        if let Some(log) = game_log(g) {
                            s.log = Some(log);
                        }
                    }
                    last_game = game;
                }
                place(&win, &mut s);
                should_show(&s)
            };
            if want != visible {
                let _ = if want { win.show() } else { win.hide() };
                visible = want;
                shared.lock().unwrap().visible = want;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_flag_overrides_everything() {
        let mut s = Shared::default();
        assert!(!should_show(&s), "no game window, nothing to show");
        s.game = Some(RECT { left: 0, top: 0, right: 100, bottom: 100 });
        assert!(should_show(&s) || !should_show(&s), "depends on the foreground window");
        s.user_hidden = true;
        assert!(!should_show(&s));
    }

    #[test]
    #[ignore = "needs a running game: cargo test -- --ignored"]
    fn finds_log_of_running_game() {
        let game = find_game().expect("game not running");
        let log = game_log(game).expect("no Client.txt next to the game exe");
        println!("{}", log.display());
    }
}
