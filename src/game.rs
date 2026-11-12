//! Keeps the overlay window on the game: the window is exactly the panel,
//! placed at the saved offset inside the game's client area, and visible only
//! while the game is in front. No click-through needed: everything around the
//! panel is the game itself.

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
#[derive(Default, Clone, Copy)]
pub struct Panel {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Default)]
pub struct Shared {
    pub panel: Panel,
    /// Set by the user hotkey: stays hidden even while the game has focus.
    pub user_hidden: bool,
    /// Client.txt to follow. The tracker points it at the running game's log
    /// unless the user pinned a path in state.json.
    pub log: Option<PathBuf>,
    pub log_pinned: bool,
    /// Game client area on screen, None while there is no game window.
    pub(crate) game: Option<RECT>,
    /// Bounds last given to the overlay window.
    pub(crate) placed: RECT,
    /// While dragging: cursor and panel offset at the start.
    pub(crate) drag: Option<(POINT, i32, i32)>,
    /// Raw HWNDs (HWND itself is not Send): the game, and the settings/regex
    /// window, which counts as "game in front" while it has focus.
    pub(crate) game_hwnd: isize,
    pub menu_hwnd: isize,
}

/// Hands the keyboard back to the game after the menu closes.
pub fn focus_game(game_hwnd: isize) {
    if game_hwnd != 0 {
        let _ = unsafe { SetForegroundWindow(HWND(game_hwnd as _)) };
    }
}

/// Sizes a window (logical px) and puts it in the middle of the game.
pub fn center_on_game(win: &WebviewWindow, game: Option<RECT>, w: f64, h: f64) {
    let k = win.scale_factor().unwrap_or(1.0);
    // Never larger than the game window (small windowed games).
    let fit = |v: f64, room: Option<i32>| ((v * k) as i32).min(room.map_or(i32::MAX, |r| r - 20));
    let (pw, ph) = (fit(w, game.map(|g| g.right - g.left)), fit(h, game.map(|g| g.bottom - g.top)));
    let _ = win.set_size(PhysicalSize::new(pw as u32, ph as u32));
    if let Some(g) = game {
        let _ = win.set_position(PhysicalPosition::new(g.left + (g.right - g.left - pw) / 2, g.top + (g.bottom - g.top - ph) / 2));
    }
}

/// Moves and sizes the overlay window to the panel, keeping it inside the game.
pub fn place(win: &WebviewWindow, s: &mut Shared) {
    let (Some(g), p) = (s.game, s.panel) else { return };
    if p.w == 0 || p.h == 0 {
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

/// Drag by the real cursor position (the window moves under the mouse, so
/// page coordinates would chase themselves). Returns the final offset on "end".
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
    Some(exe.parent()?.join("logs").join("Client.txt")).filter(|p| p.is_file())
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

/// `regex`: the regex list, hidden together with the overlay when the game leaves the front.
pub fn spawn_tracker(win: WebviewWindow, regex: WebviewWindow, shared: Arc<Mutex<Shared>>) {
    // Test knob: point the overlay at another window while the real game runs.
    let title = HSTRING::from(std::env::var("POE2OVERLAY_GAME_TITLE").unwrap_or_else(|_| "Path of Exile 2".into()));
    thread::spawn(move || {
        let (mut visible, mut last_game) = (false, None);
        loop {
            // ponytail: 100 ms polling, switch to SetWinEventHook if the lag ever shows
            thread::sleep(Duration::from_millis(100));
            let game = unsafe { FindWindowW(None, &title) }.ok();
            let rect = game.and_then(client_rect).filter(|r| r.right > r.left);
            let mut s = shared.lock().unwrap();
            let fg = unsafe { GetForegroundWindow() };
            let in_front = game.is_some_and(|g| (fg == g || (s.menu_hwnd != 0 && fg.0 as isize == s.menu_hwnd)) && !unsafe { IsIconic(g) }.as_bool());
            s.game_hwnd = game.map_or(0, |g| g.0 as isize);
            if game.is_some() && game != last_game && !s.log_pinned {
                if let Some(log) = game.and_then(game_log) {
                    s.log = Some(log);
                }
            }
            last_game = game;
            s.game = rect;
            place(&win, &mut s);
            let want_visible = in_front && rect.is_some() && !s.user_hidden;
            if want_visible != visible {
                let _ = if want_visible { win.show() } else { win.hide() };
                if !want_visible {
                    let _ = regex.hide();
                }
                visible = want_visible;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;

    #[test]
    #[ignore = "needs a running game: cargo test -- --ignored"]
    fn finds_log_of_running_game() {
        let game = unsafe { FindWindowW(None, w!("Path of Exile 2")) }.expect("game not running");
        let log = game_log(game).expect("no Client.txt next to the game exe");
        println!("{}", log.display());
    }
}
