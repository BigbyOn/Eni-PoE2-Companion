//! Headless API for Eni-VTuber: state, decision memory, item evaluation,
//! trade search and budget planning.
//!
//! This module is the single place that turns the companion's on-disk data and
//! the existing modules (`model`, `builds`, `campaign`, `item`, `trade`,
//! `trade_i18n`, `scout`) into JSON. It is used by:
//!
//! * `src/bin/eni-poe2-bridge.rs` — the stdio bridge the VTuber talks to;
//! * the Tauri commands, through the same `Context` and functions.
//!
//! It performs no UI work and no game interaction. Network access happens only
//! when a method explicitly asks for it (`auto`, `price`, `refresh_rates`).

use crate::budget;
use crate::decisions;
use crate::model::{self, GuideRecord, L10n, Saved};
use crate::scout;
use crate::trade;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const PROTOCOL: u32 = 1;
pub const STATE_MAX_AGE_SECONDS: u64 = 600;
const SESSION_MAX_AGE_SECONDS: u64 = 20;

#[derive(Debug, Clone)]
pub struct Context {
    pub dir: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        ApiError {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

fn pick(text: &L10n) -> String {
    if text.ru.trim().is_empty() {
        text.en.clone()
    } else {
        text.ru.clone()
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn file_age_seconds(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let modified_secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    Some((now_secs() - modified_secs).max(0.0) as u64)
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

impl Context {
    /// Resolve the companion data directory exactly like the app does:
    /// `ENI_COMPANION_DATA_DIR`, otherwise the Tauri app-data directory.
    pub fn resolve() -> Context {
        if let Some(dir) = std::env::var_os("ENI_COMPANION_DATA_DIR") {
            if !dir.is_empty() {
                return Context {
                    dir: PathBuf::from(dir),
                };
            }
        }
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Context {
            dir: base.join("io.github.bigbyon.enipoe2companion"),
        }
    }

    pub fn at(dir: impl Into<PathBuf>) -> Context {
        Context { dir: dir.into() }
    }

    pub fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn builds_dir(&self) -> PathBuf {
        self.dir.join("builds")
    }

    pub fn scout_path(&self) -> PathBuf {
        self.dir.join("scout.json")
    }

    pub fn stats_path(&self) -> PathBuf {
        self.dir.join("trade_stats.json")
    }

    pub fn decisions_path(&self) -> PathBuf {
        decisions::file_path(&self.dir)
    }

    pub fn saved(&self) -> Saved {
        model::load_state(&self.state_path())
    }

    pub fn state_age(&self) -> Option<u64> {
        file_age_seconds(&self.state_path())
    }

    /// Cached trade stat catalogue. The bridge never downloads it implicitly:
    /// the app files it once and both share the same three-day cache.
    pub fn stats(&self) -> Option<Vec<trade::StatDef>> {
        let age = file_age_seconds(&self.stats_path())?;
        if age > 3 * 24 * 3600 {
            return None;
        }
        #[derive(Deserialize)]
        struct Cache {
            #[serde(default)]
            stats: Vec<trade::StatDef>,
        }
        let cache: Cache = serde_json::from_value(read_json(&self.stats_path())?).ok()?;
        (!cache.stats.is_empty()).then_some(cache.stats)
    }

    pub fn scout(&self) -> Option<(scout::Scout, u64)> {
        let age = file_age_seconds(&self.scout_path())?;
        let value: scout::Scout = serde_json::from_value(read_json(&self.scout_path())?).ok()?;
        (value.divine_price > 0.0).then_some((value, age))
    }

    pub fn build(&self, id: &str) -> Option<crate::builds::BuildDoc> {
        if id.trim().is_empty() {
            return None;
        }
        crate::builds::load_library(Some(&self.builds_dir()))
            .into_iter()
            .find(|doc| doc.id == id)
    }

    pub fn campaign(&self) -> Option<crate::campaign::Campaign> {
        crate::campaign::Campaign::parse(crate::campaign::BUNDLED).ok()
    }

    pub fn guide_for(&self, saved: &Saved, build_id: &str) -> Option<GuideRecord> {
        saved
            .guides
            .iter()
            .rev()
            .find(|guide| guide.build_id == build_id)
            .cloned()
    }

    /// Currency rates of the requested league, or an explicit "unknown" value
    /// that never claims a set fits.
    pub fn rates_for(&self, saved: &Saved, league: &str) -> budget::Rates {
        let wanted = if league.trim().is_empty() {
            saved.trade.league.trim().to_string()
        } else {
            league.trim().to_string()
        };
        let Some((value, age)) = self.scout() else {
            return budget::Rates {
                league: wanted,
                source: "нет scout.json".into(),
                ..Default::default()
            };
        };
        if !wanted.is_empty() && !value.league.eq_ignore_ascii_case(&wanted) {
            // A rate of another league must never be used silently.
            return budget::Rates {
                league: value.league.clone(),
                source: format!("{} (другая лига)", value.source),
                as_of: value.fetched.clone(),
                age_seconds: Some(age),
                ..Default::default()
            };
        }
        budget::Rates {
            league: value.league.clone(),
            exalted_per_divine: Some(value.divine_price),
            chaos_per_divine: (value.chaos_per_divine > 0.0).then_some(value.chaos_per_divine),
            source: value.source.clone(),
            as_of: value.fetched.clone(),
            age_seconds: Some(age),
        }
    }

    pub fn preferred_league(&self, saved: &Saved) -> String {
        let saved_league = saved.trade.league.trim();
        if !saved_league.is_empty() {
            return saved_league.to_string();
        }
        self.scout()
            .map(|(value, _)| value.league)
            .unwrap_or_default()
    }
}

fn active_build_id(saved: &Saved) -> String {
    saved
        .profile()
        .map(|profile| profile.build_id.clone())
        .unwrap_or_default()
}

fn stage_for<'a>(
    build: &'a crate::builds::BuildDoc,
    level: u32,
) -> Option<&'a crate::builds::Stage> {
    build
        .stages
        .iter()
        .find(|stage| level >= stage.levels[0] && level <= stage.levels[1])
        .or_else(|| {
            build
                .stages
                .iter()
                .filter(|stage| stage.levels[0] <= level)
                .max_by_key(|stage| stage.levels[0])
        })
        .or_else(|| build.stages.first())
}

fn check_label(l10n: &L10n) -> String {
    pick(l10n)
}

/// Mode-scoped summary used by every answer: where the player is, which build
/// and stage are active, what the next goal is and what data is missing.
pub fn state_summary(ctx: &Context) -> Value {
    let saved = ctx.saved();
    let profile = saved.profile().cloned();
    let build_id = active_build_id(&saved);
    let build = ctx.build(&build_id);
    let campaign = ctx.campaign();
    let age = ctx.state_age();
    let mut missing: Vec<String> = Vec::new();

    if !ctx.state_path().exists() {
        missing.push("нет state.json: Eni-PoE2-Companion ещё не запускался на этом профиле".into());
    }
    let Some(profile) = profile else {
        missing.push("нет активного профиля персонажа".into());
        return json!({
            "protocol": PROTOCOL,
            "ok": false,
            "missing": missing,
            "state_age_seconds": age,
        });
    };
    if build_id.trim().is_empty() {
        missing.push("у профиля не выбран активный билд".into());
    } else if build.is_none() {
        missing.push(format!("файл билда {build_id} не найден в папке builds"));
    }

    let guide = ctx.guide_for(&saved, &build_id);

    let mut build_info = Value::Null;
    let mut improve = json!({});
    if let Some(build) = &build {
        let stage = stage_for(build, profile.level);
        let stage_json = stage.map(|stage| {
            let next_check = stage
                .checklist
                .iter()
                .find(|check| !profile.build_done.iter().any(|done| done == &check.id));
            json!({
                "id": stage.id,
                "title": pick(&stage.title),
                "focus": pick(&stage.focus),
                "levels": stage.levels,
                "next_check": next_check.map(|check| json!({
                    "id": check.id,
                    "label": check_label(&check.label),
                })),
                "gear": stage.gear.iter().map(|line| pick(line)).collect::<Vec<_>>(),
                "skills": stage.skills.iter().map(|skill| {
                    json!({ "main": skill.name, "supports": skill.supports })
                }).collect::<Vec<_>>(),
            })
        });
        if stage_json.is_none() {
            missing.push("в билде нет этапов — нечего сравнивать с уровнем".into());
        }
        build_info = json!({
            "id": build.id,
            "name": pick(&build.name),
            "class": build.class,
            "ascendancy": build.ascendancy,
            "patch": build.patch,
            "stage": stage_json,
        });
        let priorities: Vec<String> = build
            .priorities
            .stats
            .iter()
            .chain(build.priorities.weapon.iter())
            .chain(build.priorities.armour.iter())
            .chain(build.priorities.accessories.iter())
            .take(5)
            .map(pick)
            .collect();
        improve = json!({
            "priorities": priorities,
            "gear_notes": stage.map(|stage| stage.gear.iter().map(pick).collect::<Vec<_>>()),
            "open_slots": guide.as_ref().map(open_slots).unwrap_or_default(),
        });
    }

    if guide.is_none() {
        missing.push(
            "нет импортированного гайда для билда: готовые фильтры по слотам недоступны, поиск возможен по модам".into(),
        );
    }

    let mut goal = Value::Null;
    if let Some(campaign) = &campaign {
        let route = if saved.route.trim().is_empty() {
            "all"
        } else {
            saved.route.trim()
        };
        let step_index = profile.step.min(campaign.steps.len().saturating_sub(1));
        let current = campaign.step(step_index).map(|step| {
            json!({
                "index": step_index,
                "act": step.act,
                "zone": pick(&step.zone_label),
                "goal": pick(&step.goal),
                "optional": step.optional,
            })
        });
        let next = campaign.next_destination(step_index, route).map(|step| {
            json!({
                "act": step.act,
                "zone": pick(&step.zone_label),
                "goal": pick(&step.goal),
                "optional": step.optional,
            })
        });
        let (position, total) = campaign.progress(step_index, route);
        let zone_progress = campaign.zone_progress(
            &profile.zone,
            &profile.done,
            step_index,
            route,
            &saved.zone_aliases,
        );
        goal = json!({
            "route": route,
            "current": current,
            "next": next,
            "position": position,
            "total": total,
            "zone_progress": {
                "zone": zone_progress.zone,
                "known": zone_progress.known,
                "next_goal": zone_progress.next_goal,
                "next_zone": zone_progress.next_zone,
                "objectives_left": zone_progress.objectives.iter()
                    .filter(|objective| !objective.done)
                    .map(|objective| json!({
                        "goal": pick(&objective.goal),
                        "optional": objective.optional,
                    }))
                    .collect::<Vec<_>>(),
            },
        });
    } else {
        missing.push("маршрут кампании не загрузился".into());
    }

    let stale = age
        .map(|seconds| seconds > STATE_MAX_AGE_SECONDS)
        .unwrap_or(true);
    if stale && ctx.state_path().exists() {
        missing.push(format!(
            "state.json не обновлялся {} — данные могут быть старыми; запусти Eni-PoE2-Companion",
            human_age(age.unwrap_or(0))
        ));
    }

    json!({
        "protocol": PROTOCOL,
        "ok": true,
        "stale": stale,
        "state_age_seconds": age,
        "character": {
            "name": profile.name,
            "class": profile.class,
            "ascendancy": profile.ascendancy,
            "level": profile.level,
            "act": profile.act,
            "zone": profile.zone,
            "step": profile.step,
        },
        "build": build_info,
        "goal": goal,
        "improve": improve,
        "guide": guide.as_ref().map(|guide| json!({
            "id": guide.id,
            "title": guide.title,
            "variant": guide.variant,
            "open_slots": open_slots(guide),
            "marked_slots": marked_slots(guide),
        })),
        "missing": missing,
    })
}

fn human_age(seconds: u64) -> String {
    if seconds < 120 {
        format!("{seconds} с назад")
    } else if seconds < 7200 {
        format!("{} мин назад", seconds / 60)
    } else {
        format!("{} ч назад", seconds / 3600)
    }
}

fn is_weapon_slot(slot: &str) -> bool {
    matches!(
        slot,
        "mainHand" | "offHand" | "weapon" | "bow" | "crossbow" | "wand" | "staff" | "quarterstaff"
    )
}

fn open_slots(guide: &GuideRecord) -> Vec<String> {
    guide
        .gear
        .iter()
        .filter(|item| {
            !guide
                .owned
                .iter()
                .any(|key| key == &format!("gear:{}", item.slot))
        })
        .map(|item| item.slot.clone())
        .collect()
}

fn marked_slots(guide: &GuideRecord) -> Vec<String> {
    guide
        .gear
        .iter()
        .filter(|item| {
            guide
                .owned
                .iter()
                .any(|key| key == &format!("gear:{}", item.slot))
        })
        .map(|item| item.slot.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Decision memory
// ---------------------------------------------------------------------------

pub fn remember(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let saved = ctx.saved();
    let profile = saved.profile().ok_or_else(|| {
        ApiError::new(
            "no_profile",
            "Нет активного профиля: не к чему привязать решение.",
        )
    })?;
    let build_id = active_build_id(&saved);
    if build_id.trim().is_empty() {
        return Err(ApiError::new(
            "no_build",
            "У профиля не выбран билд: решение нельзя привязать к билду.",
        ));
    }
    let build = ctx.build(&build_id);
    let stage = build.as_ref().and_then(|doc| stage_for(doc, profile.level));

    let mut new: decisions::NewDecision = serde_json::from_value(params).map_err(|error| {
        ApiError::new("bad_params", format!("Не разобраны поля решения: {error}"))
    })?;
    if new.build_id.trim().is_empty() {
        new.build_id = build_id.clone();
    }
    if new.build_name.trim().is_empty() {
        new.build_name = build
            .as_ref()
            .map(|doc| pick(&doc.name))
            .unwrap_or_default();
    }
    if new.patch.trim().is_empty() {
        new.patch = build
            .as_ref()
            .map(|doc| doc.patch.clone())
            .unwrap_or_default();
    }
    if new.stage_id.trim().is_empty() {
        new.stage_id = stage.map(|stage| stage.id.clone()).unwrap_or_default();
    }
    if new.stage_title.trim().is_empty() {
        new.stage_title = stage.map(|stage| pick(&stage.title)).unwrap_or_default();
    }

    let record = decisions::add(&ctx.decisions_path(), new)
        .map_err(|error| ApiError::new("decision_rejected", error))?;
    Ok(json!({ "ok": true, "record": record }))
}

pub fn list_decisions(ctx: &Context, params: Value) -> Value {
    let mut filter: decisions::DecisionFilter = serde_json::from_value(params).unwrap_or_default();
    if filter.build_id.trim().is_empty() {
        filter.build_id = active_build_id(&ctx.saved());
    }
    let records = decisions::list(&ctx.decisions_path(), &filter);
    json!({ "ok": true, "count": records.len(), "records": records })
}

pub fn delete_decision(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(ApiError::new("bad_params", "Не указан id записи."));
    }
    let deleted = decisions::delete(&ctx.decisions_path(), &id)
        .map_err(|error| ApiError::new("delete_failed", error))?;
    Ok(json!({ "ok": true, "deleted": deleted, "id": id }))
}

pub fn similar_decisions(ctx: &Context, params: Value) -> Value {
    let situation = params
        .get("situation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let saved = ctx.saved();
    let build_id = params
        .get("build_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| active_build_id(&saved));
    let patch = params
        .get("patch")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| ctx.build(&build_id).map(|doc| doc.patch))
        .unwrap_or_default();
    let stage_id = params
        .get("stage_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(3) as usize;

    let hits = decisions::similar(
        &ctx.decisions_path(),
        &build_id,
        &patch,
        &stage_id,
        situation,
        limit,
    );
    json!({
        "ok": true,
        "build_id": build_id,
        "patch": patch,
        "count": hits.len(),
        "examples": hits,
    })
}

// ---------------------------------------------------------------------------
// Item evaluation
// ---------------------------------------------------------------------------

const ITEM_VERDICT_FITS: &str = "fits_build";
const ITEM_VERDICT_UPGRADE: &str = "likely_upgrade";
const ITEM_VERDICT_WORSE: &str = "not_better";

pub fn evaluate_item(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError::new(
            "no_item_text",
            "Нет текста предмета: скопируй предмет в игре и повтори.",
        ));
    }
    let parsed = crate::item::parse(text).ok_or_else(|| {
        ApiError::new(
            "item_not_parsed",
            "Текст не похож на предмет PoE 2: скопируй его в игре (Ctrl+C).",
        )
    })?;
    let saved = ctx.saved();
    let profile = saved.profile();
    let level = profile.map(|profile| profile.level).unwrap_or(1);
    let analysis = crate::item::analyze(&parsed, level);
    let build_id = active_build_id(&saved);
    let build = ctx.build(&build_id);

    let priorities = build
        .as_ref()
        .map(|build| {
            let mut out: Vec<String> = build.priorities.stats.iter().map(pick).collect();
            let group = if is_weapon_slot(&parsed.slot) {
                &build.priorities.weapon
            } else if is_accessory_slot(&parsed.slot) {
                &build.priorities.accessories
            } else {
                &build.priorities.armour
            };
            out.extend(group.iter().map(pick));
            out
        })
        .unwrap_or_default();
    let priority_matches = match_priorities(&priorities, &parsed.mods);

    let equipped = saved
        .items
        .iter()
        .filter(|record| record.parsed.slot == parsed.slot)
        .last()
        .cloned();

    let (verdict, reason, equipped_known) = match &equipped {
        None => (
            ITEM_VERDICT_FITS.to_string(),
            if analysis.score > 0 {
                "Подходит под билд: надетый предмет в приложении не импортирован, поэтому это не сравнение с апгрейдом.".to_string()
            } else {
                "Слабых совпадений с приоритетами билда нет; надетый предмет неизвестен, назвать апгрейдом нельзя.".to_string()
            },
            false,
        ),
        Some(record) => {
            let delta = analysis.score - record.analysis.score;
            if delta > 0 {
                (
                    ITEM_VERDICT_UPGRADE.to_string(),
                    format!(
                        "По эвристике приложения сильнее надетого на {delta} балл(ов); это не расчёт DPS.",
                    ),
                    true,
                )
            } else {
                (
                    ITEM_VERDICT_WORSE.to_string(),
                    format!(
                        "По эвристике приложения не лучше надетого (разница {delta}); сравни моды вручную.",
                    ),
                    true,
                )
            }
        }
    };

    Ok(json!({
        "ok": true,
        "parsed": parsed,
        "analysis": analysis,
        "build": if build_id.is_empty() { Value::Null } else { json!({ "id": build_id }) },
        "priority_matches": priority_matches,
        "equipped_known": equipped_known,
        "equipped": equipped.as_ref().map(|record| json!({
            "name": record.parsed.name,
            "score": record.analysis.score,
            "ts": record.ts,
        })),
        "verdict": verdict,
        "reason": reason,
        "price": Value::Null,
        "price_note": "Цена не запрашивалась: скажи «сколько стоит», чтобы получить снимок лотов (не сделок).",
    }))
}

fn is_accessory_slot(slot: &str) -> bool {
    matches!(
        slot,
        "ring" | "leftRing" | "rightRing" | "amulet" | "belt" | "gloves"
    )
}

fn match_priorities(priorities: &[String], mods: &[String]) -> Vec<String> {
    let mods_lower: Vec<String> = mods.iter().map(|line| line.to_lowercase()).collect();
    priorities
        .iter()
        .filter(|priority| {
            let words: Vec<String> = priority
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric() && c != '-')
                .filter(|word| word.len() >= 4)
                .map(str::to_string)
                .collect();
            words
                .iter()
                .any(|word| mods_lower.iter().any(|line| line.contains(word)))
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Trade search and budget
// ---------------------------------------------------------------------------

/// Deep merge used to add a price cap or a category without dropping the
/// filters the query already had (for example the rarity filter).
fn deep_merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                deep_merge(
                    target_map.entry(key.clone()).or_insert_with(|| json!({})),
                    value,
                );
            }
        }
        (target_slot, value) => *target_slot = value.clone(),
    }
}

fn merge_filters(query: &mut Value, patch: Value) {
    if !query.is_object() {
        *query = json!({});
    }
    let filters = query
        .as_object_mut()
        .unwrap()
        .entry("filters")
        .or_insert_with(|| json!({}));
    if !filters.is_object() {
        *filters = json!({});
    }
    deep_merge(filters, &patch);
}

fn price_patch(amount: f64, currency: &str) -> Value {
    json!({
        "trade_filters": {
            "filters": {
                "price": { "max": amount, "option": if currency.trim().is_empty() { "exalted" } else { currency.trim() } }
            }
        }
    })
}

fn category_patch(category: &str) -> Value {
    json!({
        "type_filters": {
            "filters": { "category": { "option": category.trim() } }
        }
    })
}

pub fn search_upgrades(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let saved = ctx.saved();
    let league = params
        .get("league")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| ctx.preferred_league(&saved));
    if league.trim().is_empty() {
        return Err(ApiError::new(
            "no_league",
            "Лига не определена: укажи лигу в Eni-PoE2 или в запросе.",
        ));
    }

    let slot = params
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let auto = params.get("auto").and_then(Value::as_bool).unwrap_or(false);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(3)
        .clamp(1, 3) as usize;
    let budget: budget::Budget = params
        .get("budget")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| ApiError::new("bad_budget", format!("Бюджет не разобран: {error}")))?
        .unwrap_or_default();
    let rates_override: Option<budget::Rates> = params
        .get("rates")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| ApiError::new("bad_rates", format!("Курс не разобран: {error}")))?;
    let mut rates = rates_override.unwrap_or_else(|| ctx.rates_for(&saved, &league));
    if rates.league.is_empty() {
        rates.league = league.clone();
    }

    let stats = ctx.stats();
    let headroom = saved.trade.headroom;
    let filter_limit = saved.trade.filters.max(1) as usize;

    let guide = ctx.guide_for(&saved, &active_build_id(&saved));
    let guide_item = guide
        .as_ref()
        .and_then(|guide| guide.gear.iter().find(|item| item.slot == slot))
        .cloned();

    let mut notes: Vec<String> =
        vec!["Цены лотов — снимок выставленных объявлений, а не цены состоявшихся сделок.".into()];
    if auto {
        notes.push(
            "Автополучение лотов включено явно; при недоступности API будет только ссылка.".into(),
        );
    } else {
        notes.push(
            "Лоты не запрашивались: ниже готовая ссылка на официальный поиск с фильтрами.".into(),
        );
    }

    // 1) Query: a ready Mobalytics query for the guide slot, otherwise mods.
    let mut matched: Vec<String> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();
    let mut query: Value;

    if let Some(item) = guide_item
        .as_ref()
        .filter(|item| !item.trade_query.trim().is_empty())
    {
        let body: Value = serde_json::from_str(&item.trade_query).map_err(|error| {
            ApiError::new(
                "bad_guide_query",
                format!("Запрос гайда не разобран: {error}"),
            )
        })?;
        query = body.get("query").cloned().unwrap_or(body);
        notes.push(format!(
            "Использован готовый запрос гайда для слота {slot}."
        ));
    } else {
        let mods: Vec<String> = params
            .get("mods")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if mods.is_empty() {
            return Err(ApiError::new(
                "no_filters",
                "Нет ни готового запроса гайда, ни модов для фильтра. Скажи, какие моды важны.",
            ));
        }
        let mut parsed = crate::item::ParsedItem {
            mods: mods.clone(),
            slot: slot.clone(),
            ..Default::default()
        };
        if let Some(base) = params.get("base").and_then(Value::as_str) {
            parsed.base = base.trim().to_string();
        }
        if let Some(name) = params.get("name").and_then(Value::as_str) {
            parsed.name = name.trim().to_string();
        }
        if let Some(rarity) = params.get("rarity").and_then(Value::as_str) {
            parsed.rarity = rarity.trim().to_string();
        }

        // Same two matchers the app already uses: the bundled RU table first
        // (offline, always available), then the downloaded stat catalogue.
        let (ru_filters, _ru_unmatched) = crate::trade_i18n::filters(&mods, filter_limit, headroom);
        let (filters, unmatched_mods) = if !ru_filters.is_empty() {
            (ru_filters, Vec::new())
        } else if let Some(dataset) = stats.as_deref() {
            trade::match_stats(&parsed, dataset, filter_limit, headroom)
        } else {
            (Vec::new(), mods.clone())
        };
        unmatched = unmatched_mods;

        matched = filters.iter().map(|filter| filter.text.clone()).collect();
        query = trade::build_query_with(&parsed, &filters);
        if filters.is_empty() {
            notes.push(
                "Моды не сопоставлены со статами trade API: фильтры по модам не применены.".into(),
            );
        }
    }

    // 2) Hard price cap in the query itself, so the site cannot return lots
    // above the budget even if the API call is skipped.
    let currency = budget
        .global
        .as_ref()
        .map(|money| money.currency.clone())
        .or_else(|| budget.per_item.as_ref().map(|money| money.currency.clone()))
        .unwrap_or_else(|| budget::EXALTED.to_string());
    if let Some(global) = budget.global.as_ref() {
        let converted = budget::convert(global.amount, &global.currency, &currency, &rates);
        if let Some(amount) = converted {
            merge_filters(&mut query, price_patch(amount, &currency));
        } else {
            notes.push("Курс неизвестен: ценовой предел в запрос не подставлен.".into());
        }
    }
    if let Some(category) = params.get("category").and_then(Value::as_str) {
        if !category.trim().is_empty() && category != "any" {
            merge_filters(&mut query, category_patch(category));
        }
    }

    let search_url = trade::build_trade_url(&league, &query);
    let body = json!({ "query": query, "sort": { "price": "asc" } }).to_string();

    // 3) Optional live lots.
    let mut listings: Vec<Value> = Vec::new();
    let mut plan: Option<budget::PlanResult> = None;
    let mut auto_error: Option<String> = None;
    if auto {
        match trade::search_raw(&body, &league) {
            Ok((rows, url)) => {
                let quotes: Vec<budget::Quote> = rows
                    .iter()
                    .take(limit)
                    .enumerate()
                    .map(|(index, row)| budget::Quote {
                        slot: slot.clone(),
                        name: row.name.clone(),
                        price: budget::Money::new(row.price, &row.currency),
                        league: league.clone(),
                        url: url.clone(),
                        source: "trade".into(),
                        priority: index as u32,
                    })
                    .collect();
                for row in rows.iter().take(limit) {
                    listings.push(json!({
                        "name": row.name,
                        "base": row.base,
                        "price": row.price,
                        "currency": row.currency,
                        "account": row.account,
                        "online": row.online,
                        "status": row.status,
                        "url": url,
                    }));
                }
                if budget.global.is_some() || budget.per_item.is_some() {
                    plan = Some(budget::plan(&quotes, &budget, &rates, &league));
                }
                if rows.is_empty() {
                    notes.push("Лотов по этим фильтрам сейчас нет; ниже ссылка на поиск.".into());
                }
            }
            Err(error) => {
                auto_error = Some(error.clone());
                notes.push(format!(
                    "Автополучение лотов недоступно ({error}); ссылка с фильтрами остаётся рабочей."
                ));
            }
        }
    }

    Ok(json!({
        "ok": true,
        "slot": slot,
        "league": league,
        "search_url": search_url,
        "query": body,
        "filters": { "matched": matched, "unmatched": unmatched },
        "listings": listings,
        "auto_requested": auto,
        "auto_error": auto_error,
        "budget": if budget.global.is_some() || budget.per_item.is_some() {
            Some(json!({ "global": budget.global, "per_item": budget.per_item, "currency": currency }))
        } else { None },
        "plan": plan,
        "rates": rates,
        "notes": notes,
    }))
}

pub fn plan_budget(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let saved = ctx.saved();
    let league = params
        .get("league")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| ctx.preferred_league(&saved));
    let quotes: Vec<budget::Quote> = params
        .get("items")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| ApiError::new("bad_items", format!("Позиции не разобраны: {error}")))?
        .unwrap_or_default();
    if quotes.is_empty() {
        return Err(ApiError::new(
            "no_items",
            "Нет позиций: передай цены предметов или найди их через trade search.",
        ));
    }
    let budget: budget::Budget = params
        .get("budget")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| ApiError::new("bad_budget", format!("Бюджет не разобран: {error}")))?
        .unwrap_or_default();
    let rates = params
        .get("rates")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| ApiError::new("bad_rates", format!("Курс не разобран: {error}")))?
        .unwrap_or_else(|| ctx.rates_for(&saved, &league));

    let result = budget::plan(&quotes, &budget, &rates, &league);
    Ok(json!({ "ok": true, "plan": result }))
}

pub fn price_item(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError::new(
            "no_item_text",
            "Нет текста предмета для проверки цены.",
        ));
    }
    let parsed = crate::item::parse(text)
        .ok_or_else(|| ApiError::new("item_not_parsed", "Текст не похож на предмет PoE 2."))?;
    let saved = ctx.saved();
    let league = params
        .get("league")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| ctx.preferred_league(&saved));
    if league.trim().is_empty() {
        return Err(ApiError::new("no_league", "Лига не определена."));
    }
    let limit = saved.trade.filters.max(1) as usize;
    let headroom = saved.trade.headroom;
    let stats = ctx.stats();
    let (filters, unmatched) = if let Some(dataset) = stats.as_deref() {
        trade::match_stats(&parsed, dataset, limit, headroom)
    } else {
        let (filters, _) = crate::trade_i18n::filters(&parsed.mods, limit, headroom);
        (filters, Vec::new())
    };
    match trade::check_with(
        &parsed,
        &league,
        stats.as_deref(),
        limit,
        headroom,
        Some(filters),
    ) {
        Ok(mut result) => {
            result.unmatched.extend(unmatched);
            result.note = format!(
                "{} Снимок лотов на момент запроса, не цена сделки.",
                result.note
            );
            Ok(json!({ "ok": true, "price": result }))
        }
        Err(error) => Err(ApiError::new("price_unavailable", error)),
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub fn dispatch(ctx: &Context, method: &str, params: Value) -> Result<Value, ApiError> {
    match method {
        "meta" => Ok(meta(ctx)),
        "state" => Ok(state_summary(ctx)),
        "decisions.add" => remember(ctx, params),
        "decisions.list" => Ok(list_decisions(ctx, params)),
        "decisions.delete" => delete_decision(ctx, params),
        "decisions.similar" => Ok(similar_decisions(ctx, params)),
        "item.evaluate" => evaluate_item(ctx, params),
        "trade.search" => search_upgrades(ctx, params),
        "trade.price" => price_item(ctx, params),
        "budget.plan" => plan_budget(ctx, params),
        "rates" => Ok(rates_view(ctx, params)),
        other => Err(ApiError::new(
            "unknown_method",
            format!("Неизвестный метод моста: {other}"),
        )),
    }
}

pub fn meta(ctx: &Context) -> Value {
    let saved = ctx.saved();
    let builds = crate::builds::load_library(Some(&ctx.builds_dir()));
    json!({
        "protocol": PROTOCOL,
        "version": env!("CARGO_PKG_VERSION"),
        "data_dir": ctx.dir,
        "state_path": ctx.state_path(),
        "state_age_seconds": ctx.state_age(),
        "active_build": active_build_id(&saved),
        "league": ctx.preferred_league(&saved),
        "rates": ctx.rates_for(&saved, ""),
        "builds": builds.iter().map(|doc| json!({
            "id": doc.id, "name": pick(&doc.name), "class": doc.class, "patch": doc.patch
        })).collect::<Vec<_>>(),
        "guides": saved.guides.iter().map(|guide| json!({
            "id": guide.id, "build_id": guide.build_id, "title": guide.title
        })).collect::<Vec<_>>(),
        "decisions": decisions::load(&ctx.decisions_path()).records.len(),
        "session_max_age_seconds": SESSION_MAX_AGE_SECONDS,
    })
}

pub fn rates_view(ctx: &Context, params: Value) -> Value {
    let saved = ctx.saved();
    let league = params
        .get("league")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let rates = ctx.rates_for(&saved, league);
    json!({ "ok": true, "rates": rates, "note": rates.note() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GuideRecord;

    fn temp_ctx(name: &str) -> Context {
        let dir = std::env::temp_dir().join(format!(
            "eni-companion-{name}-{}-{}",
            std::process::id(),
            now_secs() as u64
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = Context::at(&dir);
        let mut saved = Saved::default();
        let profile_id = saved.active.clone();
        if let Some(profile) = saved.profiles.iter_mut().find(|p| p.id == profile_id) {
            profile.name = "Enisam".into();
            profile.class = "Снайпер".into();
            profile.level = 24;
            profile.act = 2;
            profile.zone = "Караван Ардура".into();
            profile.step = 31;
            profile.build_id = "lightning-arrow-deadeye".into();
        }
        model::save_state(&ctx.state_path(), &saved).unwrap();
        ctx
    }

    fn sample_item() -> &'static str {
        "Rarity: Rare\nDoom Loop\nPrismatic Ring\n--------\nItem Level: 65\n--------\n+25 to maximum Life\n+20% to Fire Resistance\n"
    }

    #[test]
    fn state_reports_progress_and_missing_pieces() {
        let ctx = temp_ctx("state");
        let state = state_summary(&ctx);
        assert_eq!(state["ok"], true);
        assert_eq!(state["character"]["level"], 24);
        assert_eq!(state["build"]["id"], "lightning-arrow-deadeye");
        // No guides imported in a fresh profile.
        assert!(state["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("гайда")));
        assert!(state["goal"]["next"].is_object() || state["goal"]["current"].is_object());
    }

    #[test]
    fn decisions_survive_a_new_context() {
        let ctx = temp_ctx("decisions");
        let saved = ctx.saved();
        let build = ctx.build("lightning-arrow-deadeye").unwrap();
        let patch = build.patch.clone();
        let added = remember(
            &ctx,
            json!({
                "situation": "Не хватает маны на боссе во втором акте",
                "advice": "Взять узел на ману",
                "decision": "Взял узел на ускорение",
                "reason": "Быстрее чищу паки, маны хватает с флаконом",
            }),
        )
        .unwrap();
        assert_eq!(added["ok"], true);

        // A fresh Context is what the bridge builds after a restart.
        let reopened = Context::at(&ctx.dir);
        let listed = list_decisions(&reopened, json!({}));
        assert_eq!(listed["count"], 1);
        let hits = similar_decisions(
            &reopened,
            json!({ "situation": "Не хватает маны на боссе", "build_id": active_build_id(&saved) }),
        );
        assert!(hits["count"].as_u64().unwrap_or(0) >= 1);
        let _ = (saved, patch);
    }

    #[test]
    fn unknown_equipped_item_is_not_called_an_upgrade() {
        let ctx = temp_ctx("equip");
        let result = evaluate_item(&ctx, json!({ "text": sample_item() })).unwrap();
        assert_eq!(result["equipped_known"], false);
        assert_eq!(result["verdict"], ITEM_VERDICT_FITS);
        assert!(result["reason"]
            .as_str()
            .unwrap()
            .contains("не импортирован"));
    }

    #[test]
    fn search_without_auto_returns_filters_and_link_only() {
        let ctx = temp_ctx("search");
        let result = search_upgrades(
            &ctx,
            json!({
                "slot": "boots",
                "category": "armour.boots",
                "mods": ["+60 to maximum Life", "+25% to Fire Resistance"],
                "league": "Standard",
                "budget": { "global": { "amount": 10, "currency": "exalted" } },
                "auto": false,
            }),
        )
        .unwrap();
        assert_eq!(result["ok"], true);
        assert!(result["search_url"]
            .as_str()
            .unwrap()
            .starts_with("https://www.pathofexile.com/trade2/search/poe2/Standard?q="));
        assert!(result["listings"].as_array().unwrap().is_empty());
        // The cap must be inside the query, not only in prose.
        let body = result["query"].as_str().unwrap();
        assert!(body.contains("\"max\":10.0") || body.contains("\"max\":10"));
        assert!(result["notes"].as_array().unwrap().iter().any(|note| note
            .as_str()
            .unwrap()
            .contains("не цены состоявшихся сделок")));
    }

    #[test]
    fn hard_budget_blocks_two_items_over_the_limit() {
        let ctx = temp_ctx("budget");
        let result = plan_budget(
            &ctx,
            json!({
                "league": "Standard",
                "items": [
                    { "slot": "boots", "name": "Boots", "price": { "amount": 15, "currency": "exalted" }, "league": "Standard" },
                    { "slot": "gloves", "name": "Gloves", "price": { "amount": 15, "currency": "exalted" }, "league": "Standard" }
                ],
                "budget": { "global": { "amount": 20, "currency": "exalted" } },
                "rates": { "league": "Standard", "exalted_per_divine": 10.0, "source": "test", "as_of": "now" }
            }),
        )
        .unwrap();
        assert_eq!(result["plan"]["fits"], false);
        assert!(result["plan"]["accepted"].as_array().unwrap().len() < 2);
    }

    #[test]
    fn unknown_rate_never_claims_a_set_fits() {
        let ctx = temp_ctx("rates");
        let result = plan_budget(
            &ctx,
            json!({
                "league": "Standard",
                "items": [
                    { "slot": "boots", "name": "Boots", "price": { "amount": 2, "currency": "divine" }, "league": "Standard" }
                ],
                "budget": { "global": { "amount": 20, "currency": "exalted" } }
            }),
        )
        .unwrap();
        assert_eq!(result["plan"]["budget_known"], false);
        assert_eq!(result["plan"]["fits"], false);
    }
}
