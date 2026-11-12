#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clip;
mod game;
mod guide;
mod log;

use game::{Panel, Shared};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// Published by poe2speedrun's build.py; one place to update the route.
const GUIDE_URL: &str = "https://poe2speedrun.malura.de/overlay/guide.json";

/// Shortcut strings as the hotkey parser reads them ("Shift+Space", "Ctrl+Alt+KeyR").
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Hotkeys {
    next: String,
    prev: String,
    mini: String,
    hide: String,
    settings: String,
    regex: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys {
            next: "Ctrl+Alt+ArrowRight".into(),
            prev: "Ctrl+Alt+ArrowLeft".into(),
            mini: "Ctrl+Alt+KeyC".into(),
            hide: "Ctrl+Alt+KeyG".into(),
            settings: "Shift+Space".into(),
            regex: "Ctrl+Alt+KeyR".into(),
        }
    }
}

impl Hotkeys {
    /// (shortcut, action) pairs. Err names the first entry that does not parse or is taken twice.
    fn parse(&self) -> Result<Vec<(Shortcut, &'static str)>, String> {
        let mut out: Vec<(Shortcut, &'static str)> = Vec::new();
        for (action, s) in [
            ("next", &self.next),
            ("prev", &self.prev),
            ("mini", &self.mini),
            ("hide", &self.hide),
            ("settings", &self.settings),
            ("regex", &self.regex),
        ] {
            if s.is_empty() {
                continue;
            }
            let sc: Shortcut = s.parse().map_err(|_| format!("{action}: \"{s}\" ist kein gültiger Hotkey"))?;
            if let Some((_, other)) = out.iter().find(|(o, _)| *o == sc) {
                return Err(format!("{action}: \"{s}\" ist schon {other} zugewiesen"));
            }
            out.push((sc, action));
        }
        Ok(out)
    }
}

/// Panel look: text size per mode in percent, background opacity in percent.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Look {
    full_size: u32,
    mini_size: u32,
    opacity: u32,
}

impl Default for Look {
    fn default() -> Self {
        Look { full_size: 100, mini_size: 100, opacity: 85 }
    }
}

impl Look {
    fn clamped(self) -> Self {
        Look { full_size: self.full_size.clamp(50, 300), mini_size: self.mini_size.clamp(50, 300), opacity: self.opacity.min(100) }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct Regex {
    name: String,
    value: String,
}

/// Persisted between runs (state.json in the app config dir).
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Saved {
    step: usize,
    lang: String,
    /// Panel offset from the game's top left corner, physical pixels.
    x: f64,
    y: f64,
    mini: bool,
    /// Override for non-standard install paths, e.g. "D:\Games\Path of Exile 2\logs\Client.txt".
    client_log: Option<PathBuf>,
    /// "all", "buffs" (must + buff) or "must", like the presets on poe2speedrun.
    route: String,
    hotkeys: Hotkeys,
    regexes: Vec<Regex>,
    look: Look,
    /// The campaign panel; off once the campaign is done. Regexes and settings keep working.
    guide_on: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            step: 0,
            lang: "de".into(),
            x: 24.0,
            y: 160.0,
            mini: false,
            client_log: None,
            route: "all".into(),
            hotkeys: Hotkeys::default(),
            regexes: Vec::new(),
            look: Look::default(),
            guide_on: true,
        }
    }
}

/// What the settings window can change.
#[derive(Deserialize)]
struct Settings {
    lang: String,
    route: String,
    hotkeys: Hotkeys,
    regexes: Vec<Regex>,
    look: Look,
    guide_on: bool,
}

/// What the pages render. Sent whole on every change, it is tiny.
#[derive(Serialize, Clone, Default)]
struct Snapshot {
    #[serde(flatten)]
    saved: Saved,
    zone: String,
    char_level: u32,
    area_level: u32,
    log_found: bool,
    log_path: Option<PathBuf>,
    /// Hotkeys Windows refused, usually taken by another overlay.
    hotkey_error: Option<String>,
}

struct App {
    snap: Mutex<Snapshot>,
    /// Parsed guide plus its JSON for the pages.
    guide: RwLock<(guide::Guide, String)>,
    shared: Arc<Mutex<Shared>>,
    bindings: Mutex<Vec<(Shortcut, &'static str)>>,
    /// Hotkeys Windows refused, retried every few seconds (another overlay may let go).
    failed: Mutex<Vec<(Shortcut, &'static str)>>,
    dir: PathBuf,
}

impl App {
    /// Apply a change, persist, and push the new snapshot to the pages.
    fn update(&self, app: &AppHandle, f: impl FnOnce(&mut Snapshot, &guide::Guide)) {
        let snap = {
            let guide = &self.guide.read().unwrap().0;
            let mut s = self.snap.lock().unwrap();
            f(&mut s, guide);
            s.saved.step = guide.snap(s.saved.step.min(guide.len() - 1), &s.saved.route);
            let mut shared = self.shared.lock().unwrap();
            s.log_path = shared.log.clone();
            shared.user_hidden = !s.saved.guide_on;
            drop(shared);
            s.clone()
        };
        let _ = std::fs::write(self.dir.join("state.json"), serde_json::to_string_pretty(&snap.saved).unwrap());
        let _ = app.emit("state", snap);
    }

    fn on_log(&self, app: &AppHandle, e: log::Event) {
        self.update(app, |s, g| match e {
            log::Event::Scene(zone) => {
                if let Some(i) = g.advance(s.saved.step, &zone, &s.saved.route) {
                    s.saved.step = i;
                }
                s.zone = zone;
            }
            log::Event::Area { level, .. } => s.area_level = level,
            log::Event::Level(l) => s.char_level = l,
            log::Event::Opened => s.log_found = true,
            log::Event::Reward(r) => {
                if let Some(i) = g.complete(s.saved.step, &r, &s.saved.route) {
                    s.saved.step = i;
                }
            }
        });
    }

    fn step(&self, app: &AppHandle, delta: i64) {
        self.update(app, |s, g| s.saved.step = g.step(s.saved.step, delta, &s.saved.route));
    }

    /// Swap in a new guide if it parses and differs. Tells the pages to reload it.
    fn set_guide(&self, app: &AppHandle, json: String) -> bool {
        if self.guide.read().unwrap().1 == json {
            return true;
        }
        let Ok(parsed) = guide::Guide::parse(&json) else { return false };
        *self.guide.write().unwrap() = (parsed, json);
        self.update(app, |_, _| {});
        let _ = app.emit("guide", ());
        true
    }

    /// Registers the saved hotkeys. Err lists the ones Windows refused (taken by another app).
    fn register_hotkeys(&self, app: &AppHandle) -> Result<(), String> {
        let bindings = self.snap.lock().unwrap().saved.hotkeys.parse()?;
        let gs = app.global_shortcut();
        let _ = gs.unregister_all();
        let failed: Vec<_> = bindings.iter().filter(|(s, _)| gs.register(*s).is_err()).copied().collect();
        *self.bindings.lock().unwrap() = bindings;
        *self.failed.lock().unwrap() = failed;
        self.report_failed(app)
    }

    /// Main thread only, see register_hotkeys.
    fn retry_failed(&self, app: &AppHandle) {
        let gs = app.global_shortcut();
        let before = self.failed.lock().unwrap().len();
        self.failed.lock().unwrap().retain(|(s, _)| gs.register(*s).is_err());
        if self.failed.lock().unwrap().len() != before {
            let _ = self.report_failed(app);
        }
    }

    fn report_failed(&self, app: &AppHandle) -> Result<(), String> {
        let names: Vec<&str> = self.failed.lock().unwrap().iter().map(|(_, a)| *a).collect();
        let err = (!names.is_empty()).then(|| format!("Hotkey von einer anderen App belegt: {}", names.join(", ")));
        self.update(app, |s, _| s.hotkey_error = err.clone());
        err.map_or(Ok(()), Err)
    }

    fn action(&self, app: &AppHandle, action: &str) {
        match action {
            "next" => self.step(app, 1),
            "prev" => self.step(app, -1),
            "mini" => self.update(app, |s, _| s.saved.mini = !s.saved.mini),
            "hide" => self.update(app, |s, _| s.saved.guide_on = !s.saved.guide_on),
            "settings" => toggle_window(app, "menu"),
            _ => toggle_window(app, "regex"),
        }
    }
}

/// Opens the settings window ("menu", takes focus for typing) or the regex
/// list ("regex", never takes focus, so the game keeps it) over the game, or
/// closes it if it is open. Never (un)registers hotkeys: this runs inside the
/// shortcut plugin's handler, which holds the plugin's lock.
fn toggle_window(app: &AppHandle, label: &str) {
    let Some(win) = app.get_webview_window(label) else { return };
    if win.is_visible().unwrap_or(false) {
        hide_window(app, label);
        return;
    }
    let (w, h) = if label == "menu" { (600.0, 1000.0) } else { (460.0, 340.0) };
    // Copy out: never hold a lock across window calls.
    let game = app.state::<App>().shared.lock().unwrap().game;
    game::center_on_game(&win, game, w, h);
    let _ = win.emit("menu-view", label);
    let _ = win.show();
    if label == "menu" {
        let _ = win.set_focus();
    }
}

fn hide_window(app: &AppHandle, label: &str) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.hide();
    }
    if label == "menu" {
        let st = app.state::<App>();
        st.update(app, |_, _| {}); // drops an unsaved look preview
        game::focus_game(st.shared.lock().unwrap().game_hwnd);
    }
}

/// Fetches the current route in the background; keeps a copy for offline starts.
fn refresh_guide(app: AppHandle) {
    std::thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().into();
        let Ok(mut res) = agent.get(GUIDE_URL).call() else { return };
        let Ok(json) = res.body_mut().read_to_string() else { return };
        let st = app.state::<App>();
        if st.set_guide(&app, json.clone()) {
            let _ = std::fs::write(st.dir.join("guide.json"), json);
        }
    });
}

#[tauri::command]
fn snapshot(app: State<App>) -> Snapshot {
    app.snap.lock().unwrap().clone()
}

#[tauri::command]
fn guide(app: State<App>) -> String {
    app.guide.read().unwrap().1.clone()
}

#[tauri::command]
fn step(handle: AppHandle, app: State<App>, delta: i64) {
    app.step(&handle, delta);
}

#[tauri::command]
fn toggle_mini(handle: AppHandle, app: State<App>) {
    app.update(&handle, |s, _| s.saved.mini = !s.saved.mini);
}

/// The ✕ in the panel: campaign guide off until switched on again.
#[tauri::command]
fn guide_off(handle: AppHandle, app: State<App>) {
    app.update(&handle, |s, _| s.saved.guide_on = false);
}

/// The window follows the panel's rendered size (CSS pixels from the page).
#[tauri::command]
fn panel_size(window: tauri::WebviewWindow, handle: AppHandle, app: State<App>, w: f64, h: f64) {
    let k = window.scale_factor().unwrap_or(1.0);
    let (w, h) = ((w * k).ceil() as i32, (h * k).ceil() as i32);
    let moved = {
        let mut s = app.shared.lock().unwrap();
        let (old, y) = (s.panel.h, s.panel.y);
        // In the lower half of the game the panel grows upwards: its bottom edge stays put.
        let lower = s.game.is_some_and(|g| old > 0 && y + old / 2 > (g.bottom - g.top) / 2);
        if lower {
            s.panel.y -= h - old;
        }
        (s.panel.w, s.panel.h) = (w, h);
        game::place(&window, &mut s);
        (lower && h != old).then_some((s.panel.x, s.panel.y))
    };
    // Saved so a restart finds the panel where it was, at its current size.
    if let Some((x, y)) = moved {
        app.update(&handle, |s, _| (s.saved.x, s.saved.y) = (x as f64, y as f64));
    }
}

/// phase: "start" | "move" | "end". The offset is saved on "end".
#[tauri::command]
fn drag(window: tauri::WebviewWindow, handle: AppHandle, app: State<App>, phase: String) {
    let done = game::drag(&window, &mut app.shared.lock().unwrap(), &phase);
    if let Some((x, y)) = done {
        app.update(&handle, |s, _| (s.saved.x, s.saved.y) = (x as f64, y as f64));
    }
}

/// Saves the settings window. Bad hotkeys are rejected and nothing changes.
#[tauri::command]
fn save_settings(handle: AppHandle, app: State<App>, settings: Settings) -> Result<(), String> {
    settings.hotkeys.parse()?;
    app.update(&handle, |s, _| {
        s.saved.lang = settings.lang;
        s.saved.route = settings.route;
        s.saved.hotkeys = settings.hotkeys;
        s.saved.regexes = settings.regexes.into_iter().filter(|r| !r.value.trim().is_empty()).collect();
        s.saved.look = settings.look.clamped();
        s.saved.guide_on = settings.guide_on;
    });
    app.register_hotkeys(&handle)
}

/// Live preview while the sliders move; not saved. Closing the settings re-sends the saved look.
#[tauri::command]
fn preview_look(handle: AppHandle, look: Look) {
    let _ = handle.emit("look", look.clamped());
}

#[tauri::command]
fn close(window: tauri::WebviewWindow) {
    hide_window(window.app_handle(), window.label());
}

#[tauri::command]
fn copy_regex(window: tauri::WebviewWindow, value: String) -> Result<(), String> {
    clip::copy(&value).map_err(|e| e.to_string())?;
    hide_window(window.app_handle(), window.label());
    Ok(())
}

#[tauri::command]
fn reload_guide(handle: AppHandle) {
    refresh_guide(handle);
}

fn main() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|handle, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let action = handle.state::<App>().bindings.lock().unwrap().iter().find(|(s, _)| s == shortcut).map(|(_, a)| *a);
                    // Runs on the main thread under the plugin's lock: actions must not
                    // (un)register hotkeys. Registering happens only at start and on save.
                    if let Some(a) = action {
                        handle.state::<App>().action(handle, a);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            snapshot,
            guide,
            step,
            toggle_mini,
            guide_off,
            panel_size,
            drag,
            save_settings,
            close,
            preview_look,
            copy_regex,
            reload_guide
        ])
        .on_window_event(|window, event| {
            // Clicking back into the game closes the menu.
            if window.label() == "menu" && matches!(event, WindowEvent::Focused(false)) {
                let _ = window.hide();
                let app = window.app_handle();
                app.state::<App>().update(app, |_, _| {}); // drops an unsaved look preview
            }
        })
        .setup(|tauri_app| {
            let handle = tauri_app.handle().clone();
            let dir = handle.path().app_config_dir()?;
            std::fs::create_dir_all(&dir)?;
            let saved: Saved = std::fs::read_to_string(dir.join("state.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            // Last fetched route if there is one, else the one built in.
            let json = std::fs::read_to_string(dir.join("guide.json"))
                .ok()
                .filter(|j| guide::Guide::parse(j).is_ok())
                .unwrap_or_else(|| include_str!("../ui/guide.json").to_string());
            let pinned = saved.client_log.clone().filter(|p| p.is_file());
            let shared = Arc::new(Mutex::new(Shared {
                panel: Panel { x: saved.x as i32, y: saved.y as i32, ..Default::default() },
                log_pinned: pinned.is_some(),
                log: pinned.or_else(log::find_client_log),
                user_hidden: !saved.guide_on,
                ..Default::default()
            }));
            tauri_app.manage(App {
                snap: Mutex::new(Snapshot { saved, ..Default::default() }),
                guide: RwLock::new((guide::Guide::parse(&json)?, json)),
                shared: shared.clone(),
                bindings: Mutex::new(Vec::new()),
                failed: Mutex::new(Vec::new()),
                dir,
            });
            // Windows are created only now ("create": false in tauri.conf.json):
            // their pages call commands right away, and those need the state above.
            for cfg in tauri_app.config().app.windows.clone() {
                tauri::WebviewWindowBuilder::from_config(tauri_app.handle(), &cfg)?.build()?;
            }
            shared.lock().unwrap().menu_hwnd = tauri_app.get_webview_window("menu").unwrap().hwnd().map_or(0, |h| h.0 as isize);
            if let Err(e) = handle.state::<App>().register_hotkeys(&handle) {
                eprintln!("hotkeys: {e}");
            }
            // Retry refused hotkeys on the main thread, outside the shortcut handler.
            let h = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(5));
                if !h.state::<App>().failed.lock().unwrap().is_empty() {
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || h2.state::<App>().retry_failed(&h2));
                }
            });
            refresh_guide(handle.clone());

            let (h, sh) = (handle.clone(), shared.clone());
            log::spawn_tail(move || sh.lock().unwrap().log.clone(), move |e| h.state::<App>().on_log(&h, e));
            game::spawn_tracker(tauri_app.get_webview_window("overlay").unwrap(), tauri_app.get_webview_window("regex").unwrap(), shared);

            let settings = MenuItem::with_id(tauri_app, "settings", "Einstellungen", true, None::<&str>)?;
            let reset = MenuItem::with_id(tauri_app, "reset", "Guide auf Anfang setzen", true, None::<&str>)?;
            let quit = MenuItem::with_id(tauri_app, "quit", "Beenden", true, None::<&str>)?;
            TrayIconBuilder::new()
                .icon(tauri_app.default_window_icon().unwrap().clone())
                .tooltip("PoE2 Overlay")
                .menu(&Menu::with_items(tauri_app, &[&settings, &reset, &quit])?)
                .on_menu_event(|handle, event| match event.id.as_ref() {
                    "settings" => toggle_window(handle, "menu"),
                    "reset" => handle.state::<App>().update(handle, |s, _| s.saved.step = 0),
                    "quit" => handle.exit(0),
                    _ => {}
                })
                .build(tauri_app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    #[test]
    fn existing_state_file_still_loads() {
        let s: super::Saved = serde_json::from_str(r#"{"step":8,"lang":"de","x":2646.0,"y":851.0,"mini":false,"client_log":null}"#).unwrap();
        assert_eq!((s.step, s.x as i32, s.route.as_str()), (8, 2646, "all"));
    }

    #[test]
    fn hotkeys_parse_and_reject_duplicates() {
        let mut h = super::Hotkeys::default();
        assert_eq!(h.parse().unwrap().len(), 6);
        h.regex = h.next.clone();
        assert!(h.parse().unwrap_err().contains("schon next"));
        h.regex = "Ctrl+Nonsense".into();
        assert!(h.parse().is_err());
    }
}
