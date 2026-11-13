//! Import a build guide from a Mobalytics PoE2 page.
//!
//! Mobalytics answers plain HTTP clients with a Cloudflare challenge, but a real
//! Chromium render passes it. We therefore ask the user's installed Microsoft
//! Edge to render the page headless and read the resulting DOM. Inside the page
//! there is a `window.__PRELOADED_STATE__` JSON with build variants: gems,
//! per-slot equipment, modifiers and ready-made trade queries.
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
    /// Base type for the trade query (may equal the name for Mobalytics data).
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub rarity: String,
    pub mods: Vec<String>,
    pub must_have: Vec<String>,
    /// Trade API request prepared by Mobalytics for this item.
    pub trade_query: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct GuideVariant {
    pub index: usize,
    /// "Вариант 1" — the page gives no readable variant names.
    pub label: String,
    pub skills: Vec<GuideSkill>,
    pub gear: Vec<GuideGear>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct GuideSet {
    pub url: String,
    pub title: String,
    pub class: String,
    pub ascendancy: String,
    pub variants: Vec<GuideVariant>,
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
fn cdp_eval(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    id: &mut u64,
    expression: &str,
) -> Result<serde_json::Value, String> {
    *id += 1;
    let request = serde_json::json!({
        "id": *id,
        "method": "Runtime.evaluate",
        "params": { "expression": expression, "returnByValue": true, "awaitPromise": true }
    });
    socket
        .send(tungstenite::Message::Text(request.to_string().into()))
        .map_err(|e| format!("CDP: {e}"))?;
    loop {
        let message = socket.read().map_err(|e| format!("CDP: {e}"))?;
        let text = match message {
            tungstenite::Message::Text(text) => text.to_string(),
            tungstenite::Message::Close(_) => return Err("CDP: соединение закрыто".into()),
            _ => continue,
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        if value.get("id").and_then(|v| v.as_u64()) == Some(*id) {
            return Ok(value);
        }
    }
}

/// Renders the page with the installed Edge and returns the DOM.
///
/// Edge 157 ignores `--dump-dom` (empty stdout), so we drive the browser over
/// the DevTools protocol: start it with a debugging port, wait for the page
/// target, poll `window.__PRELOADED_STATE__` and read `outerHTML` when the
/// build data is there. Everything is bounded: 120 s overall, 15 s per CDP read.
pub fn fetch_html(url: &str) -> Result<String, String> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    if !allowed_url(url) {
        return Err("Разрешены только страницы билдов Mobalytics (https://mobalytics.gg/poe-2/...)".into());
    }
    let edge = edge_path().ok_or("Не найден Microsoft Edge: он нужен, чтобы открыть страницу Mobalytics")?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let profile = std::env::temp_dir().join(format!("eni-guide-{}-{stamp}", std::process::id()));
    let port = 9300 + (stamp % 500) as u16;
    let mut child = Command::new(&edge)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--hide-scrollbars",
            "--mute-audio",
            &format!("--user-data-dir={}", profile.display()),
            &format!("--user-agent={UA}"),
            &format!("--remote-debugging-port={port}"),
            url,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Edge не запустился: {e}"))?;

    let result = (|| -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut ws_url: Option<String> = None;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return Err("Edge закрылся до загрузки страницы".into());
            }
            let list_url = format!("http://127.0.0.1:{port}/json/list");
            let listed = ureq::get(&list_url)
                .header("User-Agent", UA)
                .call()
                .ok()
                .and_then(|mut response| response.body_mut().read_to_string().ok())
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
            if let Some(targets) = listed.as_ref().and_then(|v| v.as_array()) {
                if let Some(target) = targets.iter().find(|t| {
                    t.get("type").and_then(|v| v.as_str()) == Some("page")
                        && t.get("url").and_then(|v| v.as_str()).map(|u| u.contains("mobalytics")).unwrap_or(false)
                }) {
                    if let Some(ws) = target.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
                        ws_url = Some(ws.to_string());
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let ws_url = ws_url.ok_or("Edge не открыл страницу за 120 секунд")?;

        let (mut socket, _) = tungstenite::connect(&ws_url).map_err(|e| format!("CDP: {e}"))?;
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_mut() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
        }
        let mut id = 0u64;
        let mut html = String::new();
        while Instant::now() < deadline {
            let probe = cdp_eval(
                &mut socket,
                &mut id,
                "JSON.stringify({t:document.title,s:!!window.__PRELOADED_STATE__,r:document.readyState})",
            )?;
            let raw = probe.get("result").and_then(|r| r.get("result")).and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
            if let Ok(state) = serde_json::from_str::<serde_json::Value>(raw) {
                let title = state.get("t").and_then(|v| v.as_str()).unwrap_or("");
                let ready = state.get("s").and_then(|v| v.as_bool()).unwrap_or(false);
                let challenge = title.contains("Just a moment") || title.contains("Один момент");
                if ready && !challenge {
                    let page = cdp_eval(&mut socket, &mut id, "document.documentElement.outerHTML")?;
                    html = page
                        .get("result")
                        .and_then(|r| r.get("result"))
                        .and_then(|r| r.get("value"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        if html.len() > MAX_HTML {
            return Err("Страница слишком большая".into());
        }
        if html.is_empty() {
            return Err("Страница не отдала данные билда за 120 секунд".into());
        }
        if !html.contains("__PRELOADED_STATE__") {
            return Err("На странице нет данных билда — проверь ссылку".into());
        }
        Ok(html)
    })();

    let pid = child.id().to_string();
    let _ = Command::new("taskkill").args(["/PID", &pid, "/T", "/F"]).output();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&profile);
    result
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

fn find_variant_values(value: &Value) -> Option<&Vec<Value>> {
    if let Some(values) = value.get("buildVariants").and_then(|v| v.get("values")).and_then(|v| v.as_array()) {
        if !values.is_empty() {
            return Some(values);
        }
    }
    match value {
        Value::Object(map) => map.values().find_map(find_variant_values),
        Value::Array(items) => items.iter().find_map(find_variant_values),
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
        let unique = item.get("isUnique").and_then(|v| v.as_bool()).unwrap_or(false);
        out.push(GuideGear {
            slot: slot.clone(),
            name: name.clone(),
            base: name,
            rarity: if unique { "Unique".into() } else { "Rare".into() },
            mods,
            must_have: must,
            trade_query: query,
        });
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

/// Parses the rendered page into every build variant.
pub fn parse(html: &str, url: &str) -> Result<GuideSet, String> {
    let state = parse_state(html)?;
    let values = find_variant_values(&state).ok_or("На странице не найден план билда (buildVariants)")?;
    let title = title_of(html);
    let (class, ascendancy) = class_from_title(&title);
    let mut variants = Vec::new();
    for (index, variant) in values.iter().enumerate() {
        let skills = skills_from_variant(variant);
        let gear = gear_from_variant(variant);
        if skills.is_empty() && gear.is_empty() {
            continue;
        }
        variants.push(GuideVariant {
            index,
            label: format!("Вариант {}", index + 1),
            skills,
            gear,
        });
    }
    if variants.is_empty() {
        return Err("В плане билда нет ни камней, ни снаряжения".into());
    }
    Ok(GuideSet { url: url.to_string(), title, class, ascendancy, variants })
}

fn l10n(ru: impl Into<String>, en: impl Into<String>) -> Value {
    serde_json::json!({ "ru": ru.into(), "en": en.into() })
}

/// "Вариант 2" / "Variant 2" depending on the UI language.
pub fn variant_label(index: usize, lang: &str) -> String {
    if lang == "en" {
        format!("Variant {}", index + 1)
    } else {
        format!("Вариант {}", index + 1)
    }
}

/// Builds a local build document (same schema as the bundled builds) for one variant.
pub fn to_build_json(set: &GuideSet, variant_index: usize, id: &str, lang: &str) -> Value {
    let variant = set.variants.iter().find(|v| v.index == variant_index).unwrap_or(&set.variants[0]);
    let title = if set.title.is_empty() { "Гайд Mobalytics".to_string() } else { set.title.clone() };
    let label = variant_label(variant.index, lang);
    let suffix = if set.variants.len() > 1 { format!(" · {label}") } else { String::new() };
    let full_title = format!("{title}{suffix}");
    let class = if set.class.is_empty() { "Unknown" } else { set.class.as_str() };
    let skills: Vec<Value> = variant
        .skills
        .iter()
        .take(10)
        .map(|s| serde_json::json!({ "name": s.main, "supports": s.supports, "note": null }))
        .collect();
    let gear: Vec<Value> = variant
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
        "Импортировано из Mobalytics: {full_title}, класс {class}, восхождение {}, групп камней {}, предметов {}.",
        if set.ascendancy.is_empty() { "—" } else { &set.ascendancy },
        variant.skills.len(),
        variant.gear.len()
    );
    serde_json::json!({
        "schema": 1,
        "id": id,
        "version": "guide-import",
        "patch": "",
        "name": l10n(full_title.clone(), full_title),
        "class": class,
        "ascendancy": set.ascendancy,
        "summary": l10n(summary.clone(), summary),
        "disclaimer": l10n(
            "Это структурная копия плана билда с Mobalytics (камни и снаряжение). Тексты гайда не копируются, источник указан ниже.",
            "A structural copy of the Mobalytics build plan (gems and gear). Guide texts are not copied; the source is linked below."
        ),
        "sources": [{ "label": "Mobalytics", "url": set.url }],
        "stages": [{
            "id": "guide-import",
            "levels": [1, 100],
            "title": l10n(format!("План из гайда ({label})"), format!("Plan from the guide ({label})")),
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

    fn fixture(two_variants: bool) -> String {
        let mut values = vec![json!({
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
        })];
        if two_variants {
            values.push(json!({
                "id": 3,
                "skillGems": {"gems": [{"activeSkill": {"name": "Tornado Shot"}, "subSkills": []}]},
                "equipment": {"body": {"commonItem": {"name": "Expert Keth Raiment", "explicitDescriptions": [], "poe2TradeRequest": {"query": "{\"query\":{\"status\":{\"option\":\"online\"}}}"}}}}
            }));
        }
        let state = json!({
            "poe2State": {"apollo": {"graphqlV2": {"queries": [{}, {"state": {"data": [{"game": {"documents": {"userGeneratedDocumentBySlug": {"data": {
                "data": {"buildVariants": {"values": values}}
            }}}}}]}}]}}}
        });
        format!(
            "<html><head><title>0.5 Fubgun Lightning Arrow Deadeye - PoE 2 Ranger Build Guide</title></head><body><script>window.__PRELOADED_STATE__={};</script></body></html>",
            state
        )
    }

    #[test]
    fn parses_title_class_and_ascendancy() {
        let g = parse(&fixture(false), "https://mobalytics.gg/poe-2/builds/x").unwrap();
        assert!(g.title.contains("Lightning Arrow"));
        assert_eq!(g.class, "Ranger");
        assert_eq!(g.ascendancy, "Deadeye");
    }

    #[test]
    fn parses_skills_and_gear_with_trade_query() {
        let g = parse(&fixture(false), "https://mobalytics.gg/poe-2/builds/x").unwrap();
        assert_eq!(g.variants.len(), 1);
        let v = &g.variants[0];
        assert_eq!(v.skills.len(), 1);
        assert_eq!(v.skills[0].main, "Lightning Arrow");
        assert_eq!(v.skills[0].supports, vec!["Faster Attack", "Weapon Elemental Damage"]);
        assert_eq!(v.gear.len(), 1, "priorityList is skipped");
        let item = &v.gear[0];
        assert_eq!(item.slot, "amulet");
        assert_eq!(item.name, "Stellar Amulet");
        assert_eq!(item.must_have, vec!["+100 to maximum Life"]);
        assert!(item.trade_query.contains("securable"));
    }

    #[test]
    fn keeps_every_variant() {
        let g = parse(&fixture(true), "https://mobalytics.gg/poe-2/builds/x").unwrap();
        assert_eq!(g.variants.len(), 2);
        assert_eq!(g.variants[0].label, "Вариант 1");
        assert_eq!(g.variants[1].label, "Вариант 2");
        assert_eq!(g.variants[1].skills[0].main, "Tornado Shot");
        assert_eq!(g.variants[1].gear[0].slot, "body");
    }

    #[test]
    fn build_document_uses_the_selected_variant() {
        let g = parse(&fixture(true), "https://mobalytics.gg/poe-2/builds/x").unwrap();
        let doc = to_build_json(&g, 1, "guide-test", "ru");
        let text = serde_json::to_string(&doc).unwrap();
        let parsed = crate::builds::BuildDoc::parse(&text).expect("guide build must validate");
        assert!(parsed.name.ru.contains("Вариант 2"));
        assert_eq!(parsed.stages[0].skills[0].name, "Tornado Shot");
        assert_eq!(parsed.stages[0].gear.len(), 1);
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
