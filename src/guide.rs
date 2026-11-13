//! Import a build guide from a Mobalytics PoE2 page.
//!
//! Mobalytics answers plain HTTP clients with a Cloudflare challenge, but a real
//! Chromium render passes it. We therefore ask the user's installed Microsoft
//! Edge to render the page headless (no window, no profile reuse, one request per
//! click) and read the resulting DOM. Inside the page there is a
//! `window.__PRELOADED_STATE__` JSON with build variants: gems, per-slot
//! equipment, modifiers and ready-made trade queries.
//!
//! Only structural facts are kept (names, modifiers, trade queries); guide prose
//! is never stored or shown. The page URL is linked as the source.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct GuideSkill {
    pub main: String,
    pub supports: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct GuideGear {
    pub slot: String,
    pub name: String,
    pub mods: Vec<String>,
    pub must_have: Vec<String>,
    /// Trade API request prepared by Mobalytics for this item.
    pub trade_query: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Guide {
    pub url: String,
    pub title: String,
    pub class: String,
    pub ascendancy: String,
    pub variant: String,
    pub skills: Vec<GuideSkill>,
    pub gear: Vec<GuideGear>,
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const MAX_HTML: usize = 8 * 1024 * 1024;

fn edge_path() -> Option<PathBuf> {
    [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

/// Only PoE2 build pages of Mobalytics are allowed.
pub fn allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let mut parts = rest.splitn(2, '/');
    let host = parts.next().unwrap_or("").to_ascii_lowercase();
    let path = format!("/{}", parts.next().unwrap_or(""));
    matches!(host.as_str(), "mobalytics.gg" | "www.mobalytics.gg") && path.starts_with("/poe-2/") && path.contains("builds")
}

/// Renders the page with the installed Edge and returns the DOM.
///
/// One process per call: a fresh temporary profile, headless mode and a normal
/// browser User-Agent (Cloudflare rejects the default `HeadlessChrome` UA).
pub fn fetch_html(url: &str) -> Result<String, String> {
    if !allowed_url(url) {
        return Err("Разрешены только страницы билдов Mobalytics (https://mobalytics.gg/poe-2/...)".into());
    }
    let edge = edge_path().ok_or("Не найден Microsoft Edge: он нужен, чтобы открыть страницу Mobalytics")?;
    let profile = std::env::temp_dir().join(format!(
        "eni-guide-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
    ));
    let output = Command::new(&edge)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--hide-scrollbars",
            "--mute-audio",
            &format!("--user-data-dir={}", profile.display()),
            &format!("--user-agent={UA}"),
            "--virtual-time-budget=25000",
            "--dump-dom",
            url,
        ])
        .output()
        .map_err(|e| format!("Edge не запустился: {e}"))?;
    let _ = std::fs::remove_dir_all(&profile);
    let html = String::from_utf8_lossy(&output.stdout).to_string();
    if html.len() > MAX_HTML {
        return Err("Страница слишком большая".into());
    }
    if html.contains("Just a moment") || html.contains("Один момент") {
        return Err("Cloudflare не пропустил страницу. Открой её в браузере и скопируй PoB-код — он импортируется без интернета.".into());
    }
    if html.len() < 5000 {
        return Err(format!("Страница не загрузилась ({} байт)", html.len()));
    }
    Ok(html)
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = text.find(start)? + start.len();
    let j = text[i..].find(end)? + i;
    Some(&text[i..j])
}

fn title_of(html: &str) -> String {
    between(html, "<title", "</title>")
        .and_then(|rest| rest.find('>').map(|i| rest[i + 1..].trim().to_string()))
        .unwrap_or_default()
}

const CLASSES: &[&str] = &["Ranger", "Huntress", "Warrior", "Sorceress", "Witch", "Monk", "Mercenary", "Druid"];
const ASCENDANCIES: &[&str] = &[
    "Deadeye", "Pathfinder", "Amazon", "Ritualist", "Titan", "Warbringer", "Stormweaver", "Chronomancer", "Blood Mage",
    "Infernalist", "Lich", "Acolyte of Chayula", "Invoker", "Witchhunter", "Gemling Legionnaire", "Tactician",
];

fn class_from_title(title: &str) -> (String, String) {
    let class = CLASSES.iter().find(|c| title.contains(**c)).copied().unwrap_or("").to_string();
    let ascendancy = ASCENDANCIES.iter().find(|a| title.contains(**a)).copied().unwrap_or("").to_string();
    (class, ascendancy)
}

fn split_camel(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_uppercase() && !out.is_empty() {
            out.push(' ');
        }
        out.push(ch);
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Support gems in the page only carry a slug; the icon file has a readable
/// name (`.../WeaponElementalDamageSupportGem.avif` -> "Weapon Elemental Damage").
fn humanize_support(slug: &str, icon: &str) -> String {
    if let Some(file) = icon.rsplit('/').next() {
        let stem = file.split('.').next().unwrap_or("");
        let stem = stem
            .trim_end_matches("SupportGem")
            .trim_end_matches("Support")
            .trim_end_matches("Gem");
        if !stem.is_empty() {
            return split_camel(stem);
        }
    }
    let mut s = slug.strip_prefix("support").unwrap_or(slug).to_string();
    for suffix in ["playertwo", "player", "two"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
            break;
        }
    }
    split_camel(&s)
}

fn parse_state(html: &str) -> Result<Value, String> {
    let start = html.find("window.__PRELOADED_STATE__=").ok_or("В странице нет данных билда (__PRELOADED_STATE__)")?;
    let json_start = html[start..].find('{').map(|i| start + i).ok_or("Данные билда пусты")?;
    let rest = &html[json_start..];
    let end = rest.find("</script>").ok_or("Не найден конец данных билда")?;
    let raw = rest[..end].trim().trim_end_matches(';');
    serde_json::from_str(raw).map_err(|e| format!("Данные страницы не разобраны: {e}"))
}

fn find_variants(value: &Value) -> Option<&Value> {
    if let Some(variants) = value.get("buildVariants").and_then(|v| v.get("values")).and_then(|v| v.as_array()) {
        if !variants.is_empty() {
            return Some(&variants[0]);
        }
    }
    match value {
        Value::Object(map) => map.values().find_map(find_variants),
        Value::Array(items) => items.iter().find_map(find_variants),
        _ => None,
    }
}

fn gear_from_variant(variant: &Value) -> Vec<GuideGear> {
    let Some(equipment) = variant.get("equipment").and_then(|e| e.as_object()) else { return Vec::new() };
    let mut out = Vec::new();
    for (slot, entry) in equipment {
        if matches!(slot.as_str(), "priorityList" | "chakras") {
            continue;
        }
        let Some(item) = entry.get("commonItem") else { continue };
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        let mut mods = Vec::new();
        let mut must = Vec::new();
        if let Some(descriptions) = item.get("explicitDescriptions").and_then(|v| v.as_array()) {
            for d in descriptions {
                let text = d.get("description").and_then(|v| v.as_str()).unwrap_or("").trim();
                if text.is_empty() {
                    continue;
                }
                mods.push(text.to_string());
                if d.get("mustHave").and_then(|v| v.as_bool()).unwrap_or(false) {
                    must.push(text.to_string());
                }
            }
        }
        let query = item
            .get("poe2TradeRequest")
            .and_then(|v| v.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(GuideGear { slot: slot.clone(), name, mods, must_have: must, trade_query: query });
    }
    out
}

fn skills_from_variant(variant: &Value) -> Vec<GuideSkill> {
    let Some(gems) = variant.get("skillGems").and_then(|v| v.get("gems")).and_then(|v| v.as_array()) else { return Vec::new() };
    let mut out = Vec::new();
    for group in gems {
        let main = group.get("activeSkill").and_then(|s| s.get("name")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if main.is_empty() {
            continue;
        }
        let mut supports: Vec<String> = Vec::new();
        if let Some(subs) = group.get("subSkills").and_then(|v| v.as_array()) {
            for sub in subs {
                let slug = sub.get("gemSlug").and_then(|v| v.as_str()).unwrap_or("");
                let icon = sub.get("iconURL").and_then(|v| v.as_str()).unwrap_or("");
                if !slug.is_empty() {
                    let name = humanize_support(slug, icon);
                    if !name.is_empty() && !supports.contains(&name) {
                        supports.push(name);
                    }
                }
            }
        }
        out.push(GuideSkill { main, supports });
    }
    out
}

/// Parses the rendered page into structural facts.
pub fn parse(html: &str, url: &str) -> Result<Guide, String> {
    let state = parse_state(html)?;
    let variant = find_variants(&state).ok_or("На странице не найден план билда (buildVariants)")?;
    let title = title_of(html);
    let (class, ascendancy) = class_from_title(&title);
    let skills = skills_from_variant(variant);
    let gear = gear_from_variant(variant);
    if skills.is_empty() && gear.is_empty() {
        return Err("В плане билда нет ни камней, ни снаряжения".into());
    }
    let variant_id = variant.get("id").map(|v| v.to_string()).unwrap_or_default();
    Ok(Guide {
        url: url.to_string(),
        title,
        class,
        ascendancy,
        variant: variant_id,
        skills,
        gear,
    })
}

fn l10n(ru: impl Into<String>, en: impl Into<String>) -> Value {
    serde_json::json!({ "ru": ru.into(), "en": en.into() })
}

/// Builds a local build document (same schema as the bundled builds).
pub fn to_build_json(guide: &Guide, id: &str) -> Value {
    let title = if guide.title.is_empty() { "Гайд Mobalytics".to_string() } else { guide.title.clone() };
    let class = if guide.class.is_empty() { "Unknown" } else { guide.class.as_str() };
    let skills: Vec<Value> = guide
        .skills
        .iter()
        .take(10)
        .map(|s| {
            serde_json::json!({
                "name": s.main,
                "supports": s.supports,
                "note": null
            })
        })
        .collect();
    let gear: Vec<Value> = guide
        .gear
        .iter()
        .take(20)
        .map(|g| {
            let line = if g.must_have.is_empty() {
                format!("{}: {}", g.slot, g.name)
            } else {
                format!("{}: {} (обязательно: {})", g.slot, g.name, g.must_have.join(", "))
            };
            l10n(line.clone(), line)
        })
        .collect();
    let summary = format!(
        "Импортировано из Mobalytics: {}, класс {class}, восхождение {}, групп камней {}, предметов {}.",
        title,
        if guide.ascendancy.is_empty() { "—" } else { &guide.ascendancy },
        guide.skills.len(),
        guide.gear.len()
    );
    serde_json::json!({
        "schema": 1,
        "id": id,
        "version": "guide-import",
        "patch": "",
        "name": l10n(title.clone(), title.clone()),
        "class": class,
        "ascendancy": guide.ascendancy,
        "summary": l10n(summary.clone(), summary),
        "disclaimer": l10n(
            "Это структурная копия плана билда с Mobalytics (камни и снаряжение). Тексты гайда не копируются, источник указан ниже.",
            "A structural copy of the Mobalytics build plan (gems and gear). Guide texts are not copied; the source is linked below."
        ),
        "sources": [{ "label": "Mobalytics", "url": guide.url }],
        "stages": [{
            "id": "guide-import",
            "levels": [1, 100],
            "title": l10n("План из гайда", "Plan from the guide"),
            "focus": l10n(
                "Сверяй камни и снаряжение; кнопка «Найти» у предмета ищет его в торговле с фильтрами из гайда.",
                "Check gems and gear; the “Find” button searches the item with the guide's filters."
            ),
            "skills": skills,
            "passives": [],
            "gear": gear,
            "checklist": [
                { "id": format!("{id}-gems"), "label": l10n("Камни и связи как в гайде", "Gems and links match the guide") },
                { "id": format!("{id}-gear"), "label": l10n("Ключевые предметы найдены", "Key items acquired") }
            ]
        }],
        "priorities": {},
        "tips": [
            l10n("Импорт — снимок плана, а не пошаговая прокачка: этапы по уровням задаются вручную.",
                 "The import is a plan snapshot, not a leveling plan: level stages are set by hand.")
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> String {
        let state = json!({
            "poe2State": {"apollo": {"graphqlV2": {"queries": [{}, {"state": {"data": [{"game": {"documents": {"userGeneratedDocumentBySlug": {"data": {
                "data": {"buildVariants": {"values": [{
                    "id": 2,
                    "skillGems": {"gems": [
                        {"activeSkill": {"name": "Lightning Arrow"}, "subSkills": [
                            {"gemSlug": "supportrapidattacksplayertwo", "gemType": "SUPPORT", "iconURL": "https://cdn.mobalytics.gg/x/FasterAttackSupportGem.avif"},
                            {"gemSlug": "supportelementalarmamentplayertwo", "gemType": "SUPPORT", "iconURL": "https://cdn.mobalytics.gg/x/WeaponElementalDamageSupportGem.avif"}
                        ]}
                    ]},
                    "equipment": {
                        "amulet": {"commonItem": {"name": "Stellar Amulet", "explicitDescriptions": [
                            {"description": "+100 to maximum Life", "mustHave": true},
                            {"description": "+30 to Spirit", "mustHave": false}
                        ], "poe2TradeRequest": {"query": "{\"query\":{\"status\":{\"option\":\"securable\"}},\"sort\":{\"price\":\"asc\"}}"}}},
                        "priorityList": {}
                    }
                }]}}
            }}}}}]}}]}}}
        });
        format!(
            "<html><head><title>0.5 Fubgun Lightning Arrow Deadeye - PoE 2 Ranger Build Guide</title></head><body><script>window.__PRELOADED_STATE__={};</script></body></html>",
            state
        )
    }

    #[test]
    fn parses_title_class_and_ascendancy() {
        let g = parse(&fixture(), "https://mobalytics.gg/poe-2/builds/x").unwrap();
        assert!(g.title.contains("Lightning Arrow"));
        assert_eq!(g.class, "Ranger");
        assert_eq!(g.ascendancy, "Deadeye");
    }

    #[test]
    fn parses_skills_and_gear_with_trade_query() {
        let g = parse(&fixture(), "https://mobalytics.gg/poe-2/builds/x").unwrap();
        assert_eq!(g.skills.len(), 1);
        assert_eq!(g.skills[0].main, "Lightning Arrow");
        assert_eq!(g.skills[0].supports, vec!["Faster Attack", "Weapon Elemental Damage"]);
        assert_eq!(g.gear.len(), 1, "priorityList is skipped");
        let item = &g.gear[0];
        assert_eq!(item.slot, "amulet");
        assert_eq!(item.name, "Stellar Amulet");
        assert_eq!(item.must_have, vec!["+100 to maximum Life"]);
        assert!(item.trade_query.contains("securable"));
    }

    #[test]
    fn build_document_is_valid() {
        let g = parse(&fixture(), "https://mobalytics.gg/poe-2/builds/x").unwrap();
        let doc = to_build_json(&g, "guide-test");
        let text = serde_json::to_string(&doc).unwrap();
        let parsed = crate::builds::BuildDoc::parse(&text).expect("guide build must validate");
        assert_eq!(parsed.class, "Ranger");
        assert!(parsed.stages[0].gear.len() == 1);
    }

    #[test]
    fn only_mobalytics_poe2_builds_are_allowed() {
        assert!(allowed_url("https://mobalytics.gg/poe-2/builds/lightning-arrow-farmer-fubgun"));
        assert!(allowed_url("https://www.mobalytics.gg/poe-2/profile/x/builds/y"));
        assert!(!allowed_url("https://mobalytics.gg/poe-2/tier-list"));
        assert!(!allowed_url("https://example.com/poe-2/builds/x"));
        assert!(!allowed_url("http://mobalytics.gg/poe-2/builds/x"));
    }

    #[test]
    fn rejects_pages_without_build_data() {
        assert!(parse("<html><title>x</title><body>no state</body></html>", "u").is_err());
        assert!(parse("<html><script>window.__PRELOADED_STATE__={\"a\":1};</script></html>", "u").is_err());
    }
}
