#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Eni-PoE2-Companion — a local companion for Path of Exile 2.
//!
//! Based on poe2overlay by Michael Malura (MIT); see LICENSE and NOTICE.
//!
//! Design rules kept from the original project:
//! * the overlay window is exactly the panel and never takes focus,
//! * hotkeys are registered only at startup and from the main thread,
//! * settings, history and screenshots stay on disk, nothing is uploaded.

mod ai;
mod builds;
mod campaign;
mod capture;
mod clip;
mod discover;
mod game;
mod guide;
mod item;
mod logparse;
mod model;
mod pob;
mod screens;
mod scout;
mod tail;
mod trade;

use model::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything the pages render. `saved` is the persisted state; the rest is
/// live information from the log tail and the game window tracker.
#[derive(Serialize, Clone)]
struct Snapshot {
    saved: Saved,
    game_running: bool,
    log_found: bool,
    log_path: Option<PathBuf>,
    log_encoding: String,
    log_error: Option<String>,
    /// The last zone from the log is not part of the campaign route.
    zone_unknown: bool,
    area_level: u32,
    /// Internal area id of the last `Generating level` line (language independent).
    area_id: String,
    hotkey_error: Option<String>,
    campaign_len: usize,
    campaign_act: u32,
    campaign_version: String,
    shots_dir: PathBuf,
    data_dir: PathBuf,
    build_ids: Vec<String>,
    ai_has_key: bool,
    ai_providers: Vec<ai::ProviderInfo>,
    /// Number of imported items (the records themselves come from `items_list`).
    items_count: usize,
    /// Vendor/stash regexes, shown by the overlay list.
    regexes: Vec<RegexItem>,
    /// The regex list is open in the overlay right now.
    regex_open: bool,
    /// Short confirmation shown in the overlay after an import.
    last_item: Option<ItemToast>,
    /// Quick price from the hotkey (overlay banner).
    quick_price: Option<QuickPrice>,
    /// Current map checklist (runtime, reset on area change).
    map_run: MapRun,
    /// Session counters, reset when the app starts.
    session_started: String,
    session_deaths: u32,
    session_zones: u32,
    session_levels: u32,
    zone_since: String,
    version: String,
}

/// Live checklist of the current endgame map.
#[derive(Serialize, Clone, Debug, Default)]
struct MapRun {
    zone: String,
    started: String,
    rares: u32,
    boss: bool,
    checks: Vec<MapCheck>,
    open: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
struct MapCheck {
    name: String,
    done: bool,
}

impl MapRun {
    fn new(zone: &str, mechanics: &[String]) -> Self {
        MapRun {
            zone: zone.to_string(),
            started: now_iso(),
            rares: 0,
            boss: false,
            checks: mechanics.iter().map(|m| MapCheck { name: m.clone(), done: false }).collect(),
            open: false,
        }
    }

    fn active(&self) -> bool {
        self.rares > 0 || self.boss || self.checks.iter().any(|c| c.done)
    }

    fn summary(&self) -> String {
        let done = self.checks.iter().filter(|c| c.done).count();
        format!(
            "{}: {} редких, босс: {}, механик: {}/{}",
            if self.zone.is_empty() { "карта" } else { &self.zone },
            self.rares,
            if self.boss { "да" } else { "нет" },
            done,
            self.checks.len()
        )
    }
}

/// Result of the quick price hotkey (shown in the overlay for a few seconds).
#[derive(Serialize, Clone, Debug, Default)]
struct QuickPrice {
    name: String,
    total: u64,
    min: f64,
    median: f64,
    currency: String,
    note: String,
    error: Option<String>,
    ts: String,
}

#[derive(Serialize, Clone, Debug)]
struct ItemToast {
    name: String,
    score: i32,
    slot: String,
    ts: String,
}

impl Snapshot {
    /// Copy for the pages: no history (requested separately), no screenshots
    /// list, no encrypted key.
    fn public(&self) -> Snapshot {
        let mut s = self.clone();
        s.items_count = s.saved.items.len();
        s.saved.items.clear();
        s.saved.history.clear();
        s.saved.shots.clear();
        s.ai_has_key = s.saved.ai.key_protected.is_some();
        s.saved.ai.key_protected = None;
        s
    }
}

#[derive(Serialize)]
struct Bootstrap {
    state: Snapshot,
    campaign: campaign::Campaign,
    builds: Vec<builds::BuildDoc>,
    version: String,
}

struct App {
    snap: Mutex<Snapshot>,
    campaign: RwLock<campaign::Campaign>,
    builds: RwLock<Vec<builds::BuildDoc>>,
    shared: Arc<Mutex<game::Shared>>,
    bindings: Mutex<Vec<(Shortcut, &'static str)>>,
    failed: Mutex<Vec<(Shortcut, &'static str)>>,
    dir: PathBuf,
    /// Area id of the last area the log reported, for zone mapping.
    last_area: Mutex<String>,
    /// Overlay regex list toggle (runtime only).
    regex_open: Mutex<bool>,
    /// Last trade request, to keep at least a few seconds between them.
    last_trade: Mutex<Option<std::time::Instant>>,
    /// Trade stat dataset, loaded once per session (and cached on disk).
    stats: Mutex<Option<Vec<trade::StatDef>>>,
    /// Currency rate, cached for a few minutes.
    rates: Mutex<Option<(std::time::Instant, trade::CurrencyRate)>>,
    /// poe2scout league/divine price, cached for 15 minutes.
    scout: Mutex<Option<(std::time::Instant, scout::Scout)>>,
}

fn action_label(action: &str) -> &'static str {
    match action {
        "next" => "следующий шаг",
        "prev" => "предыдущий шаг",
        "mode" => "режим оверлея",
        "hide" => "скрыть оверлей",
        "screenshot" => "скриншот",
        "item" => "импорт предмета",
        "regex" => "regex-список",
        "map" => "чек-лист карты",
        "price" => "быстрая цена",
        _ => "главное окно",
    }
}

fn parse_hotkeys(h: &Hotkeys) -> Result<Vec<(Shortcut, &'static str)>, String> {
    let mut out: Vec<(Shortcut, &'static str)> = Vec::new();
    for (action, value) in h.entries() {
        if value.trim().is_empty() {
            continue;
        }
        let sc: Shortcut = value.parse().map_err(|_| format!("\"{value}\" для действия «{}» — недопустимое сочетание", action_label(action)))?;
        if let Some((_, other)) = out.iter().find(|(o, _)| *o == sc) {
            return Err(format!("\"{value}\" уже занято действием «{}»", action_label(other)));
        }
        out.push((sc, action));
    }
    Ok(out)
}

/// Class names as the client writes them, in English and Russian, mapped to the
/// canonical English name used by the build documents.
fn canonical_class(raw: &str) -> String {
    let key = raw.trim().to_lowercase();
    let canonical = match key.as_str() {
        "ranger" | "лучница" | "следопыт" => "Ranger",
        "huntress" | "охотница" => "Huntress",
        "witch" | "ведьма" => "Witch",
        "sorceress" | "колдунья" | "чародейка" => "Sorceress",
        "warrior" | "воин" => "Warrior",
        "monk" | "монах" => "Monk",
        "mercenary" | "наёмник" | "наемник" => "Mercenary",
        "druid" | "друид" => "Druid",
        _ => return raw.trim().to_string(),
    };
    canonical.to_string()
}

fn push_event(s: &mut Snapshot, kind: &str, title: String, detail: String) {
    let (profile, level, zone, act) = match s.saved.profile() {
        Some(p) => (p.id.clone(), p.level, p.zone.clone(), p.act),
        None => (String::new(), 0, String::new(), 1),
    };
    s.saved.push_history(HistoryEvent {
        id: new_id("h"),
        ts: now_iso(),
        profile,
        kind: kind.to_string(),
        level,
        zone,
        act,
        title,
        detail,
    });
}

impl App {
    /// Screenshot store for a snapshot that is already locked. Never take the
    /// snapshot lock here: callers usually hold it already.
    fn store(snap: &Snapshot) -> screens::Store {
        screens::Store::new(snap.shots_dir.clone())
    }

    fn public(&self) -> Snapshot {
        let game_running = self.shared.lock().unwrap().game.is_some();
        let mut s = self.snap.lock().unwrap();
        s.game_running = game_running;
        s.shots_dir = s.saved.shots_dir.clone().unwrap_or_else(screens::default_shots_dir);
        s.regexes = s.saved.regexes.clone();
        s.public()
    }

    /// Applies a change, keeps the profile step inside the route, persists and
    /// pushes the new snapshot to the pages.
    fn update(&self, app: &AppHandle, f: impl FnOnce(&mut Snapshot, &campaign::Campaign)) {
        let campaign = self.campaign.read().unwrap();
        let mut s = self.snap.lock().unwrap();
        f(&mut s, &campaign);
        let mode = s.saved.route.clone();
        let aliases = s.saved.zone_aliases.clone();
        let len = campaign.len();
        let mut zone_unknown = false;
        if let Some(p) = s.saved.profile_mut() {
            p.step = campaign.snap(p.step.min(len.saturating_sub(1)), &mode);
            p.act = campaign.step(p.step).map(|st| st.act).unwrap_or(p.act);
            p.updated = now_iso();
            zone_unknown = !p.zone.is_empty() && campaign.step_for_zone(&p.zone, &aliases).is_none();
        }
        s.zone_unknown = zone_unknown;
        s.campaign_len = len;
        s.campaign_version = campaign.version.clone();
        s.campaign_act = s.saved.profile().map(|p| p.act).unwrap_or(1);
        s.build_ids = self.builds.read().map(|b| b.iter().map(|d| d.id.clone()).collect()).unwrap_or_default();
        s.regexes = s.saved.regexes.clone();
        s.items_count = s.saved.items.len();
        s.game_running = self.shared.lock().unwrap().game.is_some();
        s.shots_dir = s.saved.shots_dir.clone().unwrap_or_else(screens::default_shots_dir);
        let saved = s.saved.clone();
        let public = s.public();
        drop(s);
        drop(campaign);
        if let Err(e) = model::save_state(&self.dir.join("state.json"), &saved) {
            eprintln!("state.json: {e}");
        }
        let _ = app.emit("state", public);
    }

    fn report_hotkeys(&self, app: &AppHandle, err: Option<String>) {
        self.update(app, |s, _| s.hotkey_error = err);
    }

    /// Registers the saved hotkeys. Err lists the ones Windows refused.
    /// Main thread only (the plugin holds its lock while waiting for it).
    fn register_hotkeys(&self, app: &AppHandle) -> Result<(), String> {
        let bindings = {
            let s = self.snap.lock().unwrap();
            parse_hotkeys(&s.saved.hotkeys)?
        };
        let gs = app.global_shortcut();
        let _ = gs.unregister_all();
        let failed: Vec<_> = bindings.iter().filter(|(sc, _)| gs.register(*sc).is_err()).copied().collect();
        let names: Vec<&str> = failed.iter().map(|(_, a)| *a).collect();
        *self.bindings.lock().unwrap() = bindings;
        *self.failed.lock().unwrap() = failed;
        let err = (!names.is_empty()).then(|| {
            let list: Vec<String> = names.iter().map(|n| format!("«{}»", action_label(n))).collect();
            format!("Горячие клавиши заняты другой программой: {}", list.join(", "))
        });
        // A busy combination is reported in the UI banner, but the rest of the
        // settings must still be saved.
        self.report_hotkeys(app, err);
        Ok(())
    }

    /// Retries refused hotkeys on the main thread, outside the shortcut handler.
    fn retry_failed(&self, app: &AppHandle) {
        let gs = app.global_shortcut();
        let before = self.failed.lock().unwrap().len();
        self.failed.lock().unwrap().retain(|(sc, _)| gs.register(*sc).is_err());
        if self.failed.lock().unwrap().len() != before {
            let names: Vec<&str> = self.failed.lock().unwrap().iter().map(|(_, a)| *a).collect();
            let err = (!names.is_empty()).then(|| {
                let list: Vec<String> = names.iter().map(|n| format!("«{}»", action_label(n))).collect();
                format!("Горячие клавиши заняты другой программой: {}", list.join(", "))
            });
            self.report_hotkeys(app, err);
        }
    }

    fn step(&self, app: &AppHandle, delta: i64) {
        self.update(app, |s, c| {
            let mode = s.saved.route.clone();
            if let Some(p) = s.saved.profile_mut() {
                p.step = c.step_by(p.step, delta, &mode);
            }
        });
    }

    fn cycle_mode(&self, app: &AppHandle) {
        self.update(app, |s, _| {
            s.saved.overlay.mode = match s.saved.overlay.mode.as_str() {
                "compact" => "standard",
                "standard" => "extended",
                _ => "compact",
            }
            .to_string();
        });
    }

    fn toggle_overlay(&self, app: &AppHandle) {
        self.update(app, |s, _| s.saved.overlay.visible = !s.saved.overlay.visible);
        self.apply_overlay_flags();
    }

    fn apply_overlay_flags(&self) {
        let s = self.snap.lock().unwrap();
        let mut shared = self.shared.lock().unwrap();
        shared.user_hidden = !s.saved.overlay.visible;
        shared.only_in_game = s.saved.overlay.only_in_game;
        shared.panel.x = s.saved.overlay.x;
        shared.panel.y = s.saved.overlay.y;
    }

    /// Reads the item under the cursor from the clipboard, parses and scores it.
    fn import_item(&self, app: &AppHandle) -> Result<ItemRecord, String> {
        let text = clip::read().map_err(|e| format!("Буфер обмена: {e}"))?;
        let parsed = item::parse(&text)
            .ok_or("В буфере нет текста предмета. Наведи курсор на предмет в игре и нажми Ctrl+C, затем повтори.")?;
        let (profile_id, level) = {
            let s = self.snap.lock().unwrap();
            (s.saved.active.clone(), s.saved.profile().map(|p| p.level).unwrap_or(1))
        };
        let analysis = item::analyze(&parsed, level);
        let dangers = item::dangers(&parsed);
        let record = ItemRecord {
            id: new_id("item"),
            ts: now_iso(),
            profile: profile_id,
            raw: text.chars().take(8000).collect(),
            parsed: parsed.clone(),
            analysis: analysis.clone(),
            dangers,
            ai: None,
        };
        let toast = ItemToast { name: parsed.name.clone(), score: analysis.score, slot: parsed.slot.clone(), ts: now_iso() };
        let title = if parsed.name.is_empty() { parsed.base.clone() } else { parsed.name.clone() };
        let detail = format!("{} · {}", parsed.rarity, analysis.matches.join(", "));
        self.update(app, |s, _| {
            s.saved.items.push(record.clone());
            if s.saved.items.len() > 200 {
                s.saved.items.remove(0);
            }
            s.last_item = Some(toast.clone());
            push_event(s, "item", format!("Предмет: {title}"), detail);
        });
        Ok(record)
    }

    /// Prices whatever item text is in the clipboard, without touching the game.
    fn quick_price(&self, app: &AppHandle) {
        let (enabled, league, filters) = {
            let s = self.snap.lock().unwrap();
            (s.saved.trade.enabled, s.saved.trade.league.clone(), s.saved.trade.filters as usize)
        };
        let text = clip::read().unwrap_or_default();
        let parsed = item::parse(&text);
        if !enabled {
            self.update(app, |s, _| {
                s.quick_price = Some(QuickPrice {
                    error: Some("Проверка цен выключена: включи её в настройках.".into()),
                    ts: now_iso(),
                    ..Default::default()
                })
            });
            return;
        }
        let Some(parsed) = parsed else {
            self.update(app, |s, _| {
                s.quick_price = Some(QuickPrice {
                    error: Some("В буфере нет предмета: наведи курсор в игре и нажми Ctrl+C.".into()),
                    ts: now_iso(),
                    ..Default::default()
                })
            });
            return;
        };
        self.update(app, |s, _| {
            s.quick_price = Some(QuickPrice {
                name: parsed.name.clone(),
                note: "Смотрю цены…".into(),
                ts: now_iso(),
                ..Default::default()
            })
        });
        let app = app.clone();
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            let stats = read_stats_cache(&dir).or_else(|| trade::fetch_stats().ok());
            if let Some(stats) = &stats {
                write_stats_cache(&dir, stats);
            }
            let result = trade::check(&parsed, &league, stats.as_deref(), filters);
            let quick = match result {
                Ok(r) => QuickPrice {
                    name: parsed.name.clone(),
                    total: r.total,
                    min: r.min,
                    median: r.median,
                    currency: r.currency,
                    note: r.note,
                    error: None,
                    ts: now_iso(),
                },
                Err(e) => QuickPrice { name: parsed.name.clone(), error: Some(e), ts: now_iso(), ..Default::default() },
            };
            let state = app.state::<App>();
            state.update(&app, |s, _| s.quick_price = Some(quick));
        });
    }

    fn toggle_regex(&self, app: &AppHandle) {
        let open = {
            let mut r = self.regex_open.lock().unwrap();
            *r = !*r;
            *r
        };
        self.shared.lock().unwrap().regex_open = open;
        self.update(app, |s, _| s.regex_open = open);
    }

    fn action(&self, app: &AppHandle, action: &str) {
        match action {
            "next" => self.step(app, 1),
            "prev" => self.step(app, -1),
            "mode" => self.cycle_mode(app),
            "hide" => self.toggle_overlay(app),
            "screenshot" => {
                if let Err(e) = self.capture(app, "other") {
                    eprintln!("screenshot: {e}");
                }
            }
            "item" => {
                if let Err(e) = self.import_item(app) {
                    eprintln!("item import: {e}");
                }
            }
            "regex" => self.toggle_regex(app),
            "map" => self.update(app, |s, _| s.map_run.open = !s.map_run.open),
            "price" => self.quick_price(app),
            _ => toggle_main(app),
        }
    }

    /// Takes a screenshot of the game's client area. The overlay is hidden for
    /// the moment of the capture so the image contains the game only.
    fn capture(&self, app: &AppHandle, kind: &str) -> Result<Shot, String> {
        let rect = {
            let mut s = self.shared.lock().unwrap();
            let Some(rect) = s.game else {
                return Err("Окно игры не найдено. Запустите Path of Exile 2 в оконном или безрамочном режиме.".into());
            };
            s.capturing = true;
            rect
        };
        let overlay = app.get_webview_window("overlay");
        if let Some(w) = &overlay {
            let _ = w.hide();
        }
        // Give the compositor a moment to drop the overlay from the desktop.
        std::thread::sleep(Duration::from_millis(120));
        let result = capture::capture_rect(rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top);
        let was_visible = {
            let mut s = self.shared.lock().unwrap();
            s.capturing = false;
            s.visible
        };
        if was_visible {
            if let Some(w) = &overlay {
                let _ = w.show();
            }
        }
        let mut rgba = result?;
        capture::bgra_to_rgba(&mut rgba);
        let (w, h) = ((rect.right - rect.left) as u32, (rect.bottom - rect.top) as u32);

        let (profile_id, zone, level) = {
            let s = self.snap.lock().unwrap();
            match s.saved.profile() {
                Some(p) => (p.id.clone(), p.zone.clone(), p.level),
                None => (String::new(), String::new(), 0),
            }
        };
        let store = {
            let s = self.snap.lock().unwrap();
            Self::store(&s)
        };
        let shot = store.save_capture(&rgba, w, h, screens::new_shot(&profile_id, kind, &zone, level))?;
        self.update(app, |s, _| {
            s.saved.shots.push(shot.clone());
            push_event(s, "shot", format!("Скриншот {}", shot.file), kind.to_string());
        });
        Ok(shot)
    }

    fn on_log(&self, app: &AppHandle, ev: logparse::Event) {
        match ev {
            logparse::Event::Opened { path, encoding } => {
                self.update(app, |s, _| {
                    s.log_path = Some(path.clone());
                    s.log_encoding = encoding.label().to_string();
                    s.log_found = true;
                    s.log_error = None;
                });
            }
            logparse::Event::Truncated => {
                self.update(app, |s, _| {
                    s.log_error = Some("Журнал был усечён или заменён: чтение начато заново".into());
                });
            }
            logparse::Event::Area { id, level } => {
                *self.last_area.lock().unwrap() = id.clone();
                self.update(app, |s, _| {
                    s.area_level = level;
                    s.area_id = id;
                });
            }
            logparse::Event::Scene(zone) => {
                let area = self.last_area.lock().unwrap().clone();
                self.update(app, |s, c| {
                    let mode = s.saved.route.clone();
                    let aliases = s.saved.zone_aliases.clone();
                    let resolved = c.resolve_scene(&zone, Some(area.as_str()), &aliases);
                    {
                        let Some(p) = s.saved.profile_mut() else { return };
                        p.zone = zone.clone();
                        if let Some(i) = c.advance(p.step, &resolved, &mode, &[]) {
                            p.step = i;
                        }
                    }
                    if s.map_run.zone != zone {
                        if s.map_run.active() {
                            let summary = s.map_run.summary();
                            push_event(s, "map", summary, String::new());
                        }
                        let mechanics = s.saved.map.mechanics.clone();
                        let auto_open = s.saved.map.enabled;
                        s.map_run = MapRun::new(&zone, &mechanics);
                        s.map_run.open = auto_open;
                    }
                    s.session_zones += 1;
                    s.zone_since = now_iso();
                    push_event(s, "zone", zone, String::new());
                });
            }
            logparse::Event::Death { name } => {
                self.update(app, |s, _| {
                    let who = name.clone().unwrap_or_default();
                    s.session_deaths += 1;
                    push_event(s, "death", if who.is_empty() { "Смерть".into() } else { format!("{who}: смерть") }, String::new());
                });
            }
            logparse::Event::Level { name, class, level } => {
                self.update(app, |s, _| {
                    let active = s.saved.active.clone();
                    let Some(p) = s.saved.profiles.iter_mut().find(|p| p.id == active) else { return };
                    let placeholder = matches!(p.name.as_str(), "" | "Эни" | "Eni" | "Новый персонаж" | "New character");
                    let accept = match &name {
                        Some(n) => n.eq_ignore_ascii_case(&p.name) || (!p.name_auto && placeholder),
                        None => true,
                    };
                    if !accept {
                        return;
                    }
                    if let Some(n) = &name {
                        p.name = n.clone();
                        p.name_auto = true;
                    }
                    if let Some(c) = &class {
                        p.class = canonical_class(c);
                    }
                    if level > 0 && level != p.level {
                        p.level = level;
                        p.levels.push(LevelRec { ts: now_iso(), level });
                        if p.levels.len() > 200 {
                            p.levels.remove(0);
                        }
                        s.session_levels += 1;
                    }
                });
                let (who, lvl) = {
                    let s = self.snap.lock().unwrap();
                    (s.saved.profile().map(|p| p.name.clone()).unwrap_or_default(), level)
                };
                self.update(app, |s, _| push_event(s, "level", format!("{who}: уровень {lvl}"), String::new()));
            }
            logparse::Event::Reward(text) => {
                self.update(app, |s, c| {
                    let mode = s.saved.route.clone();
                    let kind = c.reward_kind(&text).unwrap_or("other").to_string();
                    {
                        let Some(p) = s.saved.profile_mut() else { return };
                        if let Some(i) = c.complete(p.step, &text, &mode) {
                            p.step = i;
                        }
                        p.rewards.push(RewardRec { ts: now_iso(), kind: kind.clone(), text: text.clone() });
                        if p.rewards.len() > 200 {
                            p.rewards.remove(0);
                        }
                    }
                    push_event(s, "reward", text, kind);
                });
            }
        }
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn toggle_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        let hwnd = app.state::<App>().shared.lock().unwrap().game_hwnd;
        game::focus_game(hwnd);
    } else {
        show_main(app);
    }
}

/// Opens a link in the default browser, but only https on trusted hosts.
fn open_url(url: &str) -> Result<(), String> {
    let host = url
        .strip_prefix("https://")
        .ok_or("Разрешены только https-ссылки")?
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !builds::ALLOWED_SOURCE_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        return Err(format!("Адрес {host} не в списке доверенных"));
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(hwnd: *mut c_void, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> *mut c_void;
        }
        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }
        let op = wide("open");
        let file = wide(url);
        let rc = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        if (rc as isize) <= 32 {
            return Err("Windows не смогла открыть ссылку".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("Открытие ссылок реализовано только для Windows".into())
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn bootstrap(app: AppHandle, state: State<App>) -> Bootstrap {
    let snapshot = state.public();
    let campaign_doc = state.campaign.read().unwrap().clone();
    let build_docs = state.builds.read().unwrap().clone();
    let _ = app.emit("state", snapshot.clone());
    Bootstrap { state: snapshot, campaign: campaign_doc, builds: build_docs, version: VERSION.to_string() }
}

#[tauri::command]
fn snapshot(state: State<App>) -> Snapshot {
    state.public()
}

/// Imports a Mobalytics build page: Edge renders it (Cloudflare), we keep only
/// structural facts (gems, gear, trade queries) and link the source.
#[tauri::command]
async fn build_import_url(app: AppHandle, state: State<'_, App>, url: String) -> Result<builds::BuildDoc, String> {
    if !guide::allowed_url(&url) {
        return Err("Разрешены только страницы билдов Mobalytics (https://mobalytics.gg/poe-2/...)".into());
    }
    let fetch_url = url.clone();
    let parsed = tauri::async_runtime::spawn_blocking(move || {
        let html = guide::fetch_html(&fetch_url)?;
        guide::parse(&html, &fetch_url)
    })
    .await
    .map_err(|e| format!("Импорт прерван: {e}"))??;
    let id = format!("guide-{}", new_id("b"));
    let json = guide::to_build_json(&parsed, &id);
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    let doc = builds::BuildDoc::parse(&text).map_err(|e| format!("Импортированный билд не прошёл проверку: {e}"))?;
    let dir = state.dir.join("builds");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Папка билдов: {e}"))?;
    std::fs::write(dir.join(format!("{id}.json")), &text).map_err(|e| format!("Запись билда: {e}"))?;
    let library = builds::load_library(Some(&dir));
    if let Ok(mut guard) = state.builds.write() {
        *guard = library;
    }
    let record = GuideRecord {
        id: new_id("g"),
        build_id: id.clone(),
        ts: now_iso(),
        url: parsed.url.clone(),
        title: parsed.title.clone(),
        class: parsed.class.clone(),
        ascendancy: parsed.ascendancy.clone(),
        skills: parsed.skills.clone(),
        gear: parsed.gear.clone(),
    };
    state.update(&app, |s, _| {
        if let Some(p) = s.saved.profile_mut() {
            p.build_id = id.clone();
        }
        s.saved.guides.push(record.clone());
        if s.saved.guides.len() > 20 {
            s.saved.guides.remove(0);
        }
        push_event(s, "manual", format!("Импорт гайда: {}", parsed.title), "Mobalytics".to_string());
    });
    Ok(doc.clone())
}

#[tauri::command]
fn guide_for_build(state: State<App>, build_id: String) -> Option<GuideRecord> {
    let s = state.snap.lock().unwrap();
    s.saved.guides.iter().rev().find(|g| g.build_id == build_id).cloned()
}

#[tauri::command]
async fn trade_search_guide(state: State<'_, App>, guide_id: String, slot: String) -> Result<TradeSearchView, String> {
    let league = state.preferred_league();
    let (query, enabled) = {
        let s = state.snap.lock().unwrap();
        let guide = s.saved.guides.iter().find(|g| g.id == guide_id).ok_or("Гайд не найден")?;
        let item = guide.gear.iter().find(|g| g.slot == slot).ok_or("Предмет не найден в гайде")?;
        if item.trade_query.trim().is_empty() {
            return Err("Для этого предмета в гайде нет запроса — открой гайд в браузере.".into());
        }
        (item.trade_query.clone(), s.saved.trade.enabled)
    };
    if !enabled {
        return Err("Торговля выключена: включи проверку цен в настройках.".into());
    }
    {
        let mut last = state.last_trade.lock().unwrap();
        if let Some(at) = *last {
            let elapsed = at.elapsed();
            if elapsed < std::time::Duration::from_secs(3) {
                return Err(format!("Не чаще раза в 3 секунды: подожди {} с.", 3 - elapsed.as_secs()));
            }
        }
        *last = Some(std::time::Instant::now());
    }
    tauri::async_runtime::spawn_blocking(move || trade::search_raw(&query, &league))
        .await
        .map_err(|e| format!("Запрос прерван: {e}"))?
        .map(|(listings, url)| TradeSearchView { listings, url })
}

#[tauri::command]
fn builds_list(state: State<App>) -> Vec<builds::BuildDoc> {
    state.builds.read().map(|b| b.clone()).unwrap_or_default()
}

#[tauri::command]
fn campaign_data(state: State<App>) -> campaign::Campaign {
    state.campaign.read().unwrap().clone()
}

/// Settings coming back from the settings page.
#[derive(Deserialize)]
struct SettingsIn {
    lang: String,
    route: String,
    hotkeys: Hotkeys,
    log: LogCfg,
    overlay: OverlayCfg,
    shots_dir: Option<String>,
    zone_aliases: Vec<ZoneAlias>,
    ai_provider: String,
    ai_model: String,
    ai_reasoning: String,
    ai_enabled: bool,
    #[serde(default)]
    trade_enabled: bool,
    #[serde(default)]
    trade_league: String,
    #[serde(default = "crate::model::default_trade_filters")]
    trade_filters: u8,
    #[serde(default)]
    map_enabled: bool,
    #[serde(default)]
    map_mechanics: Vec<String>,
    /// New key in plain text; only ever stored DPAPI-encrypted.
    ai_key: Option<String>,
    ai_clear_key: bool,
    #[serde(default)]
    regexes: Vec<RegexItem>,
}

#[tauri::command]
fn save_settings(app: AppHandle, state: State<App>, settings: SettingsIn) -> Result<(), String> {
    parse_hotkeys(&settings.hotkeys)?;
    let mut key_protected = {
        let s = state.snap.lock().unwrap();
        s.saved.ai.key_protected.clone()
    };
    if settings.ai_clear_key {
        key_protected = None;
    } else if let Some(k) = settings.ai_key.as_ref().filter(|k| !k.trim().is_empty()) {
        key_protected = Some(ai::protect_key(k.trim())?);
    }
    let shots_dir = settings
        .shots_dir
        .as_ref()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    let log = LogCfg {
        mode: if settings.log.mode == "manual" { "manual".into() } else { "auto".into() },
        path: settings.log.path.filter(|p| !p.as_os_str().is_empty()),
    };
    let pinned = log.mode == "manual" && log.path.as_ref().is_some_and(|p| p.is_file());
    state.update(&app, |s, _| {
        s.saved.lang = if settings.lang == "en" { "en".into() } else { "ru".into() };
        s.saved.route = if settings.route == "must" { "must".into() } else { "all".into() };
        s.saved.hotkeys = settings.hotkeys.clone();
        s.saved.log = log.clone();
        s.saved.overlay = settings.overlay.clone().clamped();
        s.saved.shots_dir = shots_dir.clone();
        s.saved.regexes = settings
            .regexes
            .into_iter()
            .filter(|r| !r.value.trim().is_empty())
            .map(|r| RegexItem { name: r.name.trim().chars().take(40).collect(), value: r.value.trim().to_string() })
            .collect();
        s.saved.zone_aliases = settings
            .zone_aliases
            .into_iter()
            .filter(|a| !a.alias.trim().is_empty() && !a.zone.trim().is_empty())
            .map(|a| ZoneAlias { alias: a.alias.trim().to_string(), zone: a.zone.trim().to_string() })
            .collect();
        s.saved.map = MapCfg {
            enabled: settings.map_enabled,
            mechanics: settings
                .map_mechanics
                .iter()
                .map(|m| m.trim().chars().take(30).collect::<String>())
                .filter(|m| !m.is_empty())
                .take(12)
                .collect(),
        };
        s.saved.trade = TradeCfg {
            enabled: settings.trade_enabled,
            league: settings.trade_league.trim().chars().take(60).collect(),
            filters: settings.trade_filters.min(3),
        };
        s.saved.ai = AiCfg {
            enabled: settings.ai_enabled,
            provider: settings.ai_provider.clone(),
            model: settings.ai_model.clone(),
            reasoning: ai::normalize_reasoning(&settings.ai_reasoning).to_string(),
            key_protected: key_protected.clone(),
        };
    });
    {
        let mut shared = state.shared.lock().unwrap();
        shared.log_pinned = pinned;
        if pinned {
            shared.log = log.path.clone();
        } else {
            // Back to automatic discovery: pick up a running game's log now.
            if let Some(p) = discover::find_client_log() {
                shared.log = Some(p);
            }
        }
        shared.only_in_game = settings.overlay.only_in_game;
        shared.user_hidden = !settings.overlay.visible;
    }
    state.register_hotkeys(&app)
}

/// The overlay page saves its own position/mode/opacity/scale.
#[tauri::command]
fn save_overlay(app: AppHandle, state: State<App>, overlay: OverlayCfg) {
    state.update(&app, |s, _| s.saved.overlay = overlay.clamped());
    state.apply_overlay_flags();
}

#[derive(Serialize, Deserialize, Clone)]
struct LookPreview {
    scale: u32,
    opacity: u32,
    mode: String,
}

#[tauri::command]
fn preview_look(app: AppHandle, look: LookPreview) {
    let _ = app.emit_to("overlay", "look", look);
}

#[tauri::command]
fn profile_new(app: AppHandle, state: State<App>, lang: String) -> String {
    let id = new_id("p");
    state.update(&app, |s, _| {
        let mut p = Profile::default();
        p.id = id.clone();
        p.name = if lang == "en" { "New character".into() } else { "Новый персонаж".into() };
        p.level = 1;
        p.step = 0;
        s.saved.profiles.push(p);
        s.saved.active = id.clone();
    });
    id
}

#[tauri::command]
fn profile_select(app: AppHandle, state: State<App>, id: String) {
    state.update(&app, |s, _| {
        if s.saved.profiles.iter().any(|p| p.id == id) {
            s.saved.active = id;
        }
    });
}

#[derive(Deserialize)]
struct ProfilePatch {
    id: String,
    name: String,
    class: String,
    ascendancy: String,
    level: u32,
    zone: String,
    build_id: String,
}

#[tauri::command]
fn profile_save(app: AppHandle, state: State<App>, patch: ProfilePatch) {
    state.update(&app, |s, c| {
        let aliases = s.saved.zone_aliases.clone();
        if let Some(p) = s.saved.profiles.iter_mut().find(|p| p.id == patch.id) {
            let renamed = p.name != patch.name;
            p.name = patch.name.trim().to_string();
            if renamed {
                p.name_auto = false;
            }
            p.class = canonical_class(&patch.class);
            p.ascendancy = patch.ascendancy.trim().to_string();
            p.level = patch.level.clamp(1, 100);
            p.zone = patch.zone.trim().to_string();
            p.build_id = patch.build_id.clone();
            if !p.zone.is_empty() {
                if let Some(i) = c.step_for_zone(&p.zone, &aliases) {
                    p.step = i;
                }
            }
            p.act = p.act.clamp(1, 6);
            let name = p.name.clone();
            push_event(s, "manual", format!("Профиль обновлён: {name}"), String::new());
        }
    });
}

#[tauri::command]
fn profile_delete(app: AppHandle, state: State<App>, id: String) -> Result<(), String> {
    let mut too_few = false;
    state.update(&app, |s, _| {
        if s.saved.profiles.len() <= 1 {
            too_few = true;
            return;
        }
        s.saved.profiles.retain(|p| p.id != id);
        if !s.saved.profiles.iter().any(|p| p.id == s.saved.active) {
            s.saved.active = s.saved.profiles[0].id.clone();
        }
    });
    if too_few {
        return Err("Нельзя удалить единственный профиль".into());
    }
    Ok(())
}

#[tauri::command]
fn campaign_step(app: AppHandle, state: State<App>, delta: i64) {
    state.step(&app, delta);
}

#[tauri::command]
fn campaign_goto(app: AppHandle, state: State<App>, step: usize) {
    state.update(&app, |s, c| {
        let mode = s.saved.route.clone();
        if let Some(p) = s.saved.profile_mut() {
            p.step = c.snap(step.min(c.len().saturating_sub(1)), &mode);
        }
    });
}

/// Marks the current step done and moves on: the manual counterpart of the
/// automatic zone/reward detection.
#[tauri::command]
fn campaign_complete(app: AppHandle, state: State<App>) {
    state.update(&app, |s, c| {
        let mode = s.saved.route.clone();
        let mut title = String::new();
        if let Some(p) = s.saved.profile() {
            if let Some(step) = c.step(p.step) {
                title = step.goal.ru.clone();
                let id = step.id.clone();
                let done = p.done.contains(&id);
                let next = c.step_by(p.step, 1, &mode);
                if let Some(p) = s.saved.profile_mut() {
                    if !done {
                        p.done.push(id);
                    }
                    p.step = next;
                }
            }
        }
        if !title.is_empty() {
            push_event(s, "task", title, String::from("отмечено вручную"));
        }
    });
}

#[tauri::command]
fn campaign_manual_zone(app: AppHandle, state: State<App>, zone: String) {
    state.update(&app, |s, c| {
        let mode = s.saved.route.clone();
        let aliases = s.saved.zone_aliases.clone();
        {
            let Some(p) = s.saved.profile_mut() else { return };
            p.zone = zone.clone();
            if let Some(i) = c.advance(p.step, &zone, &mode, &aliases) {
                p.step = i;
            }
        }
        push_event(s, "zone", zone, String::from("указано вручную"));
    });
}

/// Maps the zone name the log reports (and its area id) to a route zone, then
/// moves the pointer. This is how a localized client becomes complete over time.
#[tauri::command]
fn zone_map(app: AppHandle, state: State<App>, zone: String) {
    let area = state.last_area.lock().unwrap().clone();
    state.update(&app, |s, c| {
        let Some(p) = s.saved.profile() else { return };
        let raw = p.zone.clone();
        let mode = s.saved.route.clone();
        let mut new_aliases: Vec<ZoneAlias> = Vec::new();
        for alias in [raw.clone(), area.clone()] {
            let alias = alias.trim().to_string();
            if alias.is_empty() {
                continue;
            }
            if s.saved.zone_aliases.iter().any(|a| crate::campaign::normalize(&a.alias) == crate::campaign::normalize(&alias)) {
                continue;
            }
            new_aliases.push(ZoneAlias { alias, zone: zone.clone() });
        }
        s.saved.zone_aliases.extend(new_aliases);
        let aliases = s.saved.zone_aliases.clone();
        if let Some(p) = s.saved.profile_mut() {
            p.zone = zone.clone();
            if let Some(i) = c.advance(p.step, &zone, &mode, &aliases) {
                p.step = i;
            }
        }
        push_event(s, "manual", format!("Локация сопоставлена: {zone}"), String::from("таблица соответствий"));
    });
}

/// Imports a Path of Building code: stores it as a normal local build and
/// makes it the active one. Only structural data; no guide text is copied.
#[tauri::command]
fn build_import_pob(app: AppHandle, state: State<App>, code: String) -> Result<builds::BuildDoc, String> {
    let xml = pob::decode(&code)?;
    let parsed = pob::parse_xml(&xml)?;
    let id = format!("pob-{}", new_id("b"));
    let json = pob::to_build_json(&parsed, &id);
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    let doc = builds::BuildDoc::parse(&text).map_err(|e| format!("Импортированный билд не прошёл проверку: {e}"))?;
    let dir = state.dir.join("builds");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Папка билдов: {e}"))?;
    std::fs::write(dir.join(format!("{id}.json")), &text).map_err(|e| format!("Запись билда: {e}"))?;
    let library = builds::load_library(Some(&dir));
    if let Ok(mut guard) = state.builds.write() {
        *guard = library;
    }
    state.update(&app, |s, _| {
        if let Some(p) = s.saved.profile_mut() {
            p.build_id = id.clone();
        }
        push_event(s, "manual", format!("Импорт PoB: {}", doc.name.ru), format!("{} · {} уровень", doc.class, parsed.level));
    });
    Ok(doc.clone())
}

#[tauri::command]
fn build_select(app: AppHandle, state: State<App>, id: String) {
    let exists = state.builds.read().map(|b| b.iter().any(|d| d.id == id)).unwrap_or(false);
    if !exists {
        return;
    }
    state.update(&app, |s, _| {
        if let Some(p) = s.saved.profile_mut() {
            p.build_id = id;
        }
    });
}

#[tauri::command]
fn build_toggle(app: AppHandle, state: State<App>, item: String) {
    state.update(&app, |s, _| {
        if let Some(p) = s.saved.profile_mut() {
            if let Some(i) = p.build_done.iter().position(|x| x == &item) {
                p.build_done.remove(i);
            } else {
                p.build_done.push(item);
            }
        }
    });
}

#[derive(Deserialize, Default)]
struct HistoryFilter {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[tauri::command]
fn history_list(state: State<App>, filter: HistoryFilter) -> Vec<HistoryEvent> {
    let s = state.snap.lock().unwrap();
    let query = filter.query.unwrap_or_default().trim().to_lowercase();
    let mut out: Vec<HistoryEvent> = s
        .saved
        .history
        .iter()
        .rev()
        .filter(|e| filter.kind.as_ref().map_or(true, |k| k.is_empty() || &e.kind == k))
        .filter(|e| filter.profile.as_ref().map_or(true, |p| p.is_empty() || &e.profile == p))
        .filter(|e| query.is_empty() || e.title.to_lowercase().contains(&query) || e.zone.to_lowercase().contains(&query) || e.detail.to_lowercase().contains(&query))
        .take(filter.limit.unwrap_or(500).min(HISTORY_LIMIT))
        .cloned()
        .collect();
    out.reverse();
    out
}

/// Writes the history as JSON next to the user's documents and returns the path.
#[tauri::command]
fn history_export(state: State<App>) -> Result<String, String> {
    let dir = documents_dir().join("Eni-PoE2-Companion");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Папка экспорта: {e}"))?;
    let path = dir.join(format!("history-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let s = state.snap.lock().unwrap();
    let doc = serde_json::json!({
        "app": "Eni-PoE2-Companion",
        "version": VERSION,
        "exported": now_iso(),
        "profile": s.saved.profile().map(|p| p.name.clone()).unwrap_or_default(),
        "events": &s.saved.history,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("Запись истории: {e}"))?;
    Ok(path.display().to_string())
}

fn documents_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Documents")
}

#[tauri::command]
fn shot_capture(app: AppHandle, state: State<App>, kind: String) -> Result<Shot, String> {
    let kind = normalize_shot_kind(&kind);
    state.capture(&app, &kind)
}

fn normalize_shot_kind(kind: &str) -> String {
    match kind {
        "item" | "inventory" | "passive" | "gems" => kind.to_string(),
        _ => "other".to_string(),
    }
}

#[tauri::command]
fn shots_list(state: State<App>) -> Vec<Shot> {
    let s = state.snap.lock().unwrap();
    let mut v = s.saved.shots.clone();
    v.sort_by(|a, b| b.ts.cmp(&a.ts));
    v
}

#[tauri::command]
fn shot_thumb(state: State<App>, id: String) -> Result<String, String> {
    let s = state.snap.lock().unwrap();
    let shot = s.saved.shots.iter().find(|x| x.id == id).ok_or("Скриншот не найден")?;
    App::store(&s).thumb_data_url(shot)
}

#[tauri::command]
fn shot_full(state: State<App>, id: String) -> Result<String, String> {
    let s = state.snap.lock().unwrap();
    let shot = s.saved.shots.iter().find(|x| x.id == id).ok_or("Скриншот не найден")?;
    App::store(&s).full_data_url(shot)
}

#[derive(Deserialize)]
struct ShotPatch {
    id: String,
    kind: String,
    note: String,
}

#[tauri::command]
fn shot_update(app: AppHandle, state: State<App>, patch: ShotPatch) {
    state.update(&app, |s, _| {
        if let Some(shot) = s.saved.shots.iter_mut().find(|x| x.id == patch.id) {
            shot.kind = normalize_shot_kind(&patch.kind);
            shot.note = patch.note.chars().take(500).collect();
        }
    });
}

#[tauri::command]
fn shot_delete(app: AppHandle, state: State<App>, id: String) {
    let removed = {
        let s = state.snap.lock().unwrap();
        s.saved.shots.iter().find(|x| x.id == id).cloned().map(|shot| (shot, App::store(&s)))
    };
    if let Some((shot, store)) = removed {
        store.delete(&shot);
        state.update(&app, |s, _| s.saved.shots.retain(|x| x.id != id));
    }
}

#[tauri::command]
fn shots_reveal(state: State<App>) -> Result<(), String> {
    let store = {
        let s = state.snap.lock().unwrap();
        App::store(&s)
    };
    store.reveal()
}

/// Native file picker for the game log. Runs off the main thread.
#[tauri::command]
async fn pick_log_file(app: AppHandle) -> Result<Option<String>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog().file().add_filter("Client.txt / LatestClient.txt", &["txt"]).blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()))
}

/// Native folder picker for the screenshot directory.
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog().file().blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()))
}

/// The log file the app currently follows, or where it looked.
#[derive(Serialize)]
struct LogInfo {
    path: Option<String>,
    found: bool,
    encoding: String,
    auto_found: Option<String>,
}

#[tauri::command]
fn log_info(app: AppHandle, state: State<App>) -> LogInfo {
    let snapshot = state.public();
    let _ = app;
    LogInfo {
        path: snapshot.log_path.map(|p| p.display().to_string()),
        found: snapshot.log_found,
        encoding: snapshot.log_encoding,
        auto_found: discover::find_client_log().map(|p| p.display().to_string()),
    }
}

/// Re-runs the automatic discovery (installed game, Steam libraries).
#[tauri::command]
fn log_auto(app: AppHandle, state: State<App>) -> LogInfo {
    let found = discover::find_client_log();
    {
        let mut shared = state.shared.lock().unwrap();
        shared.log = found.clone();
        shared.log_pinned = false;
    }
    state.update(&app, |s, _| {
        s.log_path = found.clone();
        s.log_found = found.as_ref().is_some_and(|p| p.is_file());
        s.log_error = if found.is_some() { None } else { Some("Журнал не найден автоматически".into()) };
    });
    let snapshot = state.public();
    LogInfo {
        path: snapshot.log_path.map(|p| p.display().to_string()),
        found: snapshot.log_found,
        encoding: snapshot.log_encoding,
        auto_found: found.map(|p| p.display().to_string()),
    }
}

/// Checks a hand-written log path and reports what the last lines say.
#[derive(Serialize)]
struct LogCheck {
    ok: bool,
    message: String,
    zone: Option<String>,
    level: Option<u32>,
    encoding: String,
}

#[tauri::command]
fn log_check(path: String) -> LogCheck {
    let p = PathBuf::from(path.trim());
    if let Err(e) = discover::validate_manual(&p) {
        return LogCheck { ok: false, message: e, zone: None, level: None, encoding: String::new() };
    }
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) => return LogCheck { ok: false, message: format!("Чтение: {e}"), zone: None, level: None, encoding: String::new() },
    };
    let (enc, skip) = logparse::sniff(&bytes[..bytes.len().min(4096)]);
    let start = bytes.len().saturating_sub(64 * 1024).max(skip).min(bytes.len());
    let chunk = &bytes[start..];
    let text = logparse::decode_line(chunk, enc);
    let mut zone = None;
    let mut level = None;
    for line in text.lines() {
        match logparse::parse(line) {
            Some(logparse::Event::Scene(z)) => zone = Some(z),
            Some(logparse::Event::Level { level: l, .. }) => level = Some(l),
            _ => {}
        }
    }
    LogCheck {
        ok: true,
        message: format!("Журнал читается ({}).", enc.label()),
        zone,
        level,
        encoding: enc.label().to_string(),
    }
}


#[tauri::command]
fn link_open(url: String) -> Result<(), String> {
    open_url(&url)
}

#[derive(Serialize)]
struct AiPrepared {
    json: String,
    path: String,
}

#[derive(Serialize)]
struct AiAnswerView {
    text: String,
    reasoning: Option<String>,
    model: String,
    provider: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    image_sent: bool,
    manifest: String,
}

/// Everything needed for an AI call, gathered under short-lived locks.
struct AiInput {
    request: ai::Request,
    key: String,
    images: Vec<Vec<u8>>,
    shots_dir: PathBuf,
    enabled: bool,
}

fn ai_input(state: &State<App>, ids: &[String], task: ai::Task, question: &str, item_text: Option<&str>) -> Result<AiInput, String> {
    let (cfg, shots, profile, lang, shots_dir) = {
        let s = state.snap.lock().unwrap();
        let shots: Vec<Shot> = s.saved.shots.iter().filter(|x| ids.contains(&x.id)).cloned().collect();
        let profile = s.saved.profile().cloned().ok_or("Нет активного профиля")?;
        (s.saved.ai.clone(), shots, profile, s.saved.lang.clone(), s.shots_dir.clone())
    };
    // The key is only decrypted when AI is enabled; the manifest itself needs no key.
    let key = match (cfg.enabled, cfg.key_protected.as_deref()) {
        (true, Some(blob)) => ai::unprotect_key(blob)?,
        _ => String::new(),
    };
    let (goal, next) = {
        let c = state.campaign.read().unwrap();
        let goal = c.step(profile.step).map(|st| st.goal.get(&lang).to_string()).unwrap_or_default();
        let next: Vec<String> = c.steps.iter().skip(profile.step + 1).take(3).map(|s| s.goal.get(&lang).to_string()).collect();
        (goal, next)
    };
    let request = {
        let builds = state.builds.read().unwrap();
        let build = builds
            .iter()
            .find(|b| b.id == profile.build_id)
            .or_else(|| builds.first())
            .ok_or("Нет загруженных билдов")?;
        ai::prepare(task, &cfg, &shots, &profile, build, &lang, question, &goal, &next, item_text)
    };
    // Only a vision model gets the pixels; text models get the description only.
    let mut images = Vec::new();
    if ai::model_supports_vision(&request.provider, &request.model) {
        for shot in &shots {
            if let Ok(bytes) = std::fs::read(shots_dir.join(&shot.file)) {
                images.push(bytes);
            }
        }
    }
    Ok(AiInput { request, key, images, shots_dir, enabled: cfg.enabled })
}

/// Network calls need an explicit, complete configuration.
fn ensure_ai_ready(input: &AiInput) -> Result<(), String> {
    if !input.enabled {
        return Err("ИИ выключен: включите его в настройках, выберите провайдера и сохраните ключ.".into());
    }
    if input.key.trim().is_empty() {
        return Err("Не задан API-ключ: вставьте ключ в настройках и сохраните.".into());
    }
    Ok(())
}

fn write_manifest(input: &AiInput) -> Result<String, String> {
    let json = serde_json::to_string_pretty(&input.request).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&input.shots_dir).map_err(|e| format!("Папка скриншотов: {e}"))?;
    let path = input.shots_dir.join(format!("ai-request-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::write(&path, &json).map_err(|e| format!("Запись описания запроса: {e}"))?;
    Ok(path.display().to_string())
}

#[tauri::command]
fn items_list(state: State<App>) -> Vec<ItemRecord> {
    let s = state.snap.lock().unwrap();
    let mut v = s.saved.items.clone();
    v.reverse();
    v
}

#[tauri::command]
fn item_import(app: AppHandle, state: State<App>) -> Result<ItemRecord, String> {
    state.import_item(&app)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StatsCache {
    fetched: String,
    stats: Vec<trade::StatDef>,
}

fn read_stats_cache(dir: &std::path::Path) -> Option<Vec<trade::StatDef>> {
    let path = dir.join("trade_stats.json");
    let age = std::fs::metadata(&path).ok()?.modified().ok()?.elapsed().ok()?;
    if age > std::time::Duration::from_secs(3 * 24 * 3600) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let cache: StatsCache = serde_json::from_str(&text).ok()?;
    (!cache.stats.is_empty()).then_some(cache.stats)
}

fn write_stats_cache(dir: &std::path::Path, stats: &[trade::StatDef]) {
    let cache = StatsCache { fetched: now_iso(), stats: stats.to_vec() };
    if let Ok(text) = serde_json::to_string(&cache) {
        let _ = std::fs::write(dir.join("trade_stats.json"), text);
    }
}

/// Price check for an imported item through the official trade API.
#[tauri::command]
async fn trade_price(state: State<'_, App>, id: String) -> Result<trade::PriceResult, String> {
    let (parsed, enabled, league, filters) = {
        let s = state.snap.lock().unwrap();
        let rec = s.saved.items.iter().find(|x| x.id == id).ok_or("Предмет не найден")?;
        (rec.parsed.clone(), s.saved.trade.enabled, state.preferred_league(), s.saved.trade.filters as usize)
    };
    if !enabled {
        return Err("Проверка цен выключена: включи её в настройках (официальный trade API).".into());
    }
    {
        let mut last = state.last_trade.lock().unwrap();
        if let Some(at) = *last {
            let elapsed = at.elapsed();
            if elapsed < std::time::Duration::from_secs(3) {
                return Err(format!("Не чаще раза в 3 секунды: подожди {} с.", 3 - elapsed.as_secs()));
            }
        }
        *last = Some(std::time::Instant::now());
    }
    // Stat filters need the dataset: memory, then the disk cache, then one request.
    let stats = {
        let cached = state.stats.lock().unwrap().clone();
        if cached.is_some() {
            cached
        } else {
            let disk = read_stats_cache(&state.dir);
            if disk.is_some() {
                *state.stats.lock().unwrap() = disk.clone();
            }
            disk
        }
    };
    let stats = match stats {
        Some(s) => Some(s),
        None => {
            let fetched = tauri::async_runtime::spawn_blocking(trade::fetch_stats)
                .await
                .ok()
                .and_then(|r| r.ok());
            if let Some(s) = &fetched {
                write_stats_cache(&state.dir, s);
                *state.stats.lock().unwrap() = Some(s.clone());
            }
            fetched
        }
    };
    tauri::async_runtime::spawn_blocking(move || trade::check(&parsed, &league, stats.as_deref(), filters))
        .await
        .map_err(|e| format!("Запрос прерван: {e}"))?
}

#[tauri::command]
fn item_delete(app: AppHandle, state: State<App>, id: String) {
    state.update(&app, |s, _| s.saved.items.retain(|x| x.id != id));
}

/// AI analysis over the imported item text (works with text-only models).
#[tauri::command]
async fn item_ai(app: AppHandle, state: State<'_, App>, id: String, question: String) -> Result<AiAnswerView, String> {
    let (raw, answer_label) = {
        let s = state.snap.lock().unwrap();
        let rec = s.saved.items.iter().find(|x| x.id == id).ok_or("Предмет не найден")?;
        (rec.raw.clone(), format!("{} · {}", rec.parsed.name, rec.parsed.base))
    };
    let input = ai_input(&state, &[], ai::Task::AnalyseItem, &question, Some(&raw))?;
    ensure_ai_ready(&input)?;
    let manifest = write_manifest(&input)?;
    let (request, key, images) = (input.request.clone(), input.key.clone(), input.images.clone());
    let lang = { state.snap.lock().unwrap().saved.lang.clone() };
    let answer = tauri::async_runtime::spawn_blocking(move || ai::complete(&request, &key, &lang, images))
        .await
        .map_err(|e| format!("Запрос ИИ прерван: {e}"))??;
    let summary = answer.text.chars().take(300).collect::<String>();
    let label = format!("ИИ по предмету: {answer_label}");
    state.update(&app, |s, _| {
        if let Some(rec) = s.saved.items.iter_mut().find(|x| x.id == id) {
            rec.ai = Some(summary);
        }
        push_event(s, "ai", label, answer.text.chars().take(160).collect());
    });
    Ok(AiAnswerView {
        text: answer.text,
        reasoning: answer.reasoning,
        model: answer.model,
        provider: answer.provider,
        prompt_tokens: answer.prompt_tokens,
        completion_tokens: answer.completion_tokens,
        image_sent: answer.image_sent,
        manifest,
    })
}

#[tauri::command]
fn quick_price_now(app: AppHandle, state: State<App>) {
    state.quick_price(&app);
}

#[tauri::command]
fn map_toggle_check(app: AppHandle, state: State<App>, name: String) {
    state.update(&app, |s, _| {
        if let Some(check) = s.map_run.checks.iter_mut().find(|c| c.name == name) {
            check.done = !check.done;
        }
    });
}

#[tauri::command]
fn map_rare(app: AppHandle, state: State<App>, delta: i32) {
    state.update(&app, |s, _| {
        s.map_run.rares = (s.map_run.rares as i32 + delta).max(0) as u32;
        s.map_run.open = true;
    });
}

#[tauri::command]
fn map_boss(app: AppHandle, state: State<App>) {
    state.update(&app, |s, _| {
        s.map_run.boss = !s.map_run.boss;
        s.map_run.open = true;
    });
}

#[tauri::command]
fn map_reset(app: AppHandle, state: State<App>) {
    state.update(&app, |s, _| {
        let mechanics = s.saved.map.mechanics.clone();
        let zone = s.map_run.zone.clone();
        s.map_run = MapRun::new(&zone, &mechanics);
        s.map_run.open = true;
    });
}

#[tauri::command]
fn map_panel(app: AppHandle, state: State<App>) {
    state.update(&app, |s, _| s.map_run.open = !s.map_run.open);
}

#[derive(Serialize)]
struct TradeSearchView {
    listings: Vec<trade::ListingView>,
    url: String,
}

fn read_scout_cache(dir: &std::path::Path) -> Option<scout::Scout> {
    let path = dir.join("scout.json");
    let age = std::fs::metadata(&path).ok()?.modified().ok()?.elapsed().ok()?;
    if age > std::time::Duration::from_secs(15 * 60) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_scout_cache(dir: &std::path::Path, value: &scout::Scout) {
    if let Ok(text) = serde_json::to_string(value) {
        let _ = std::fs::write(dir.join("scout.json"), text);
    }
}

impl App {
    /// League to price against: the setting, otherwise the current league from
    /// poe2scout (it knows `IsCurrent` better than the raw trade league list).
    fn preferred_league(&self) -> String {
        let saved = self.snap.lock().unwrap().saved.trade.league.trim().to_string();
        if !saved.is_empty() {
            return saved;
        }
        if let Some((_, value)) = self.scout.lock().unwrap().as_ref() {
            return value.league.clone();
        }
        String::new()
    }
}

#[tauri::command]
async fn trade_rate(state: State<'_, App>) -> Result<trade::CurrencyRate, String> {
    let (enabled, league) = {
        let s = state.snap.lock().unwrap();
        (s.saved.trade.enabled, s.saved.trade.league.clone())
    };
    if !enabled {
        return Err("Проверка цен выключена: включи её в настройках.".into());
    }
    if let Some((at, rate)) = state.rates.lock().unwrap().clone() {
        if at.elapsed() < std::time::Duration::from_secs(300) {
            return Ok(rate);
        }
    }
    // 1) poe2scout: 24h average and the current league.
    let cached = {
        let memory = state.scout.lock().unwrap().clone();
        match memory {
            Some(pair) if pair.0.elapsed() < std::time::Duration::from_secs(15 * 60) => Some(pair.1),
            _ => {
                let disk = read_scout_cache(&state.dir);
                if let Some(value) = &disk {
                    *state.scout.lock().unwrap() = Some((std::time::Instant::now(), value.clone()));
                }
                disk
            }
        }
    };
    let scout_value = match cached {
        Some(value) => Some(value),
        None => {
            let fetched = tauri::async_runtime::spawn_blocking(scout::fetch).await.ok().and_then(|r| r.ok());
            if let Some(value) = &fetched {
                write_scout_cache(&state.dir, value);
                *state.scout.lock().unwrap() = Some((std::time::Instant::now(), value.clone()));
            }
            fetched
        }
    };
    if let Some(value) = scout_value.filter(|v| v.divine_price > 0.0) {
        let rate = trade::CurrencyRate {
            league: value.league,
            exalted_per_divine: value.divine_price,
            offers: 0,
            source: value.source,
            updated: value.fetched,
        };
        *state.rates.lock().unwrap() = Some((std::time::Instant::now(), rate.clone()));
        return Ok(rate);
    }
    // 2) Official exchange / priced listings.
    let league = state.preferred_league();
    let rate = tauri::async_runtime::spawn_blocking(move || trade::currency_rate(&league))
        .await
        .map_err(|e| format!("Запрос прерван: {e}"))??;
    *state.rates.lock().unwrap() = Some((std::time::Instant::now(), rate.clone()));
    Ok(rate)
}

#[tauri::command]
async fn trade_search(state: State<'_, App>, query: String) -> Result<TradeSearchView, String> {
    let enabled = {
        let s = state.snap.lock().unwrap();
        s.saved.trade.enabled
    };
    let league = state.preferred_league();
    if !enabled {
        return Err("Торговля выключена: включи проверку цен в настройках.".into());
    }
    {
        let mut last = state.last_trade.lock().unwrap();
        if let Some(at) = *last {
            let elapsed = at.elapsed();
            if elapsed < std::time::Duration::from_secs(3) {
                return Err(format!("Не чаще раза в 3 секунды: подожди {} с.", 3 - elapsed.as_secs()));
            }
        }
        *last = Some(std::time::Instant::now());
    }
    tauri::async_runtime::spawn_blocking(move || trade::search_text(&query, &league))
        .await
        .map_err(|e| format!("Запрос прерван: {e}"))?
        .map(|(listings, url)| TradeSearchView { listings, url })
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    clip::copy(&text).map_err(|e| e.to_string())
}

#[tauri::command]
fn regex_toggle(app: AppHandle, state: State<App>) {
    state.toggle_regex(&app);
}

#[tauri::command]
fn copy_regex(app: AppHandle, state: State<App>, value: String) -> Result<(), String> {
    clip::copy(&value).map_err(|e| e.to_string())?;
    {
        let mut r = state.regex_open.lock().unwrap();
        *r = false;
    }
    state.shared.lock().unwrap().regex_open = false;
    state.update(&app, |s, _| s.regex_open = false);
    Ok(())
}

/// Writes the AI request description next to the screenshots. Nothing is sent.
#[tauri::command]
fn ai_prepare(app: AppHandle, state: State<App>, ids: Vec<String>, task: String) -> Result<AiPrepared, String> {
    let input = ai_input(&state, &ids, ai::Task::parse(&task), "", None)?;
    let path = write_manifest(&input)?;
    let json = serde_json::to_string_pretty(&input.request).map_err(|e| e.to_string())?;
    let _ = app.emit("state", state.public());
    Ok(AiPrepared { json, path })
}

/// Sends the request. Explicit user action only: the button in the screenshot view.
#[tauri::command]
async fn ai_analyze(app: AppHandle, state: State<'_, App>, ids: Vec<String>, task: String, question: String) -> Result<AiAnswerView, String> {
    let input = ai_input(&state, &ids, ai::Task::parse(&task), &question, None)?;
    ensure_ai_ready(&input)?;
    let manifest = write_manifest(&input)?;
    let (request, key, images) = (input.request.clone(), input.key.clone(), input.images.clone());
    let lang = { state.snap.lock().unwrap().saved.lang.clone() };
    let answer = tauri::async_runtime::spawn_blocking(move || ai::complete(&request, &key, &lang, images))
        .await
        .map_err(|e| format!("Запрос ИИ прерван: {e}"))??;
    let summary = answer.text.chars().take(160).collect::<String>();
    let label = format!("ИИ: {} · {}", answer.model, answer.provider);
    state.update(&app, |s, _| push_event(s, "ai", label, summary));
    Ok(AiAnswerView {
        text: answer.text,
        reasoning: answer.reasoning,
        model: answer.model,
        provider: answer.provider,
        prompt_tokens: answer.prompt_tokens,
        completion_tokens: answer.completion_tokens,
        image_sent: answer.image_sent,
        manifest,
    })
}

/// Round-trip check from the settings page: one short message, no images.
#[tauri::command]
async fn ai_test(state: State<'_, App>) -> Result<String, String> {
    let input = ai_input(&state, &[], ai::Task::Answer, "Ответь одним словом: работает", None)?;
    ensure_ai_ready(&input)?;
    let (request, key) = (input.request.clone(), input.key.clone());
    let lang = { state.snap.lock().unwrap().saved.lang.clone() };
    let answer = tauri::async_runtime::spawn_blocking(move || ai::complete(&request, &key, &lang, Vec::new()))
        .await
        .map_err(|e| format!("Запрос ИИ прерван: {e}"))??;
    Ok(format!("{} · {} · {} символов", answer.provider, answer.model, answer.text.chars().count()))
}

#[tauri::command]
fn diagnostics(state: State<App>) -> String {
    // One lock at a time: the log tail thread updates the snapshot while this
    // runs, and a nested reverse order would deadlock the whole app.
    let (campaign_version, campaign_steps, campaign_acts) = {
        let c = state.campaign.read().unwrap();
        (c.version.clone(), c.len(), c.acts.len())
    };
    let builds: Vec<String> = { state.builds.read().unwrap().iter().map(|b| b.id.clone()).collect() };
    let (log_pinned, game_rect, overlay_visible) = {
        let sh = state.shared.lock().unwrap();
        (sh.log_pinned, sh.game, sh.visible)
    };
    let s = state.snap.lock().unwrap();
    let mut out = String::new();
    out.push_str(&format!("Eni-PoE2-Companion {VERSION}\n"));
    out.push_str(&format!("data: {}\n", state.dir.display()));
    out.push_str(&format!("screenshots: {}\n", s.shots_dir.display()));
    out.push_str(&format!("state.json: {}\n", state.dir.join("state.json").display()));
    out.push_str(&format!("log: {} ({})\n", s.log_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "не найден".into()), s.log_encoding));
    out.push_str(&format!("log pinned: {log_pinned}\n"));
    out.push_str(&format!("game window: {} ({})\n", if game_rect.is_some() { "найдено" } else { "нет" }, game::game_title()));
    out.push_str(&format!("overlay visible: {overlay_visible}\n"));
    out.push_str(&format!("campaign: {campaign_version} ({campaign_steps} шагов, {campaign_acts} актов)\n"));
    out.push_str(&format!("builds: {builds:?}\n"));
    out.push_str(&format!("profiles: {} (active {})\n", s.saved.profiles.len(), s.saved.active));
    out.push_str(&format!("history: {} events\n", s.saved.history.len()));
    out.push_str(&format!("screenshots: {} entries\n", s.saved.shots.len()));
    out.push_str(&format!("items: {} imported, {} regexes\n", s.saved.items.len(), s.saved.regexes.len()));
    out.push_str(&format!("map: checklist={} mechanics={}\n", s.saved.map.enabled, s.saved.map.mechanics.len()));
    out.push_str(&format!("trade: enabled={} league={} stats_cached={}\n", s.saved.trade.enabled, if s.saved.trade.league.is_empty() { "auto" } else { s.saved.trade.league.as_str() }, s.data_dir.join("trade_stats.json").is_file()));
    out.push_str(&format!("hotkey error: {}\n", s.hotkey_error.clone().unwrap_or_else(|| "нет".into())));
    out.push_str(&format!("zone unknown: {}\n", s.zone_unknown));
    out.push_str(&format!("area id: {}\n", s.area_id));
    if let Some(g) = game_rect {
        out.push_str(&format!("game client area: {},{} {}x{}\n", g.left, g.top, g.right - g.left, g.bottom - g.top));
    }
    out.push_str(&format!("ai: provider={} enabled={} key={}\n", s.saved.ai.provider, s.saved.ai.enabled, s.ai_has_key));
    out
}

#[tauri::command]
fn reveal_data_dir(state: State<App>) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(state.dir.as_os_str())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Не удалось открыть проводник: {e}"))
}

#[tauri::command]
fn show_main_window(app: AppHandle) {
    show_main(&app);
}

#[tauri::command]
fn overlay_toggle(app: AppHandle, state: State<App>) {
    state.toggle_overlay(&app);
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// The window follows the panel's rendered size (CSS pixels from the page).
#[tauri::command]
fn panel_size(window: WebviewWindow, app: AppHandle, state: State<App>, w: f64, h: f64) {
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return;
    }
    let k = window.scale_factor().unwrap_or(1.0);
    let (w, h) = ((w * k).ceil() as i32, (h * k).ceil() as i32);
    let moved = {
        let mut s = state.shared.lock().unwrap();
        let old = s.panel.h;
        let y = s.panel.y;
        let lower = s.game.is_some_and(|g| old > 0 && y + old / 2 > (g.bottom - g.top) / 2);
        if lower {
            s.panel.y -= h - old;
        }
        (s.panel.w, s.panel.h) = (w, h);
        game::place(&window, &mut s);
        (lower && h != old).then_some((s.panel.x, s.panel.y))
    };
    if let Some((x, y)) = moved {
        state.update(&app, |s, _| {
            s.saved.overlay.x = x;
            s.saved.overlay.y = y;
        });
    }
}

/// phase: "start" | "move" | "end". The offset is saved on "end".
#[tauri::command]
fn drag(window: WebviewWindow, app: AppHandle, state: State<App>, phase: String) {
    let done = game::drag(&window, &mut state.shared.lock().unwrap(), &phase);
    if let Some((x, y)) = done {
        state.update(&app, |s, _| {
            s.saved.overlay.x = x;
            s.saved.overlay.y = y;
        });
    }
}

// ---------------------------------------------------------------------------
// App startup
// ---------------------------------------------------------------------------

fn main() {    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|handle, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let action = handle.state::<App>().bindings.lock().unwrap().iter().find(|(s, _)| s == shortcut).map(|(_, a)| *a);
                    // Runs on the main thread under the plugin's lock: actions must not
                    // (un)register hotkeys. Only startup and save_settings do that.
                    if let Some(a) = action {
                        handle.state::<App>().action(handle, a);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            snapshot,
            campaign_data,
            builds_list,
            save_settings,
            save_overlay,
            preview_look,
            profile_new,
            profile_select,
            profile_save,
            profile_delete,
            campaign_step,
            campaign_goto,
            campaign_complete,
            campaign_manual_zone,
            zone_map,
            build_select,
            build_import_pob,
            build_import_url,
            guide_for_build,
            trade_search_guide,
            build_toggle,
            history_list,
            history_export,
            shot_capture,
            shots_list,
            shot_thumb,
            shot_full,
            shot_update,
            shot_delete,
            shots_reveal,
            log_info,
            log_auto,
            log_check,
            pick_log_file,
            pick_folder,
            copy_text,
            link_open,
            ai_prepare,
            ai_analyze,
            ai_test,
            items_list,
            item_import,
            item_delete,
            item_ai,
            trade_price,
            regex_toggle,
            copy_regex,
            map_toggle_check,
            map_rare,
            map_boss,
            map_reset,
            map_panel,
            quick_price_now,
            trade_search,
            trade_rate,
            diagnostics,
            reveal_data_dir,
            show_main_window,
            overlay_toggle,
            quit_app,
            panel_size,
            drag
        ])
        .on_window_event(|window, event| {
            // Closing the companion window keeps the overlay and the tray alive.
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                    // Hand the keyboard back to the game instead of the desktop.
                    let hwnd = window.app_handle().state::<App>().shared.lock().unwrap().game_hwnd;
                    game::focus_game(hwnd);
                }
            }
        })
        .setup(|tauri_app| {
            let handle = tauri_app.handle().clone();
            let dir = handle.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let saved = model::load_state(&dir.join("state.json"));
            let shots_dir = saved.shots_dir.clone().unwrap_or_else(screens::default_shots_dir);
            let campaign = campaign::Campaign::parse(campaign::BUNDLED).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let build_dir = dir.join("builds");
            let build_docs = builds::load_library(Some(&build_dir));
            let pinned = saved.log.mode == "manual" && saved.log.path.as_ref().is_some_and(|p| p.is_file());
            let log = if pinned { saved.log.path.clone() } else { discover::find_client_log() };
            let shared = Arc::new(Mutex::new(game::Shared {
                panel: game::Panel { x: saved.overlay.x, y: saved.overlay.y, ..Default::default() },
                log_pinned: pinned,
                log: log.clone(),
                user_hidden: !saved.overlay.visible,
                only_in_game: saved.overlay.only_in_game,
                ..Default::default()
            }));
            tauri_app.manage(App {
                snap: Mutex::new(Snapshot {
                    saved,
                    game_running: false,
                    log_found: log.as_ref().is_some_and(|p| p.is_file()),
                    log_path: log,
                    log_encoding: String::new(),
                    log_error: None,
                    zone_unknown: false,
                    area_level: 0,
                    area_id: String::new(),
                    hotkey_error: None,
                    campaign_len: campaign.len(),
                    campaign_act: 1,
                    campaign_version: campaign.version.clone(),
                    shots_dir: shots_dir.clone(),
                    data_dir: dir.clone(),
                    build_ids: build_docs.iter().map(|b| b.id.clone()).collect(),
                    ai_has_key: false,
                    ai_providers: ai::PROVIDERS.to_vec(),
                    items_count: 0,
                    regexes: Vec::new(),
                    regex_open: false,
                    last_item: None,
                    quick_price: None,
                    map_run: MapRun::default(),
                    session_started: now_iso(),
                    session_deaths: 0,
                    session_zones: 0,
                    session_levels: 0,
                    zone_since: String::new(),
                    version: VERSION.to_string(),
                }),
                campaign: RwLock::new(campaign),
                builds: RwLock::new(build_docs),
                shared: shared.clone(),
                bindings: Mutex::new(Vec::new()),
                failed: Mutex::new(Vec::new()),
                dir: dir.clone(),
                last_area: Mutex::new(String::new()),
                regex_open: Mutex::new(false),
                last_trade: Mutex::new(None),
                stats: Mutex::new(None),
                rates: Mutex::new(None),
                scout: Mutex::new(None),
            });
            // Windows are created only now ("create": false in tauri.conf.json):
            // their pages call commands right away, and those need the state above.
            for cfg in tauri_app.config().app.windows.clone() {
                tauri::WebviewWindowBuilder::from_config(tauri_app.handle(), &cfg)?.build()?;
            }
            if let Some(main) = tauri_app.get_webview_window("main") {
                let hwnd = main.hwnd().map_or(0, |h| h.0 as isize);
                handle.state::<App>().shared.lock().unwrap().main_hwnd = hwnd;
            }
            // Test knob: keep the window hidden while the pages still load.
            if std::env::var("ENI_COMPANION_HEADLESS").is_err() {
                show_main(&handle);
            }
            if let Err(e) = handle.state::<App>().register_hotkeys(&handle) {
                eprintln!("hotkeys: {e}");
            }
            let h = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(5));
                if !h.state::<App>().failed.lock().unwrap().is_empty() {
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || h2.state::<App>().retry_failed(&h2));
                }
            });
            let (h, sh) = (handle.clone(), shared.clone());
            tail::spawn(move || sh.lock().unwrap().log.clone(), move |e| h.state::<App>().on_log(&h, e));
            if let Some(overlay) = tauri_app.get_webview_window("overlay") {
                game::spawn_tracker(overlay, shared);
            }
            let open_item = MenuItem::with_id(tauri_app, "open", "Открыть Eni-PoE2-Companion", true, None::<&str>)?;
            let overlay_item = MenuItem::with_id(tauri_app, "overlay", "Оверлей вкл/выкл", true, None::<&str>)?;
            let shot_item = MenuItem::with_id(tauri_app, "shot", "Скриншот", true, None::<&str>)?;
            let next_item = MenuItem::with_id(tauri_app, "next", "Следующий шаг", true, None::<&str>)?;
            let quit = MenuItem::with_id(tauri_app, "quit", "Выход", true, None::<&str>)?;
            TrayIconBuilder::new()
                .icon(tauri_app.default_window_icon().unwrap().clone())
                .tooltip("Eni-PoE2-Companion")
                .menu(&Menu::with_items(tauri_app, &[&open_item, &overlay_item, &shot_item, &next_item, &quit])?)
                .on_menu_event(|handle, event| match event.id.as_ref() {
                    "open" => show_main(handle),
                    "overlay" => handle.state::<App>().toggle_overlay(handle),
                    "shot" => {
                        if let Err(e) = handle.state::<App>().capture(handle, "other") {
                            eprintln!("screenshot: {e}");
                        }
                    }
                    "next" => handle.state::<App>().step(handle, 1),
                    "quit" => handle.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(tauri_app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_summary_counts_progress() {
        let mut run = MapRun::new("Охотничьи угодья", &["Breach".to_string()]);
        run.rares = 3;
        run.boss = true;
        run.checks[0].done = true;
        let summary = run.summary();
        assert!(summary.contains("3 редких"), "{summary}");
        assert!(summary.contains("босс: да"), "{summary}");
        assert!(summary.contains("1/1"), "{summary}");
        assert!(run.active());
        assert!(!MapRun::new("x", &[]).active());
    }

    #[test]
    fn hotkeys_parse_and_reject_duplicates() {
        let mut h = Hotkeys::default();
        assert_eq!(parse_hotkeys(&h).unwrap().len(), 10);
        h.screenshot = h.next.clone();
        assert!(parse_hotkeys(&h).unwrap_err().contains("занято"));
        h.screenshot = "Ctrl+Nonsense".into();
        assert!(parse_hotkeys(&h).is_err());
        h.screenshot = String::new();
        assert_eq!(parse_hotkeys(&h).unwrap().len(), 9, "an empty field is simply unused");
    }

    #[test]
    fn class_names_are_canonicalized() {
        assert_eq!(canonical_class("Ranger"), "Ranger");
        assert_eq!(canonical_class("Лучница"), "Ranger");
        assert_eq!(canonical_class("ЛУЧНИЦА"), "Ranger");
        assert_eq!(canonical_class("Witch"), "Witch");
        assert_eq!(canonical_class("Homebrew"), "Homebrew");
    }

    #[test]
    fn only_trusted_https_links_open() {
        assert!(open_url("http://example.com").is_err());
        assert!(open_url("https://evil.example/x").is_err());
        assert!(open_url("https://user@evil.example").is_err());
    }

    #[test]
    fn shot_kinds_are_whitelisted() {
        assert_eq!(normalize_shot_kind("item"), "item");
        assert_eq!(normalize_shot_kind("passive"), "passive");
        assert_eq!(normalize_shot_kind("../../etc"), "other");
    }
}

