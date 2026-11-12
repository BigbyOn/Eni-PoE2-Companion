//! Path of Building (PoB/PoB2) code import.
//!
//! A PoB code is `base64(zlib(xml))` — URL-safe base64, usually without padding.
//! We decode it with strict limits (no zip bombs), read only structural facts
//! (class, ascendancy, level, gem groups, item texts, tree node count) and build
//! a normal local build document from them. Guide prose is never copied; the
//! original source is linked instead.

use crate::item;
use serde::Serialize;
use std::io::Read;

/// Upper bounds against a malicious or broken code.
const MAX_CODE_LEN: usize = 2_000_000;
const MAX_XML_LEN: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct PobSkill {
    pub main: String,
    pub supports: Vec<String>,
    pub label: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct PobItem {
    pub name: String,
    pub base: String,
    pub rarity: String,
    pub slot: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct PobBuild {
    pub class: String,
    pub ascendancy: String,
    pub level: u32,
    pub skills: Vec<PobSkill>,
    pub items: Vec<PobItem>,
    pub tree_nodes: usize,
    pub source: Option<String>,
}

/// Normalises base64 (URL-safe, missing padding, whitespace) and inflates it.
pub fn decode(code: &str) -> Result<String, String> {
    let cleaned: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err("Пустой код".into());
    }
    if cleaned.len() > MAX_CODE_LEN {
        return Err("Код слишком длинный для PoB-сборки".into());
    }
    let normalized: String = cleaned
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    let mut padded = normalized;
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, padded.as_bytes())
        .map_err(|_| "Код не является base64 (проверь, что скопирован весь код)".to_string())?;

    let mut decoder = flate2::read::ZlibDecoder::new(&bytes[..]);
    let mut xml = String::new();
    decoder
        .by_ref()
        .take(MAX_XML_LEN + 1)
        .read_to_string(&mut xml)
        .map_err(|_| "Код не распаковывается (ожидался zlib)".to_string())?;
    if xml.len() as u64 > MAX_XML_LEN {
        return Err("Распакованный код больше 8 МБ — отклонено".into());
    }
    if !xml.contains("<PathOfBuilding") {
        return Err("Это не PoB-код: внутри нет <PathOfBuilding>".into());
    }
    Ok(xml)
}

fn attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.try_get_attribute(name).ok().flatten().map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn truthy(value: Option<String>) -> bool {
    !matches!(value.as_deref(), Some("false") | Some("0"))
}

/// Reads the structural parts of the PoB XML.
pub fn parse_xml(xml: &str) -> Result<PobBuild, String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut build = PobBuild::default();
    let mut current_skill: Option<PobSkill> = None;
    let mut item_text = String::new();
    let mut in_item = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"Build" => {
                    if let Some(v) = attr(&e, "className") {
                        build.class = v;
                    }
                    if let Some(v) = attr(&e, "ascendClassName") {
                        build.ascendancy = v;
                    }
                    if let Some(v) = attr(&e, "level").and_then(|v| v.parse().ok()) {
                        build.level = v;
                    }
                    if let Some(v) = attr(&e, "source").filter(|v| !v.is_empty()) {
                        build.source = Some(v);
                    }
                }
                b"Skill" => {
                    // A disabled skill group is ignored completely.
                    current_skill = truthy(attr(&e, "enabled")).then(|| PobSkill {
                        label: attr(&e, "label").unwrap_or_default(),
                        ..Default::default()
                    });
                }
                b"Gem" => {
                    let name = attr(&e, "nameSpec").or_else(|| attr(&e, "name")).unwrap_or_default();
                    if truthy(attr(&e, "enabled")) && !name.trim().is_empty() {
                        if let Some(skill) = current_skill.as_mut() {
                            if skill.main.is_empty() {
                                skill.main = name.trim().to_string();
                            } else {
                                skill.supports.push(name.trim().to_string());
                            }
                        }
                    }
                }
                b"Item" => {
                    in_item = true;
                    item_text.clear();
                }
                b"Spec" => {
                    if let Some(nodes) = attr(&e, "nodes") {
                        let count = nodes.split(',').filter(|n| !n.trim().is_empty()).count();
                        build.tree_nodes = build.tree_nodes.max(count);
                    }
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_item {
                    if let Ok(text) = t.decode() {
                        item_text.push_str(text.trim_end());
                        item_text.push('\n');
                    }
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"Skill" => {
                    if let Some(skill) = current_skill.take() {
                        if !skill.main.is_empty() {
                            build.skills.push(skill);
                        }
                    }
                }
                b"Item" => {
                    in_item = false;
                    if let Some(parsed) = item::parse(&item_text) {
                        build.items.push(PobItem {
                            name: parsed.name,
                            base: parsed.base,
                            rarity: parsed.rarity,
                            slot: parsed.slot,
                        });
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML не разобран: {e}")),
            _ => {}
        }
    }
    if build.class.is_empty() && build.skills.is_empty() {
        return Err("В коде нет данных о классе или навыках".into());
    }
    Ok(build)
}

fn l10n(ru: impl Into<String>, en: impl Into<String>) -> serde_json::Value {
    serde_json::json!({ "ru": ru.into(), "en": en.into() })
}

/// Builds a normal local build document from the PoB facts.
pub fn to_build_json(pob: &PobBuild, id: &str) -> serde_json::Value {
    let main_skill = pob.skills.first().map(|s| s.main.clone()).unwrap_or_default();
    let class = if pob.class.trim().is_empty() { "Unknown" } else { pob.class.trim() };
    let level = pob.level.clamp(1, 100);
    let title = if main_skill.is_empty() { format!("PoB: {class}") } else { format!("PoB: {class} — {main_skill}") };

    let skills: Vec<serde_json::Value> = pob
        .skills
        .iter()
        .take(8)
        .map(|s| {
            serde_json::json!({
                "name": s.main,
                "supports": s.supports,
                "note": if s.label.is_empty() { serde_json::Value::Null } else { l10n(s.label.clone(), s.label.clone()) }
            })
        })
        .collect();
    let gear: Vec<serde_json::Value> = pob
        .items
        .iter()
        .take(20)
        .map(|i| {
            let line = if i.base.is_empty() || i.base == i.name {
                format!("{} {}", i.rarity, i.name)
            } else {
                format!("{} {} ({})", i.rarity, i.name, i.base)
            };
            l10n(line.clone(), line)
        })
        .collect();
    let mut passives = Vec::new();
    if pob.tree_nodes > 0 {
        passives.push(l10n(format!("Узлов в дереве навыков: {}", pob.tree_nodes), format!("Passive tree nodes: {}", pob.tree_nodes)));
        passives.push(l10n(
            "Идентификаторы узлов из PoB не сопоставлены с названиями — сверяйся с деревом в игре.".to_string(),
            "PoB node ids are not mapped to names — check the tree in game.".to_string(),
        ));
    }
    let summary = format!(
        "Импортировано из Path of Building: класс {class}, восхождение {}, уровень {level}, групп навыков {}, предметов {}, узлов дерева {}.",
        if pob.ascendancy.is_empty() { "—" } else { &pob.ascendancy },
        pob.skills.len(),
        pob.items.len(),
        pob.tree_nodes
    );

    serde_json::json!({
        "schema": 1,
        "id": id,
        "version": "pob-import",
        "patch": "",
        "name": l10n(title.clone(), title.clone()),
        "class": class,
        "ascendancy": pob.ascendancy,
        "summary": l10n(summary.clone(), summary),
        "disclaimer": l10n(
            "Это структурная копия билда из Path of Building (класс, камни, предметы, число узлов). Тексты гайдов не копируются, источник указан ниже.",
            "This is a structural copy of a Path of Building build (class, gems, items, node count). Guide texts are not copied; the source is linked below."
        ),
        "sources": [{ "label": "Path of Building (community)", "url": "https://github.com/PathOfBuildingCommunity" }],
        "stages": [{
            "id": format!("pob-{level}"),
            "levels": [level, 100],
            "title": l10n(format!("Импорт PoB (уровень {level})"), format!("PoB import (level {level})")),
            "focus": l10n(
                "Проверь камни, сопротивления и предметы по этому списку; дальше веди персонажа по своим этапам.",
                "Check gems, resistances and items against this list; continue with your own stages."
            ),
            "skills": skills,
            "passives": passives,
            "gear": gear,
            "checklist": [
                { "id": format!("{id}-gems"), "label": l10n("Камни и связи совпадают с PoB", "Gems and links match PoB") },
                { "id": format!("{id}-res"), "label": l10n("Сопротивления и жизнь в порядке", "Resistances and life are fine") }
            ]
        }],
        "priorities": {},
        "tips": [
            l10n("Импорт — снимок билда, а не пошаговая прокачка: этапы по уровням задаются вручную или другим билдом.",
                 "The import is a snapshot, not a leveling plan: level stages are set by hand or by another build.")
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<PathOfBuilding>
  <Build level="42" className="Ranger" ascendClassName="Deadeye" source="pobb.in/test">
    <PlayerStat stat="Life" value="1200"/>
  </Build>
  <Skills>
    <Skill enabled="true" label="Lightning Arrow">
      <Gem nameSpec="Lightning Arrow" level="18" quality="20" enabled="true"/>
      <Gem nameSpec="Rapid Attacks" level="18" enabled="true"/>
      <Gem nameSpec="Added Lightning Damage" level="18" enabled="true"/>
      <Gem nameSpec="Disabled Support" level="1" enabled="false"/>
    </Skill>
    <Skill enabled="false" label="Off">
      <Gem nameSpec="Frost Bomb" enabled="true"/>
    </Skill>
  </Skills>
  <Tree><Spec treeVersion="0_5" nodes="1,2,3,4,5"/></Tree>
  <Items>
    <Item>Rarity: Unique
Doom Loop
Prismatic Ring
--------
+18 to maximum Life
</Item>
  </Items>
</PathOfBuilding>"#;

    fn encode(xml: &str, url_safe: bool) -> String {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(xml.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        if url_safe {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(compressed)
        } else {
            base64::engine::general_purpose::STANDARD.encode(compressed)
        }
    }

    #[test]
    fn decodes_standard_and_url_safe_codes() {
        let standard = encode(XML, false);
        let url = encode(XML, true);
        assert!(decode(&standard).unwrap().contains("<PathOfBuilding"));
        assert!(decode(&url).unwrap().contains("<PathOfBuilding"));
        // whitespace and missing padding are tolerated
        let spaced = format!("  {}\n", url);
        assert!(decode(&spaced).unwrap().contains("className"));
    }

    #[test]
    fn rejects_garbage_and_zip_bombs() {
        assert!(decode("not base64 at all!!").is_err());
        assert!(decode("").is_err());
        // valid base64, but not zlib
        let raw = base64::engine::general_purpose::STANDARD.encode(b"hello");
        assert!(decode(&raw).is_err());
    }

    #[test]
    fn parses_class_gems_items_and_nodes() {
        let xml = decode(&encode(XML, true)).unwrap();
        let pob = parse_xml(&xml).unwrap();
        assert_eq!(pob.class, "Ranger");
        assert_eq!(pob.ascendancy, "Deadeye");
        assert_eq!(pob.level, 42);
        assert_eq!(pob.tree_nodes, 5);
        assert_eq!(pob.source.as_deref(), Some("pobb.in/test"));
        assert_eq!(pob.skills.len(), 1, "disabled skill group is skipped");
        assert_eq!(pob.skills[0].main, "Lightning Arrow");
        assert_eq!(pob.skills[0].supports, vec!["Rapid Attacks", "Added Lightning Damage"]);
        assert_eq!(pob.items.len(), 1);
        assert_eq!(pob.items[0].name, "Doom Loop");
        assert_eq!(pob.items[0].rarity, "Unique");
    }

    #[test]
    fn produces_a_valid_build_document() {
        let pob = parse_xml(XML).unwrap();
        let json = to_build_json(&pob, "pob-test");
        let text = serde_json::to_string(&json).unwrap();
        let doc = crate::builds::BuildDoc::parse(&text).expect("imported build must validate");
        assert_eq!(doc.class, "Ranger");
        assert_eq!(doc.ascendancy, "Deadeye");
        assert!(doc.stage_for(42).levels[0] <= 42);
        assert!(doc.stage_for(42).skills[0].name.contains("Lightning Arrow"));
        assert!(doc.sources.iter().all(|s| s.url.starts_with("https://")));
    }

    #[test]
    fn rejects_xml_without_build_data() {
        assert!(parse_xml("<PathOfBuilding></PathOfBuilding>").is_err());
        assert!(decode(&encode("<html>nope</html>", true)).is_err());
    }
}
