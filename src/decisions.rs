//! Local decision memory for Eni-VTuber.
//!
//! The assistant may advise, but the player sometimes chooses differently. This
//! module stores *why*: the situation, the advice, the decision and the reason,
//! bound to the active build, stage and patch.
//!
//! Rules (see `docs/ENI-VTUBER.md`):
//! * memory is scoped: a record is only reused for the **same** `build_id` and
//!   the **same** `patch`; an empty build or patch never matches, so an old
//!   decision is never carried into another build or another patch silently;
//! * this is retrieval, not training — Qwen is not fine-tuned;
//! * the file is plain JSON in the companion data directory, written atomically
//!   and capped, so a broken file degrades to "no memory" instead of a crash.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA: u32 = 1;
pub const FILE_NAME: &str = "decisions.json";
pub const MAX_RECORDS: usize = 500;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Words ignored when comparing situations. Only used for similarity, never to
/// change the stored text.
const STOPWORDS: &[&str] = &[
    "что",
    "как",
    "где",
    "когда",
    "почему",
    "это",
    "этот",
    "эта",
    "для",
    "или",
    "при",
    "над",
    "под",
    "без",
    "мне",
    "меня",
    "мой",
    "моя",
    "мои",
    "его",
    "она",
    "они",
    "там",
    "тут",
    "уже",
    "ещё",
    "еще",
    "было",
    "быть",
    "есть",
    "надо",
    "нужно",
    "можно",
    "лучше",
    "всё",
    "все",
    "the",
    "and",
    "for",
    "with",
    "why",
    "how",
    "what",
    "that",
    "this",
    "was",
    "were",
    "you",
];

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DecisionRecord {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub id: String,
    pub created: String,
    pub build_id: String,
    #[serde(default)]
    pub build_name: String,
    #[serde(default)]
    pub stage_id: String,
    #[serde(default)]
    pub stage_title: String,
    #[serde(default)]
    pub patch: String,
    pub situation: String,
    #[serde(default)]
    pub advice: String,
    pub decision: String,
    pub reason: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: String,
}

fn default_schema() -> u32 {
    SCHEMA
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DecisionFile {
    #[serde(default = "default_schema")]
    pub schema: u32,
    #[serde(default)]
    pub records: Vec<DecisionRecord>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct NewDecision {
    pub situation: String,
    #[serde(default)]
    pub advice: String,
    pub decision: String,
    pub reason: String,
    #[serde(default)]
    pub build_id: String,
    #[serde(default)]
    pub build_name: String,
    #[serde(default)]
    pub stage_id: String,
    #[serde(default)]
    pub stage_title: String,
    #[serde(default)]
    pub patch: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DecisionFilter {
    #[serde(default)]
    pub build_id: String,
    #[serde(default)]
    pub patch: String,
    #[serde(default)]
    pub stage_id: String,
    #[serde(default)]
    pub limit: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct ScoredDecision {
    pub score: f64,
    pub record: DecisionRecord,
}

pub fn file_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Load the store. A missing or corrupt file yields an empty store: the memory
/// must never prevent the assistant from answering.
pub fn load(path: &Path) -> DecisionFile {
    let Ok(text) = std::fs::read_to_string(path) else {
        return DecisionFile::default();
    };
    let mut file: DecisionFile = serde_json::from_str(&text).unwrap_or_default();
    if file.schema == 0 {
        file.schema = SCHEMA;
    }
    file
}

/// Atomic write: a crash mid-write leaves the previous file intact.
pub fn save(path: &Path, file: &DecisionFile) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("создать папку памяти: {e}"))?;
    }
    let text =
        serde_json::to_string_pretty(file).map_err(|e| format!("сериализовать память: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("записать память: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("заменить память: {e}"))?;
    Ok(())
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn next_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("d-{nanos:x}{n:x}")
}

/// Store one decision. Empty `situation`, `decision` or `reason` are rejected:
/// a memory without a why cannot be reused honestly.
pub fn add(path: &Path, new: NewDecision) -> Result<DecisionRecord, String> {
    let situation = new.situation.trim();
    let decision = new.decision.trim();
    let reason = new.reason.trim();
    if situation.is_empty() {
        return Err("Не записано: не хватает ситуации (что происходило).".into());
    }
    if decision.is_empty() {
        return Err("Не записано: не хватает решения игрока.".into());
    }
    if reason.is_empty() {
        return Err("Не записано: не хватает причины решения.".into());
    }
    if new.build_id.trim().is_empty() {
        return Err("Не записано: неизвестен активный билд — решение не к чему привязать.".into());
    }

    let mut file = load(path);
    let record = DecisionRecord {
        schema: SCHEMA,
        id: next_id(),
        created: now_iso(),
        build_id: new.build_id.trim().to_string(),
        build_name: new.build_name.trim().to_string(),
        stage_id: new.stage_id.trim().to_string(),
        stage_title: new.stage_title.trim().to_string(),
        patch: new.patch.trim().to_string(),
        situation: situation.to_string(),
        advice: new.advice.trim().to_string(),
        decision: decision.to_string(),
        reason: reason.to_string(),
        tags: new.tags,
        source: if new.source.trim().is_empty() {
            "voice".into()
        } else {
            new.source.trim().to_string()
        },
    };
    file.records.push(record.clone());
    if file.records.len() > MAX_RECORDS {
        let extra = file.records.len() - MAX_RECORDS;
        file.records.drain(0..extra);
    }
    save(path, &file)?;
    Ok(record)
}

pub fn list(path: &Path, filter: &DecisionFilter) -> Vec<DecisionRecord> {
    let file = load(path);
    let limit = if filter.limit == 0 {
        50
    } else {
        filter.limit.min(200)
    };
    let mut out: Vec<DecisionRecord> = file
        .records
        .into_iter()
        .filter(|r| filter.build_id.is_empty() || r.build_id == filter.build_id)
        .filter(|r| filter.patch.is_empty() || r.patch == filter.patch)
        .filter(|r| filter.stage_id.is_empty() || r.stage_id == filter.stage_id)
        .collect();
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out.truncate(limit);
    out
}

pub fn delete(path: &Path, id: &str) -> Result<bool, String> {
    let mut file = load(path);
    let before = file.records.len();
    file.records.retain(|r| r.id != id);
    if file.records.len() == before {
        return Ok(false);
    }
    save(path, &file)?;
    Ok(true)
}

fn tokenize(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .map(|w| w.trim_matches('-').to_string())
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

fn similarity(a: &str, b: &str) -> f64 {
    let left = tokenize(a);
    let right = tokenize(b);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let common = left.intersection(&right).count() as f64;
    let smaller = left.len().min(right.len()) as f64;
    // Favour records that cover the new situation, not ones that merely share
    // a couple of long words out of a much bigger text.
    common / smaller
}

/// Find confirmed examples that are safe to reuse: same build **and** same
/// patch. `stage_id` only improves the ordering; it never widens the scope.
pub fn similar(
    path: &Path,
    build_id: &str,
    patch: &str,
    stage_id: &str,
    situation: &str,
    limit: usize,
) -> Vec<ScoredDecision> {
    let build_id = build_id.trim();
    let patch = patch.trim();
    if build_id.is_empty() || patch.is_empty() {
        // Without both keys we cannot prove the memory belongs here.
        return Vec::new();
    }
    let limit = if limit == 0 { 3 } else { limit.min(10) };
    let mut scored: Vec<ScoredDecision> = load(path)
        .records
        .into_iter()
        .filter(|r| r.build_id == build_id && r.patch == patch)
        .filter_map(|record| {
            let mut score = similarity(situation, &record.situation);
            if !stage_id.is_empty() && record.stage_id == stage_id {
                score += 0.1;
            }
            if score >= 0.12 {
                Some(ScoredDecision { score, record })
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.record.created.cmp(&a.record.created))
    });
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eni-decisions-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(build: &str, patch: &str, situation: &str) -> NewDecision {
        NewDecision {
            situation: situation.into(),
            advice: "Совет: взять узел на сопротивление".into(),
            decision: "Взял узел на ману".into(),
            reason: "Маны не хватало на ротацию".into(),
            build_id: build.into(),
            build_name: "Lightning Arrow".into(),
            stage_id: "stage-2".into(),
            stage_title: "Act 2".into(),
            patch: patch.into(),
            tags: vec![],
            source: "test".into(),
        }
    }

    #[test]
    fn add_then_list_and_delete() {
        let dir = temp_dir("crud");
        let path = file_path(&dir);
        let record = add(
            &path,
            sample("build-a", "0.5.5", "Не хватает маны на боссе"),
        )
        .unwrap();
        assert!(record.id.starts_with("d-"));
        assert_eq!(list(&path, &DecisionFilter::default()).len(), 1);

        assert!(delete(&path, &record.id).unwrap());
        assert!(!delete(&path, &record.id).unwrap());
        assert!(list(&path, &DecisionFilter::default()).is_empty());
    }

    #[test]
    fn memory_survives_reload() {
        let dir = temp_dir("reload");
        let path = file_path(&dir);
        add(&path, sample("build-a", "0.5.5", "Выбор награды в акте 2")).unwrap();

        // A fresh load from disk is what happens after a restart.
        let reloaded = load(&path);
        assert_eq!(reloaded.records.len(), 1);
        assert_eq!(reloaded.records[0].decision, "Взял узел на ману");
    }

    #[test]
    fn empty_build_or_patch_is_rejected() {
        let dir = temp_dir("required");
        let path = file_path(&dir);
        assert!(add(&path, sample("", "0.5.5", "s")).is_err());
        assert!(add(
            &path,
            NewDecision {
                build_id: "build-a".into(),
                situation: "s".into(),
                decision: "d".into(),
                reason: "".into(),
                ..Default::default()
            }
        )
        .is_err());
    }

    #[test]
    fn similarity_ignores_other_builds_and_patches() {
        let dir = temp_dir("scope");
        let path = file_path(&dir);
        add(
            &path,
            sample("build-a", "0.5.5", "Не хватает маны на боссе"),
        )
        .unwrap();
        add(
            &path,
            sample("build-b", "0.5.5", "Не хватает маны на боссе"),
        )
        .unwrap();
        add(
            &path,
            sample("build-a", "0.5.6", "Не хватает маны на боссе"),
        )
        .unwrap();

        let hits = similar(&path, "build-a", "0.5.5", "stage-2", "Не хватает маны", 5);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score > 0.5);

        assert!(similar(&path, "build-a", "0.5.5", "", "совсем другое", 5).is_empty());
        // Unknown patch must not reuse anything.
        assert!(similar(&path, "build-a", "", "", "Не хватает маны", 5).is_empty());
    }

    #[test]
    fn corrupt_file_means_empty_memory() {
        let dir = temp_dir("corrupt");
        let path = file_path(&dir);
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).records.is_empty());
        assert!(add(&path, sample("build-a", "0.5.5", "s")).is_ok());
    }

    #[test]
    fn old_records_are_capped() {
        let dir = temp_dir("cap");
        let path = file_path(&dir);
        for i in 0..MAX_RECORDS + 5 {
            add(&path, sample("build-a", "0.5.5", &format!("ситуация {i}"))).unwrap();
        }
        assert_eq!(load(&path).records.len(), MAX_RECORDS);
    }
}
