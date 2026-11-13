//! Persisted data model: settings, profiles, history, screenshots, AI config.
//!
//! Everything lives in one `state.json` in the app data directory. Writing is
//! atomic (temp file + rename) and a file that does not parse is kept as
//! `state.corrupt-<timestamp>.json` instead of being silently overwritten.

use crate::guide::{GuideGear, GuideSkill};
use crate::item::{Analysis, ParsedItem};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const SCHEMA: u32 = 1;
/// Older events fall off the end; the history stays a small local file.
pub const HISTORY_LIMIT: usize = 5000;

/// A string in both interface languages. Russian is the primary language.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct L10n {
    pub ru: String,
    pub en: String,
}

impl L10n {
    pub fn get(&self, lang: &str) -> &str {
        if lang == "en" {
            &self.en
        } else {
            &self.ru
        }
    }
}

/// Local time, second precision, with the UTC offset: `2026-11-12T21:35:27+03:00`.
pub fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Process-unique, sortable id: `p-018f...` style without extra dependencies.
pub fn new_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{:x}{:02x}", now_ms(), n & 0xff)
}

/// Writes through a temp file so a crash mid-write cannot truncate the state.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    // rename over an existing file works on Windows for files (not directories).
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Hotkeys {
    /// Next / previous campaign step.
    pub next: String,
    pub prev: String,
    /// Cycle overlay mode: compact -> standard -> extended.
    pub mode: String,
    /// Show / hide the overlay.
    pub hide: String,
    /// Take a screenshot of the game window.
    pub screenshot: String,
    /// Open the main window.
    pub main: String,
    /// Import the item under the cursor from the clipboard.
    pub item: String,
    /// Show the vendor/stash regex list in the overlay.
    pub regex: String,
    /// Toggle the map checklist in the overlay.
    pub map: String,
    /// Price the item in the clipboard right away.
    pub price: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys {
            next: "Ctrl+Alt+ArrowRight".into(),
            prev: "Ctrl+Alt+ArrowLeft".into(),
            mode: "Ctrl+Alt+KeyO".into(),
            hide: "Ctrl+Alt+KeyG".into(),
            screenshot: "Ctrl+Alt+KeyS".into(),
            main: "Shift+Space".into(),
            item: "Ctrl+Alt+KeyI".into(),
            regex: "Ctrl+Alt+KeyR".into(),
            map: "Ctrl+Alt+KeyJ".into(),
            price: "Ctrl+Alt+KeyP".into(),
        }
    }
}

impl Hotkeys {
    pub fn entries(&self) -> [(&'static str, &String); 10] {
        [
            ("next", &self.next),
            ("prev", &self.prev),
            ("mode", &self.mode),
            ("hide", &self.hide),
            ("screenshot", &self.screenshot),
            ("main", &self.main),
            ("item", &self.item),
            ("regex", &self.regex),
            ("map", &self.map),
            ("price", &self.price),
        ]
    }
}

/// Overlay look and placement. `x`/`y` are physical pixels inside the game's
/// client area, `scale` and `opacity` are percentages.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct OverlayCfg {
    /// "compact" | "standard" | "extended"
    pub mode: String,
    pub x: i32,
    pub y: i32,
    pub opacity: u32,
    pub scale: u32,
    pub visible: bool,
    /// Hide the overlay while the game is not in front (it never steals focus).
    pub only_in_game: bool,
}

impl Default for OverlayCfg {
    fn default() -> Self {
        OverlayCfg {
            mode: "standard".into(),
            x: 24,
            y: 160,
            opacity: 88,
            scale: 100,
            visible: true,
            only_in_game: true,
        }
    }
}

impl OverlayCfg {
    pub fn clamped(mut self) -> Self {
        if !matches!(self.mode.as_str(), "compact" | "standard" | "extended") {
            self.mode = "standard".into();
        }
        self.opacity = self.opacity.min(100);
        self.scale = self.scale.clamp(60, 200);
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct LogCfg {
    /// "auto" (find it) | "manual" (use `path`)
    pub mode: String,
    pub path: Option<PathBuf>,
}

impl Default for LogCfg {
    fn default() -> Self {
        LogCfg { mode: "auto".into(), path: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RewardRec {
    pub ts: String,
    pub kind: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LevelRec {
    pub ts: String,
    pub level: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ZoneAlias {
    /// What the log writes, e.g. a localized zone name.
    pub alias: String,
    /// Canonical English zone name used by the campaign route.
    pub zone: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// The name came from a `is now level` line, not from typing.
    pub name_auto: bool,
    pub class: String,
    pub ascendancy: String,
    pub level: u32,
    pub act: u32,
    pub zone: String,
    /// Index into the campaign route.
    pub step: usize,
    pub rewards: Vec<RewardRec>,
    pub levels: Vec<LevelRec>,
    /// Campaign step ids marked as done by hand.
    pub done: Vec<String>,
    pub build_id: String,
    /// Build checklist item ids that are ticked.
    pub build_done: Vec<String>,
    pub created: String,
    pub updated: String,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            id: new_id("p"),
            name: String::new(),
            name_auto: false,
            class: "Ranger".into(),
            ascendancy: "Deadeye".into(),
            level: 1,
            act: 1,
            zone: String::new(),
            step: 0,
            rewards: Vec::new(),
            levels: Vec::new(),
            done: Vec::new(),
            build_id: "lightning-arrow-deadeye".into(),
            build_done: Vec::new(),
            created: now_iso(),
            updated: now_iso(),
        }
    }
}

/// The starting profile the task asks for: Ranger/Deadeye, Lightning Arrow,
/// level 6, act one.
pub fn starter_profile(lang: &str) -> Profile {
    let mut p = Profile { level: 6, act: 1, ..Default::default() };
    p.name = if lang == "en" { "Eni".into() } else { "Эни".into() };
    p.zone = "The Grim Tangle".into();
    // Level 6 is step 8 of the route (The Grim Tangle, level 5-6).
    p.step = 7;
    p.levels = vec![LevelRec { ts: now_iso(), level: 6 }];
    p
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HistoryEvent {
    pub id: String,
    pub ts: String,
    pub profile: String,
    /// "zone" | "level" | "reward" | "task" | "act" | "shot" | "manual" | "log"
    pub kind: String,
    pub level: u32,
    pub zone: String,
    pub act: u32,
    pub title: String,
    pub detail: String,
}

/// An item imported from the clipboard, with its local analysis.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ItemRecord {
    pub id: String,
    pub ts: String,
    pub profile: String,
    /// Raw clipboard text, capped when stored.
    pub raw: String,
    pub parsed: ParsedItem,
    pub analysis: Analysis,
    /// Dangerous map/waystone modifiers found in the text.
    #[serde(default)]
    pub dangers: Vec<crate::item::Danger>,
    /// Last AI answer for this item, if the user asked one.
    #[serde(default)]
    pub ai: Option<String>,
}

/// A build guide imported from Mobalytics: structural facts only.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct GuideRecord {
    pub id: String,
    pub build_id: String,
    pub ts: String,
    pub url: String,
    pub title: String,
    pub class: String,
    pub ascendancy: String,
    pub skills: Vec<GuideSkill>,
    pub gear: Vec<GuideGear>,
}

/// A vendor/stash search regex the player keeps at hand.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct RegexItem {
    pub name: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Shot {
    pub id: String,
    pub ts: String,
    pub profile: String,
    /// "item" | "inventory" | "passive" | "gems" | "other"
    pub kind: String,
    /// File names inside the screenshots directory.
    pub file: String,
    pub thumb: String,
    pub w: u32,
    pub h: u32,
    pub sha256: String,
    pub note: String,
    pub zone: String,
    pub level: u32,
}

/// Checklist of the current location: what is left to clear before moving on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct MapCfg {
    /// Open the checklist automatically when a new area is entered.
    pub enabled: bool,
    /// Mechanics to tick off (edited in the settings).
    pub mechanics: Vec<String>,
}

impl Default for MapCfg {
    fn default() -> Self {
        MapCfg {
            enabled: false,
            mechanics: default_map_mechanics(),
        }
    }
}

pub fn default_map_mechanics() -> Vec<String> {
    ["Breach", "Ritual", "Expedition", "Delirium", "Tower"].iter().map(|s| s.to_string()).collect()
}

/// Price check through the official trade API (opt-in, one click per request).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TradeCfg {
    pub enabled: bool,
    /// Empty means "detect the current challenge league".
    pub league: String,
    /// How many modifiers to use as filters (0-3); 2 is a good default.
    #[serde(default = "default_trade_filters")]
    pub filters: u8,
}

pub fn default_trade_filters() -> u8 {
    2
}

impl Default for TradeCfg {
    fn default() -> Self {
        TradeCfg { enabled: false, league: String::new(), filters: 2 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AiCfg {
    pub enabled: bool,
    /// "disabled" | "openai" | "deepseek"
    pub provider: String,
    pub model: String,
    /// "auto" | "low" | "medium" | "high" — sent as `reasoning_effort`.
    pub reasoning: String,
    /// DPAPI-protected key, base64. Never plain text, never in the repo.
    pub key_protected: Option<String>,
}

impl Default for AiCfg {
    fn default() -> Self {
        AiCfg { enabled: false, provider: "disabled".into(), model: String::new(), reasoning: "auto".into(), key_protected: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Saved {
    pub schema: u32,
    /// "ru" | "en"
    pub lang: String,
    pub log: LogCfg,
    pub overlay: OverlayCfg,
    pub hotkeys: Hotkeys,
    /// "all" | "must"
    pub route: String,
    pub profiles: Vec<Profile>,
    pub active: String,
    pub history: Vec<HistoryEvent>,
    pub shots: Vec<Shot>,
    /// Items imported from the clipboard (newest last).
    #[serde(default)]
    pub items: Vec<ItemRecord>,
    /// Vendor/stash regexes shown by the regex hotkey.
    #[serde(default = "default_regexes")]
    pub regexes: Vec<RegexItem>,
    /// Imported build guides (Mobalytics).
    #[serde(default)]
    pub guides: Vec<GuideRecord>,
    pub buildings_dir: Option<PathBuf>,
    pub shots_dir: Option<PathBuf>,
    pub ai: AiCfg,
    #[serde(default)]
    pub trade: TradeCfg,
    #[serde(default)]
    pub map: MapCfg,
    #[serde(default = "default_aliases")]
    pub zone_aliases: Vec<ZoneAlias>,
}

impl Default for Saved {
    fn default() -> Self {
        let profile = starter_profile("ru");
        Saved {
            schema: SCHEMA,
            lang: "ru".into(),
            log: LogCfg::default(),
            overlay: OverlayCfg::default(),
            hotkeys: Hotkeys::default(),
            route: "all".into(),
            active: profile.id.clone(),
            profiles: vec![profile],
            history: Vec::new(),
            shots: Vec::new(),
            items: Vec::new(),
            regexes: default_regexes(),
            guides: Vec::new(),
            buildings_dir: None,
            shots_dir: None,
            ai: AiCfg::default(),
            trade: TradeCfg::default(),
            map: MapCfg::default(),
            zone_aliases: default_aliases(),
        }
    }
}

/// Zone names of a localized client, verified against a real Russian PoE2 log,
/// plus two area ids so a renamed zone still resolves.
///
/// The area id is the language-independent fallback: `G1_1` is The Riverbank,
/// `G1_town` is the act 1 town. Users add more in the settings; the app also
/// offers to map an unknown zone right when it first appears.
/// Starter regexes for the vendor/stash search box (short, editable).
pub fn default_regexes() -> Vec<RegexItem> {
    vec![
        RegexItem { name: "Сопротивления".into(), value: r#""[egdl] to.at|resistance|сопротив""#.into() },
        RegexItem { name: "Жизнь и мана".into(), value: r#""life|mana|здоров|мана""#.into() },
        RegexItem { name: "Скорость бега".into(), value: r#""movement speed|скорость передвижения""#.into() },
        RegexItem { name: "Камни и руны".into(), value: r#""gem|rune|камень|руна""#.into() },
    ]
}

pub fn default_aliases() -> Vec<ZoneAlias> {
    vec![
        ZoneAlias { alias: "Берег реки".into(), zone: "The Riverbank".into() },
        ZoneAlias { alias: "Лагерь Клирфелл".into(), zone: "Clearfell Encampment".into() },
        ZoneAlias { alias: "Клирфелл".into(), zone: "Clearfell".into() },
        ZoneAlias { alias: "Красная Долина".into(), zone: "The Red Vale".into() },
        ZoneAlias { alias: "G1_1".into(), zone: "The Riverbank".into() },
        ZoneAlias { alias: "G1_town".into(), zone: "Clearfell Encampment".into() },
    ]
}

impl Saved {
    /// Drops fields that a hand-edited or corrupted file could push out of range.
    pub fn sanitized(mut self) -> Self {
        self.schema = SCHEMA;
        if self.lang != "en" {
            self.lang = "ru".into();
        }
        if !matches!(self.route.as_str(), "all" | "must") {
            self.route = "all".into();
        }
        self.overlay = self.overlay.clone().clamped();
        if self.profiles.is_empty() {
            let p = starter_profile(&self.lang);
            self.active = p.id.clone();
            self.profiles.push(p);
        }
        if !self.profiles.iter().any(|p| p.id == self.active) {
            self.active = self.profiles[0].id.clone();
        }
        for p in &mut self.profiles {
            p.level = p.level.clamp(1, 100);
            p.act = p.act.clamp(1, 6);
            p.levels.retain(|l| l.level > 0 && l.level <= 100);
            if p.levels.len() > 200 {
                let keep = p.levels.len() - 200;
                p.levels.drain(..keep);
            }
            if p.rewards.len() > 200 {
                let keep = p.rewards.len() - 200;
                p.rewards.drain(..keep);
            }
        }
        self.ai.reasoning = match self.ai.reasoning.as_str() {
            "low" | "medium" | "high" => self.ai.reasoning.clone(),
            _ => "auto".into(),
        };
        if self.history.len() > HISTORY_LIMIT {
            let keep = self.history.len() - HISTORY_LIMIT;
            self.history.drain(..keep);
        }
        self.shots.retain(|s| !s.id.is_empty() && !s.file.is_empty() && !s.file.contains("..") && !s.file.contains(['/', '\\']));
        for item in &mut self.items {
            if item.raw.len() > 8000 {
                item.raw.truncate(8000);
            }
            item.parsed.mods.truncate(24);
        }
        if self.items.len() > 200 {
            let keep = self.items.len() - 200;
            self.items.drain(..keep);
        }
        self.regexes.retain(|r| !r.value.trim().is_empty());
        for r in &mut self.regexes {
            r.name = r.name.chars().take(40).collect();
            r.value = r.value.chars().take(400).collect();
        }
        if self.regexes.len() > 100 {
            self.regexes.truncate(100);
        }
        for g in &mut self.guides {
            g.title = g.title.chars().take(120).collect();
            g.skills.truncate(12);
            g.gear.truncate(24);
            for skill in &mut g.skills {
                skill.supports.truncate(12);
            }
            for item in &mut g.gear {
                item.mods.truncate(10);
                item.must_have.truncate(10);
                item.trade_query = item.trade_query.chars().take(4000).collect();
            }
        }
        self.guides.truncate(20);
        self
    }

    pub fn profile(&self) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == self.active)
    }

    pub fn profile_mut(&mut self) -> Option<&mut Profile> {
        let active = self.active.clone();
        self.profiles.iter_mut().find(|p| p.id == active)
    }

    pub fn push_history(&mut self, mut ev: HistoryEvent) {
        if ev.title.len() > 300 {
            ev.title.truncate(300);
        }
        self.history.push(ev);
        if self.history.len() > HISTORY_LIMIT {
            let keep = self.history.len() - HISTORY_LIMIT;
            self.history.drain(..keep);
        }
    }
}

/// Loads `state.json`, keeping an unreadable file instead of overwriting it.
pub fn load_state(path: &Path) -> Saved {
    let Ok(bytes) = std::fs::read(path) else { return Saved::default() };
    // Notepad and PowerShell write a UTF-8 BOM; serde does not accept one.
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    let text = String::from_utf8_lossy(bytes);
    match serde_json::from_str::<Saved>(&text) {
        Ok(saved) => saved.sanitized(),
        Err(err) => {
            eprintln!("state.json is damaged ({err}), starting fresh and keeping a copy");
            let backup = path.with_file_name(format!(
                "state.corrupt-{}.json",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            ));
            let _ = std::fs::rename(path, backup);
            Saved::default()
        }
    }
}

pub fn save_state(path: &Path, saved: &Saved) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(saved).unwrap_or_else(|_| b"{}".to_vec());
    write_atomic(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_has_the_starter_profile() {
        let s = Saved::default();
        let p = s.profile().expect("active profile");
        assert_eq!(s.lang, "ru");
        assert_eq!((p.class.as_str(), p.ascendancy.as_str(), p.level, p.act), ("Ranger", "Deadeye", 6, 1));
        assert_eq!(p.build_id, "lightning-arrow-deadeye");
    }

    #[test]
    fn corrupted_state_is_backed_up_and_replaced() {
        let dir = std::env::temp_dir().join(format!("eni-state-{}-{}", std::process::id(), new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let loaded = load_state(&path);
        assert_eq!(loaded.lang, "ru");
        let backups: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("state.corrupt-"))
            .collect();
        assert_eq!(backups.len(), 1, "damaged file must be kept, not deleted");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn state_roundtrip_keeps_profiles_and_history() {
        let dir = std::env::temp_dir().join(format!("eni-state-{}-{}", std::process::id(), new_id("t")));
        let path = dir.join("state.json");
        let mut s = Saved::default();
        s.push_history(HistoryEvent {
            id: new_id("h"),
            ts: now_iso(),
            profile: s.active.clone(),
            kind: "zone".into(),
            level: 6,
            zone: "Clearfell".into(),
            act: 1,
            title: "Clearfell".into(),
            detail: String::new(),
        });
        save_state(&path, &s).unwrap();
        let back = load_state(&path);
        assert_eq!(back.history.len(), 1);
        assert_eq!(back.profiles.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_fields_use_defaults() {
        let s: Saved = serde_json::from_str(r#"{"lang":"ru"}"#).unwrap();
        assert_eq!(s.regexes.len(), 4, "default regexes missing: {:?}", s.regexes);
        assert_eq!(s.hotkeys.item, "Ctrl+Alt+KeyI");
        assert!(Saved::default().regexes.len() == 4);
    }

    #[test]
    fn missing_map_settings_get_defaults() {
        let s: Saved = serde_json::from_str(r#"{"lang":"ru"}"#).unwrap();
        assert_eq!(s.map.mechanics.len(), 5, "defaults: {:?}", s.map.mechanics);
        assert_eq!(s.map.mechanics[0], "Breach");
    }

    #[test]
    fn state_with_a_utf8_bom_still_loads() {
        let dir = std::env::temp_dir().join(format!("eni-state-bom-{}-{}", std::process::id(), new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"lang":"en","log":{"mode":"manual"}}"#);
        std::fs::write(&path, bytes).unwrap();
        let loaded = load_state(&path);
        assert_eq!(loaded.lang, "en");
        assert_eq!(loaded.log.mode, "manual");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hand_edited_state_is_clamped() {
        let mut s = Saved::default();
        s.overlay.scale = 9000;
        s.overlay.opacity = 400;
        s.overlay.mode = "gigantic".into();
        s.lang = "de".into();
        s.profiles[0].level = 0;
        s.shots.push(Shot {
            id: "s1".into(),
            ts: now_iso(),
            profile: s.active.clone(),
            kind: "item".into(),
            file: "../../secret.txt".into(),
            thumb: String::new(),
            w: 1,
            h: 1,
            sha256: String::new(),
            note: String::new(),
            zone: String::new(),
            level: 1,
        });
        let s = s.sanitized();
        assert_eq!(s.overlay.scale, 200);
        assert_eq!(s.overlay.opacity, 100);
        assert_eq!(s.overlay.mode, "standard");
        assert_eq!(s.lang, "ru");
        assert_eq!(s.profiles[0].level, 1);
        assert!(s.shots.is_empty(), "path traversal in a screenshot name is dropped");
    }

    #[test]
    fn hotkey_defaults_are_unique() {
        let h = Hotkeys::default();
        let mut seen: Vec<&str> = h.entries().iter().map(|(_, s)| s.as_str()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before);
    }
}
