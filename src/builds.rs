//! Local build documents: a small JSON format a user can write by hand and that
//! a later AI feature can read. Only the bundled Lightning Arrow build ships
//! with v0.1; more can be dropped into the user's `builds` directory.

use crate::model::L10n;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BUNDLED: &str = include_str!("../assets/builds/lightning-arrow-deadeye.json");

/// Only these hosts may appear as a build source, and only over HTTPS.
pub const ALLOWED_SOURCE_HOSTS: &[&str] = &["mobalytics.gg", "www.mobalytics.gg", "pathofexile.com", "www.pathofexile.com", "poe2speedrun.malura.de", "github.com", "raw.githubusercontent.com", "maxroll.gg", "poe2wiki.net", "www.poe2wiki.net", "poedb.tw", "craftofexile.com", "www.craftofexile.com"];

#[derive(Deserialize, Clone, Debug)]
struct RawBuild {
    schema: u32,
    id: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    patch: String,
    name: L10n,
    #[serde(default)]
    class: String,
    #[serde(default)]
    ascendancy: String,
    summary: L10n,
    #[serde(default)]
    disclaimer: Option<L10n>,
    #[serde(default)]
    sources: Vec<Source>,
    stages: Vec<RawStage>,
    #[serde(default)]
    priorities: Priorities,
    #[serde(default)]
    tips: Vec<L10n>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub label: String,
    pub url: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    #[serde(default)]
    pub supports: Vec<String>,
    #[serde(default)]
    pub note: Option<L10n>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub id: String,
    pub label: L10n,
}

#[derive(Deserialize, Clone, Debug)]
struct RawStage {
    id: String,
    levels: [u32; 2],
    title: L10n,
    focus: L10n,
    #[serde(default)]
    skills: Vec<Skill>,
    #[serde(default)]
    passives: Vec<L10n>,
    #[serde(default)]
    gear: Vec<L10n>,
    #[serde(default)]
    checklist: Vec<Check>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    pub id: String,
    pub levels: [u32; 2],
    pub title: L10n,
    pub focus: L10n,
    pub skills: Vec<Skill>,
    pub passives: Vec<L10n>,
    pub gear: Vec<L10n>,
    pub checklist: Vec<Check>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Priorities {
    pub stats: Vec<L10n>,
    pub weapon: Vec<L10n>,
    pub armour: Vec<L10n>,
    pub accessories: Vec<L10n>,
}

#[derive(Serialize, Clone, Debug)]
pub struct BuildDoc {
    pub id: String,
    pub version: String,
    pub patch: String,
    pub name: L10n,
    pub class: String,
    pub ascendancy: String,
    pub summary: L10n,
    pub disclaimer: Option<L10n>,
    pub sources: Vec<Source>,
    pub stages: Vec<Stage>,
    pub priorities: Priorities,
    pub tips: Vec<L10n>,
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.contains('@') && !host.is_empty()).then_some(host)
}

impl BuildDoc {
    pub fn parse(json: &str) -> Result<BuildDoc, String> {
        let raw: RawBuild = serde_json::from_str(json).map_err(|e| format!("JSON билда повреждён: {e}"))?;
        if raw.schema != 1 {
            return Err(format!("Неподдерживаемая версия схемы билда: {}", raw.schema));
        }
        if raw.id.trim().is_empty() || raw.name.ru.trim().is_empty() || raw.name.en.trim().is_empty() {
            return Err("У билда нет id или названия".into());
        }
        if raw.stages.is_empty() {
            return Err("В билде нет ни одного этапа прокачки".into());
        }
        for s in &raw.sources {
            let Some(host) = host_of(&s.url) else {
                return Err(format!("Источник должен быть https-ссылкой: {}", s.url));
            };
            if !ALLOWED_SOURCE_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host)) {
                return Err(format!("Источник с недоверенного адреса: {host}"));
            }
        }
        let mut seen: Vec<&str> = Vec::new();
        let mut checklist_ids: Vec<&str> = Vec::new();
        for st in &raw.stages {
            if st.levels[0] == 0 || st.levels[1] < st.levels[0] {
                return Err(format!("Этап {}: неверный диапазон уровней", st.id));
            }
            if seen.contains(&st.id.as_str()) {
                return Err(format!("Этап {} указан дважды", st.id));
            }
            seen.push(&st.id);
            for c in &st.checklist {
                if c.id.trim().is_empty() || c.label.ru.trim().is_empty() {
                    return Err(format!("Этап {}: пункт списка без id или текста", st.id));
                }
                if checklist_ids.contains(&c.id.as_str()) {
                    return Err(format!("Пункт {} указан дважды", c.id));
                }
                checklist_ids.push(&c.id);
            }
        }
        let stages: Vec<Stage> = raw
            .stages
            .into_iter()
            .map(|s| Stage {
                id: s.id,
                levels: s.levels,
                title: s.title,
                focus: s.focus,
                skills: s.skills,
                passives: s.passives,
                gear: s.gear,
                checklist: s.checklist,
            })
            .collect();
        Ok(BuildDoc {
            id: raw.id,
            version: raw.version,
            patch: raw.patch,
            name: raw.name,
            class: raw.class,
            ascendancy: raw.ascendancy,
            summary: raw.summary,
            disclaimer: raw.disclaimer,
            sources: raw.sources,
            stages,
            priorities: raw.priorities,
            tips: raw.tips,
        })
    }

    /// The stage that covers this character level (the last one wins above 60).
    pub fn stage_for(&self, level: u32) -> &Stage {
        self.stages
            .iter()
            .find(|s| level >= s.levels[0] && level <= s.levels[1])
            .unwrap_or_else(|| self.stages.last().expect("validated: at least one stage"))
    }

    #[allow(dead_code)]
    pub fn checklist_total(&self) -> usize {
        self.stages.iter().map(|s| s.checklist.len()).sum()
    }
}

/// Bundled build plus every valid `*.json` in `dir` (a user file replaces a
/// bundled build with the same id). Broken files are skipped, never fatal.
pub fn load_library(dir: Option<&Path>) -> Vec<BuildDoc> {
    let mut out = Vec::new();
    if let Ok(doc) = BuildDoc::parse(BUNDLED) {
        out.push(doc);
    }
    let Some(dir) = dir else { return out };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"))).collect())
        .unwrap_or_default();
    files.sort();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        match BuildDoc::parse(&text) {
            Ok(doc) => {
                out.retain(|d| d.id != doc.id);
                out.push(doc);
            }
            Err(err) => eprintln!("build {} skipped: {err}", file.display()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_build_is_valid_and_cover_all_levels() {
        let b = BuildDoc::parse(BUNDLED).expect("bundled build parses");
        assert_eq!(b.id, "lightning-arrow-deadeye");
        assert_eq!((b.class.as_str(), b.ascendancy.as_str()), ("Ranger", "Deadeye"));
        assert!(!b.stages.is_empty());
        for level in 1..=80u32 {
            let st = b.stage_for(level);
            assert!(st.levels[1] >= level || st.levels[1] == 60, "level {level} mapped to {}", st.id);
        }
        assert_eq!(b.stage_for(6).id, "lvl-6-14");
        assert_eq!(b.stage_for(99).id, "lvl-40-plus");
        assert!(b.checklist_total() >= 10);
        assert!(b.sources.iter().all(|s| s.url.starts_with("https://")));
    }

    #[test]
    fn broken_builds_are_rejected_with_messages() {
        let base = |s: &str| s.replace("__STAGES__", r#"[{"id":"s1","levels":[1,5],"title":{"ru":"а","en":"a"},"focus":{"ru":"ф","en":"f"}}]"#);
        let valid = base(
            r#"{"schema":1,"id":"x","name":{"ru":"и","en":"n"},"summary":{"ru":"с","en":"s"},"stages":__STAGES__}"#,
        );
        assert!(BuildDoc::parse(&valid).is_ok());
        assert!(BuildDoc::parse(&valid.replace(r#""schema":1"#, r#""schema":9"#)).is_err());
        let no_stages = r#"{"schema":1,"id":"x","name":{"ru":"и","en":"n"},"summary":{"ru":"с","en":"s"},"stages":[]}"#;
        assert!(BuildDoc::parse(no_stages).unwrap_err().contains("этап"));
        let bad_url = valid.replace(
            r#""stages""#,
            r#""sources":[{"label":"x","url":"http://evil.example/x"}],"stages""#,
        );
        assert!(BuildDoc::parse(&bad_url).unwrap_err().contains("https"));
        let evil_host = valid.replace(
            r#""stages""#,
            r#""sources":[{"label":"x","url":"https://evil.example/x"}],"stages""#,
        );
        assert!(BuildDoc::parse(&evil_host).unwrap_err().contains("недоверенного"));
        let dup = base(
            r#"{"schema":1,"id":"x","name":{"ru":"и","en":"n"},"summary":{"ru":"с","en":"s"},"stages":[
            {"id":"s1","levels":[1,5],"title":{"ru":"а","en":"a"},"focus":{"ru":"ф","en":"f"},"checklist":[{"id":"c1","label":{"ru":"п","en":"p"}}]},
            {"id":"s2","levels":[6,9],"title":{"ru":"б","en":"b"},"focus":{"ru":"ф","en":"f"},"checklist":[{"id":"c1","label":{"ru":"п","en":"p"}}]}]}"#,
        );
        assert!(BuildDoc::parse(&dup).unwrap_err().contains("дважды"));
    }

    #[test]
    fn user_builds_override_bundled_ones() {
        let dir = std::env::temp_dir().join(format!("eni-builds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let custom = BUNDLED.replace("Lightning Arrow — Deadeye", "Мой вариант");
        std::fs::write(dir.join("mine.json"), custom).unwrap();
        std::fs::write(dir.join("broken.json"), "{ nope").unwrap();
        let lib = load_library(Some(&dir));
        assert_eq!(lib.len(), 1, "broken file skipped, bundled one replaced");
        assert_eq!(lib[0].name.ru, "Мой вариант");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
