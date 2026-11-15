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

    let last_event = last_game_event(&saved);
    let game_age = last_event
        .as_ref()
        .map(|(at, _)| (chrono::Utc::now() - *at).num_seconds().max(0) as u64);
    let zone_basis = match game_age {
        Some(seconds) if seconds <= STATE_MAX_AGE_SECONDS => "live",
        _ => "last_known",
    };
    let stale = zone_basis != "live";
    let client_active = client_log_active(&saved);
    if stale && ctx.state_path().exists() {
        missing.push(format!(
            "последняя известная зона: {} (последнее игровое событие {}; изменения настроек и профиля не считаются активностью клиента)",
            profile.zone,
            game_age
                .map(human_age)
                .unwrap_or_else(|| "неизвестно когда".to_string())
        ));
    }

    json!({
        "protocol": PROTOCOL,
        "ok": true,
        "stale": stale,
        "zone_basis": zone_basis,
        "state_age_seconds": age,
        "game": {
            "last_event_at": last_event.as_ref().map(|(at, _)| at.to_rfc3339()),
            "last_event_kind": last_event.as_ref().map(|(_, kind)| kind.clone()),
            "age_seconds": game_age,
            "client_active": client_active,
            "zone_basis": zone_basis,
        },
        "profile_updated": profile.updated,
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

fn parse_iso(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

/// Last event that came from the game log. Manual profile edits are not game
/// activity and must not make an old zone look live.
fn last_game_event(saved: &Saved) -> Option<(chrono::DateTime<chrono::Utc>, String)> {
    saved
        .history
        .iter()
        .filter(|event| {
            matches!(
                event.kind.as_str(),
                "zone" | "level" | "reward" | "death" | "map"
            )
        })
        .filter_map(|event| parse_iso(&event.ts).map(|at| (at, event.kind.clone())))
        .max_by_key(|(at, _)| *at)
}

/// True only while the game log file itself was written recently.
fn client_log_active(saved: &Saved) -> bool {
    let path = saved
        .log
        .path
        .clone()
        .or_else(crate::discover::find_client_log);
    let Some(path) = path else {
        return false;
    };
    let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
        return false;
    };
    modified
        .elapsed()
        .map(|elapsed| elapsed.as_secs() < 180)
        .unwrap_or(false)
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

    let equipped_text = params
        .get("equipped_text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let equipped = equipped_text
        .and_then(crate::item::parse)
        .filter(|item| item.slot == parsed.slot);
    let equipped_analysis = equipped
        .as_ref()
        .map(|item| crate::item::analyze(item, level));

    let (verdict, reason, equipped_known) = match (&equipped, &equipped_analysis) {
        (Some(item), Some(equipped_analysis)) => {
            let delta = analysis.score - equipped_analysis.score;
            if delta > 0 {
                (
                    ITEM_VERDICT_UPGRADE.to_string(),
                    format!(
                        "По эвристике приложения сильнее явно переданного надетого {} на {delta} балл(ов); это не расчёт DPS.",
                        item.slot
                    ),
                    true,
                )
            } else {
                (
                    ITEM_VERDICT_WORSE.to_string(),
                    format!(
                        "По эвристике приложения не лучше явно переданного надетого {} (разница {delta}); сравни моды вручную.",
                        item.slot
                    ),
                    true,
                )
            }
        }
        _ => (
            ITEM_VERDICT_FITS.to_string(),
            if analysis.score > 0 {
                "Подходит под билд. Надетый предмет для сравнения не передан, поэтому это не апгрейд: сравнивать не с чем.".to_string()
            } else {
                "Слабых совпадений с приоритетами билда нет; надетый предмет не передан, назвать апгрейдом нельзя.".to_string()
            },
            false,
        ),
    };

    Ok(json!({
        "ok": true,
        "parsed": parsed,
        "analysis": analysis,
        "build": if build_id.is_empty() { Value::Null } else { json!({ "id": build_id }) },
        "priority_matches": priority_matches,
        "equipped_known": equipped_known,
        "equipped_source": if equipped_known { "explicit" } else { "none" },
        "equipped": equipped.as_ref().map(|item| json!({
            "name": item.name,
            "slot": item.slot,
            "score": equipped_analysis.as_ref().map(|analysis| analysis.score),
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

// ---------------------------------------------------------------------------
// Multi-slot planning for the voice assistant
// ---------------------------------------------------------------------------

/// One build-relevant modifier, resolved to an official trade stat id.
#[derive(Clone, Copy, Debug)]
pub struct StatSpec {
    pub id: &'static str,
    pub text: &'static str,
    pub default_min: f64,
}

/// The symbolic keys the assistant may send. Ids come from the bundled
/// `assets/trade_i18n/ru_stats.json` table, not from guesses.
pub fn resolve_stat_key(key: &str) -> Option<StatSpec> {
    let spec = match key.trim().to_lowercase().as_str() {
        "life" | "maximum_life" => StatSpec {
            id: "explicit.stat_3299347043",
            text: "+# to maximum Life",
            default_min: 50.0,
        },
        "mana" | "maximum_mana" => StatSpec {
            id: "explicit.stat_1050105434",
            text: "+# to maximum Mana",
            default_min: 30.0,
        },
        "cold_res" | "cold_resistance" => StatSpec {
            id: "explicit.stat_4220027924",
            text: "+#% to Cold Resistance",
            default_min: 20.0,
        },
        "fire_res" | "fire_resistance" => StatSpec {
            id: "explicit.stat_3372524247",
            text: "+#% to Fire Resistance",
            default_min: 20.0,
        },
        "lightning_res" | "lightning_resistance" => StatSpec {
            id: "explicit.stat_1671376347",
            text: "+#% to Lightning Resistance",
            default_min: 20.0,
        },
        "chaos_res" | "chaos_resistance" => StatSpec {
            id: "explicit.stat_2923486259",
            text: "+#% to Chaos Resistance",
            default_min: 10.0,
        },
        "movement_speed" => StatSpec {
            id: "explicit.stat_2250533757",
            text: "#% increased Movement Speed",
            default_min: 25.0,
        },
        "attack_speed" => StatSpec {
            id: "explicit.stat_210067635",
            text: "#% increased Attack Speed",
            default_min: 10.0,
        },
        _ => return None,
    };
    Some(spec)
}

/// Broad PoE2 trade categories are the only ones the official API exposes
/// (`armour`, `accessory`, `weapon`, ...). Slot precision comes from the stat
/// filters, which is why the applied filters are verified in the final query.
pub fn category_for_slot(slot: &str) -> &'static str {
    match slot {
        "body" | "helmet" | "gloves" | "boots" | "shield" => "armour",
        "amulet" | "belt" | "ring" | "leftRing" | "rightRing" => "accessory",
        "mainHand" | "offHand" | "weapon" | "bow" | "crossbow" | "wand" | "staff" => "weapon",
        _ => "",
    }
}

#[derive(serde::Deserialize, Debug, Clone, Default)]
pub struct FilterRequest {
    #[serde(default)]
    pub stat: String,
    #[serde(default)]
    pub min: Option<f64>,
}

#[derive(serde::Deserialize, Debug, Clone, Default)]
pub struct PlanSlotRequest {
    #[serde(default)]
    pub slot: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub filters: Vec<FilterRequest>,
}

#[derive(serde::Deserialize, Debug, Clone, Default)]
struct PlanRequest {
    #[serde(default)]
    league: String,
    #[serde(default)]
    auto: bool,
    #[serde(default)]
    slots: Vec<PlanSlotRequest>,
    #[serde(default)]
    budget: Option<budget::Budget>,
    #[serde(default)]
    rates: Option<budget::Rates>,
    /// Explicit quotes instead of a live search (deterministic tests and the
    /// "реальные кандидаты" hand-off). Never mixed with fetched alternatives.
    #[serde(default)]
    quotes: Vec<budget::Quote>,
}

pub const NO_LOTS_MESSAGE: &str =
    "Лоты не получены, итоговую стоимость комплекта проверить не могу.";

struct SlotPlan {
    slot: String,
    url: String,
    body: Value,
    applied: Vec<Value>,
    unmatched: Vec<String>,
    notes: Vec<String>,
    lot: Option<budget::Quote>,
    listings_count: usize,
    error: Option<String>,
}

fn explicit_filters(slot: &PlanSlotRequest) -> (Vec<trade::StatFilter>, Vec<Value>, Vec<String>) {
    let mut filters = Vec::new();
    let mut applied = Vec::new();
    let mut unmatched = Vec::new();
    for request in &slot.filters {
        match resolve_stat_key(&request.stat) {
            Some(spec) => {
                let min = request.min.unwrap_or(spec.default_min).max(1.0);
                filters.push(trade::StatFilter {
                    id: spec.id.to_string(),
                    min,
                    max: None,
                    text: spec.text.to_string(),
                });
                applied.push(json!({
                    "stat": request.stat,
                    "id": spec.id,
                    "min": min,
                    "text": spec.text,
                }));
            }
            None => unmatched.push(format!("неизвестный стат: {}", request.stat)),
        }
    }
    (filters, applied, unmatched)
}

fn append_stat_filters(query: &mut Value, filters: &[trade::StatFilter]) {
    if filters.is_empty() {
        return;
    }
    let list: Vec<Value> = filters
        .iter()
        .map(|filter| {
            json!({
                "id": filter.id,
                "value": { "min": filter.min },
                "disabled": false,
            })
        })
        .collect();
    if !query.is_object() {
        *query = json!({});
    }
    let stats = query
        .as_object_mut()
        .unwrap()
        .entry("stats")
        .or_insert_with(|| json!([]));
    if !stats.is_array() {
        *stats = json!([]);
    }
    let groups = stats.as_array_mut().unwrap();
    let ids: Vec<String> = filters.iter().map(|filter| filter.id.clone()).collect();
    if let Some(group) = groups
        .iter_mut()
        .find(|group| group.get("type").and_then(Value::as_str) == Some("and"))
    {
        match group.get_mut("filters").and_then(Value::as_array_mut) {
            Some(existing) => {
                // An explicit requirement replaces the guide's value for the
                // same stat instead of sending duplicate ids.
                existing.retain(|item| {
                    item.get("id")
                        .and_then(Value::as_str)
                        .map(|id| !ids.iter().any(|ours| ours == id))
                        .unwrap_or(true)
                });
                existing.extend(list);
            }
            None => group["filters"] = json!(list),
        }
    } else {
        groups.push(json!({ "type": "and", "filters": list }));
    }
}

fn missing_stat_ids(query: &Value, filters: &[trade::StatFilter]) -> Vec<String> {
    let mut present: Vec<String> = Vec::new();
    if let Some(groups) = query.get("stats").and_then(Value::as_array) {
        for group in groups {
            if let Some(items) = group.get("filters").and_then(Value::as_array) {
                for item in items {
                    if item.get("disabled").and_then(Value::as_bool) == Some(true) {
                        continue;
                    }
                    if let Some(id) = item.get("id").and_then(Value::as_str) {
                        present.push(id.to_string());
                    }
                }
            }
        }
    }
    filters
        .iter()
        .filter(|filter| !present.iter().any(|id| id == &filter.id))
        .map(|filter| filter.id.clone())
        .collect()
}

fn build_slot_plan(
    ctx: &Context,
    saved: &Saved,
    request: &PlanSlotRequest,
    league: &str,
    budget: &budget::Budget,
    rates: &budget::Rates,
    currency: &str,
) -> SlotPlan {
    let mut notes: Vec<String> = Vec::new();
    let (filters, applied, mut unmatched) = explicit_filters(request);
    let guide = ctx.guide_for(saved, &active_build_id(saved));
    let guide_query = guide
        .as_ref()
        .and_then(|guide| guide.gear.iter().find(|item| item.slot == request.slot))
        .filter(|item| !item.trade_query.trim().is_empty())
        .map(|item| item.trade_query.clone());

    let mut query = if let Some(raw) = guide_query {
        match serde_json::from_str::<Value>(&raw) {
            Ok(body) => {
                notes.push(
                    "Использован готовый запрос гайда; явные требования добавлены в него.".into(),
                );
                body.get("query").cloned().unwrap_or(body)
            }
            Err(error) => {
                unmatched.push(format!("запрос гайда не разобран: {error}"));
                json!({ "status": { "option": "online" } })
            }
        }
    } else {
        json!({ "status": { "option": "online" } })
    };

    let category = if request.category.trim().is_empty() {
        category_for_slot(&request.slot)
    } else {
        request.category.trim()
    };
    if !category.is_empty() {
        merge_filters(&mut query, category_patch(category));
    }
    append_stat_filters(&mut query, &filters);
    let missing = missing_stat_ids(&query, &filters);
    for id in missing {
        unmatched.push(format!("фильтр не попал в итоговый запрос: {id}"));
    }

    // Hard price ceiling in the query itself.
    let cap = budget
        .per_item
        .as_ref()
        .and_then(|limit| {
            budget::convert(limit.amount, &limit.currency, currency, rates)
                .map(|amount| (amount, limit))
        })
        .or_else(|| {
            budget.global.as_ref().and_then(|limit| {
                budget::convert(limit.amount, &limit.currency, currency, rates)
                    .map(|amount| (amount, limit))
            })
        });
    match cap {
        Some((amount, limit)) => {
            let cap_currency = budget::normalize_currency(&limit.currency);
            merge_filters(&mut query, price_patch(amount, &cap_currency));
            notes.push(format!(
                "Ценовой предел в запросе: {amount:.0} {}.",
                budget::display_currency(&cap_currency)
            ));
        }
        None => notes
            .push("Ценовой предел не подставлен: курс или валюта бюджета не определены.".into()),
    }

    let url = trade::build_trade_url(league, &query);
    let body = json!({ "query": query, "sort": { "price": "asc" } });
    SlotPlan {
        slot: request.slot.clone(),
        url,
        body,
        applied,
        unmatched,
        notes,
        lot: None,
        listings_count: 0,
        error: None,
    }
}

fn plan_impl(
    ctx: &Context,
    saved: &Saved,
    request: PlanRequest,
    slots: Vec<PlanSlotRequest>,
) -> Result<Value, ApiError> {
    let league = {
        let requested = request.league.trim();
        if requested.is_empty() {
            ctx.preferred_league(saved)
        } else {
            requested.to_string()
        }
    };
    if league.trim().is_empty() {
        return Err(ApiError::new(
            "no_league",
            "Лига не определена: укажи лигу в Eni-PoE2 или в запросе.",
        ));
    }
    if slots.is_empty() {
        return Err(ApiError::new("no_slots", "Не указан ни один слот."));
    }

    let mut seen: Vec<String> = Vec::new();
    for slot in &slots {
        if slot.slot.trim().is_empty() {
            return Err(ApiError::new("no_slot", "У слота нет имени."));
        }
        if seen.iter().any(|name| name == &slot.slot) {
            return Err(ApiError::new(
                "duplicate_slot",
                format!(
                    "Слот {} указан дважды: альтернативные лоты одного слота не складываются в комплект.",
                    slot.slot
                ),
            ));
        }
        seen.push(slot.slot.clone());
    }

    let budget = request.budget.clone().unwrap_or_default();
    let rates = request
        .rates
        .clone()
        .unwrap_or_else(|| ctx.rates_for(saved, &league));
    let currency = budget
        .global
        .as_ref()
        .map(|money| budget::normalize_currency(&money.currency))
        .or_else(|| {
            budget
                .per_item
                .as_ref()
                .map(|money| budget::normalize_currency(&money.currency))
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| budget::EXALTED.to_string());

    let explicit_quotes = !request.quotes.is_empty();
    let mut plans: Vec<SlotPlan> = Vec::new();
    for slot in &slots {
        let mut plan = build_slot_plan(ctx, saved, slot, &league, &budget, &rates, &currency);
        if explicit_quotes {
            if let Some(quote) = request.quotes.iter().find(|quote| quote.slot == slot.slot) {
                plan.lot = Some(quote.clone());
                plan.listings_count = 1;
            } else {
                plan.error = Some("лот не передан".into());
            }
        } else if request.auto {
            match trade::search_raw(&plan.body.to_string(), &league) {
                Ok((rows, url)) => {
                    plan.listings_count = rows.len();
                    let chosen = rows.iter().find(|row| row.price > 0.0);
                    match chosen {
                        Some(row) => {
                            plan.lot = Some(budget::Quote {
                                slot: slot.slot.clone(),
                                name: row.name.clone(),
                                price: budget::Money::new(row.price, &row.currency),
                                league: league.clone(),
                                url: url.clone(),
                                source: "trade".into(),
                                priority: plans.len() as u32,
                            });
                        }
                        None => {
                            plan.error = Some("нет лотов с ценой".into());
                        }
                    }
                }
                Err(error) => {
                    plan.error = Some(error);
                }
            }
            // The official API asks for at most one request every few seconds.
            std::thread::sleep(std::time::Duration::from_millis(1600));
        }
        plans.push(plan);
    }

    let lots_received = plans.iter().all(|plan| plan.lot.is_some());

    let quotes: Vec<budget::Quote> = plans
        .iter()
        .enumerate()
        .filter_map(|(index, plan)| {
            plan.lot.as_ref().map(|lot| {
                let mut lot = lot.clone();
                lot.priority = index as u32;
                lot
            })
        })
        .collect();

    let plan_result = if lots_received && (!quotes.is_empty()) {
        Some(budget::plan(&quotes, &budget, &rates, &league))
    } else {
        None
    };

    let total_message = if !lots_received {
        NO_LOTS_MESSAGE.to_string()
    } else if let Some(result) = &plan_result {
        if result.fits {
            format!(
                "Комплект укладывается: {} {}{}",
                result
                    .set_total
                    .as_ref()
                    .map(|money| money.amount)
                    .unwrap_or(0.0),
                budget::display_currency(&currency),
                result
                    .remaining
                    .as_ref()
                    .map(|money| format!(", остаток {:.0}", money.amount))
                    .unwrap_or_default()
            )
        } else {
            "Комплект не укладывается в бюджет.".to_string()
        }
    } else {
        "Бюджет не указан: итоговую стоимость не проверяю.".to_string()
    };

    let slot_views: Vec<Value> = plans
        .iter()
        .map(|plan| {
            json!({
                "slot": plan.slot,
                "search_url": plan.url,
                "filters_applied": plan.applied,
                "filters_unmatched": plan.unmatched,
                "notes": plan.notes,
                "listings_count": plan.listings_count,
                "lot": plan.lot.as_ref().map(|lot| json!({
                    "name": lot.name,
                    "price": lot.price,
                    "currency": lot.price.currency,
                    "league": lot.league,
                    "url": lot.url,
                })),
                "error": plan.error,
            })
        })
        .collect();

    Ok(json!({
        "ok": true,
        "league": league,
        "lots_received": lots_received,
        "totals": {
            "message": total_message,
            "currency": currency,
            "fits": plan_result.as_ref().map(|plan| plan.fits),
            "set_total": plan_result.as_ref().and_then(|plan| plan.set_total.clone()),
            "accepted_total": plan_result.as_ref().and_then(|plan| plan.accepted_total.clone()),
            "remaining": plan_result.as_ref().and_then(|plan| plan.remaining.clone()),
        },
        "plan": plan_result,
        "slots": slot_views,
        "rates": rates,
        "notes": [
            "Цены лотов — снимок выставленных объявлений, а не цены состоявшихся сделок.",
            if request.auto {
                "Лоты запрошены по явной команде; на слот выбран не больше одного лота.".to_string()
            } else {
                NO_LOTS_MESSAGE.to_string()
            },
            rates.note(),
        ],
    }))
}

pub fn trade_plan(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let request: PlanRequest = serde_json::from_value(params)
        .map_err(|error| ApiError::new("bad_plan", format!("План не разобран: {error}")))?;
    if request.slots.is_empty() {
        return Err(ApiError::new("no_slots", "Не указан ни один слот."));
    }
    let saved = ctx.saved();
    let slots = request.slots.clone();
    plan_impl(ctx, &saved, request, slots)
}

fn keys_from_texts(texts: &[String]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut push = |key: &str| {
        if !keys.iter().any(|existing| existing == key) {
            keys.push(key.to_string());
        }
    };
    for text in texts {
        let lower = text.to_lowercase();
        if lower.contains("жизн") || lower.contains("life") {
            push("life");
        }
        if lower.contains("холод") || lower.contains("cold") {
            push("cold_res");
        }
        if lower.contains("огн") || lower.contains("fire") {
            push("fire_res");
        }
        if lower.contains("молни") || lower.contains("lightning") {
            push("lightning_res");
        }
        if lower.contains("хаос") || lower.contains("chaos") {
            push("chaos_res");
        }
        if lower.contains("передвиж") || lower.contains("movement") {
            push("movement_speed");
        } else if lower.contains("скорост") && lower.contains("атак") {
            push("attack_speed");
        }
        if lower.contains("мана") || lower.contains("mana") {
            push("mana");
        }
    }
    keys
}

/// Slots and filters taken from the active build: the imported guide first,
/// then the build priorities. Used by "что лучше заменить первым".
fn derived_upgrade_slots(ctx: &Context, saved: &Saved) -> Vec<PlanSlotRequest> {
    let build_id = active_build_id(saved);
    let Some(build) = ctx.build(&build_id) else {
        return Vec::new();
    };
    if let Some(guide) = ctx.guide_for(saved, &build_id) {
        let mut slots: Vec<PlanSlotRequest> = Vec::new();
        for item in &guide.gear {
            if guide
                .owned
                .iter()
                .any(|key| key == &format!("gear:{}", item.slot))
            {
                continue;
            }
            let mut texts: Vec<String> = item.mods.clone();
            texts.extend(item.must_have.clone());
            let filters = keys_from_texts(&texts)
                .into_iter()
                .map(|stat| FilterRequest { stat, min: None })
                .collect();
            slots.push(PlanSlotRequest {
                slot: item.slot.clone(),
                category: category_for_slot(&item.slot).into(),
                filters,
            });
        }
        if !slots.is_empty() {
            return slots;
        }
    }

    let level = saved.profile().map(|profile| profile.level).unwrap_or(1);
    let stage = stage_for(&build, level);
    let mut stage_texts: Vec<String> = Vec::new();
    if let Some(stage) = stage {
        stage_texts.extend(stage.gear.iter().map(pick));
        stage_texts.extend(stage.checklist.iter().map(|check| pick(&check.label)));
    }
    let mut armour_texts: Vec<String> = build.priorities.armour.iter().map(pick).collect();
    armour_texts.extend(stage_texts.clone());
    let accessory_texts: Vec<String> = build.priorities.accessories.iter().map(pick).collect();
    let weapon_texts: Vec<String> = build.priorities.weapon.iter().map(pick).collect();

    let armour_keys = keys_from_texts(&armour_texts);
    let accessory_keys = keys_from_texts(&accessory_texts);
    let weapon_keys = keys_from_texts(&weapon_texts);

    let to_filters = |keys: &[String]| -> Vec<FilterRequest> {
        keys.iter()
            .map(|stat| FilterRequest {
                stat: stat.clone(),
                min: None,
            })
            .collect()
    };

    let mut slots: Vec<PlanSlotRequest> = Vec::new();
    for slot in ["body", "helmet", "gloves", "boots"] {
        let mut keys = armour_keys.clone();
        if keys.is_empty() {
            keys.push("life".into());
        }
        if slot == "boots" && !keys.iter().any(|key| key == "movement_speed") {
            keys.push("movement_speed".into());
        }
        slots.push(PlanSlotRequest {
            slot: slot.into(),
            category: category_for_slot(slot).into(),
            filters: to_filters(&keys),
        });
    }
    for slot in ["amulet", "belt", "ring"] {
        let mut keys = accessory_keys.clone();
        if keys.is_empty() {
            keys.push("life".into());
        }
        slots.push(PlanSlotRequest {
            slot: slot.into(),
            category: category_for_slot(slot).into(),
            filters: to_filters(&keys),
        });
    }
    if stage_texts
        .iter()
        .any(|text| text.to_lowercase().contains("лук") || text.to_lowercase().contains("bow"))
    {
        slots.push(PlanSlotRequest {
            slot: "mainHand".into(),
            category: "weapon".into(),
            filters: to_filters(&weapon_keys),
        });
    }
    slots
}

/// Filters the build/guide suggests for the requested slots. Used when the
/// player names a slot but no mods ("найди ботинки и пояс").
pub fn slots_suggest(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let requested: Vec<String> = params
        .get("slots")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let saved = ctx.saved();
    let derived = derived_upgrade_slots(ctx, &saved);
    let mut out = Vec::new();
    for slot in &requested {
        if let Some(found) = derived.iter().find(|entry| &entry.slot == slot) {
            let keys: Vec<String> = found
                .filters
                .iter()
                .map(|filter| filter.stat.clone())
                .collect();
            out.push(json!({
                "slot": slot,
                "filters": keys,
                "source": "build",
                "note": "Взято из активного гайда или приоритетов билда.",
            }));
        } else {
            out.push(json!({
                "slot": slot,
                "filters": [],
                "source": "none",
                "note": "В билде нет подсказки для этого слота: спроси моды у игрока.",
            }));
        }
    }
    Ok(json!({ "ok": true, "slots": out }))
}

pub fn upgrade_plan(ctx: &Context, params: Value) -> Result<Value, ApiError> {
    let mut request: PlanRequest = serde_json::from_value(params)
        .map_err(|error| ApiError::new("bad_plan", format!("План не разобран: {error}")))?;
    let saved = ctx.saved();
    if request.slots.is_empty() {
        request.slots = derived_upgrade_slots(ctx, &saved);
    }
    if request.slots.is_empty() {
        return Err(ApiError::new(
            "no_slots",
            "Не удалось вывести слоты из билда: импортируй гайд или назови слоты.",
        ));
    }
    let slots = request.slots.clone();
    let mut result = plan_impl(ctx, &saved, request, slots)?;
    result["basis"] = json!("build_priority");
    result["equipped_known"] = json!(false);
    result["basis_note"] = json!(
        "Надетые предметы не переданы: это приоритет по билду, а не установленный факт, что вещь хуже."
    );
    Ok(result)
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
        "trade.plan" => trade_plan(ctx, params),
        "upgrade.plan" => upgrade_plan(ctx, params),
        "slots.suggest" => slots_suggest(ctx, params),
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
        assert!(result["reason"].as_str().unwrap().contains("не передан"));
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
    fn plan_returns_one_link_per_slot_without_lots() {
        let ctx = temp_ctx("plan-links");
        let result = trade_plan(
            &ctx,
            json!({
                "league": "Standard",
                "auto": false,
                "budget": { "global": { "amount": 20, "currency": "exalted" } },
                "slots": [
                    { "slot": "boots", "filters": [
                        { "stat": "movement_speed" },
                        { "stat": "cold_res", "min": 30 }
                    ]},
                    { "slot": "belt", "filters": [ { "stat": "life" } ] }
                ]
            }),
        )
        .unwrap();

        assert_eq!(result["lots_received"], false);
        assert_eq!(result["totals"]["fits"], Value::Null);
        assert_eq!(result["totals"]["message"], NO_LOTS_MESSAGE);
        let slots = result["slots"].as_array().unwrap();
        assert_eq!(slots.len(), 2);
        assert!(slots.iter().all(|slot| slot["search_url"]
            .as_str()
            .unwrap()
            .starts_with("https://www.pathofexile.com/trade2/search/poe2/Standard?q=")));
        assert_eq!(
            slots[0]["filters_applied"][1]["id"],
            "explicit.stat_4220027924"
        );
        assert_eq!(slots[0]["filters_applied"][1]["min"], 30.0);
        assert_eq!(
            slots[1]["filters_applied"][0]["id"],
            "explicit.stat_3299347043"
        );
        assert!(slots
            .iter()
            .all(|slot| slot["filters_unmatched"].as_array().unwrap().is_empty()));
    }

    #[test]
    fn plan_explicit_quotes_respect_the_global_limit() {
        let ctx = temp_ctx("plan-quotes");
        let result = trade_plan(
            &ctx,
            json!({
                "league": "Standard",
                "auto": false,
                "budget": { "global": { "amount": 20, "currency": "exalted" } },
                "rates": { "league": "Standard", "exalted_per_divine": 10.0, "source": "test" },
                "slots": [
                    { "slot": "boots", "filters": [ { "stat": "movement_speed" } ] },
                    { "slot": "belt", "filters": [ { "stat": "life" } ] }
                ],
                "quotes": [
                    { "slot": "boots", "name": "Boots A", "price": { "amount": 15, "currency": "exalted" }, "league": "Standard", "url": "u1" },
                    { "slot": "belt", "name": "Belt B", "price": { "amount": 15, "currency": "exalted" }, "league": "Standard", "url": "u2" }
                ]
            }),
        )
        .unwrap();

        assert_eq!(result["lots_received"], true);
        assert_eq!(result["plan"]["fits"], false);
        assert!(result["plan"]["accepted"].as_array().unwrap().len() < 2);
        assert_eq!(result["plan"]["set_total"]["amount"], 30.0);
        assert!(result["plan"]["violations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("не укладывается")));
    }

    #[test]
    fn plan_rejects_duplicate_slots() {
        let ctx = temp_ctx("plan-duplicate");
        let error = trade_plan(
            &ctx,
            json!({
                "league": "Standard",
                "slots": [
                    { "slot": "boots", "filters": [] },
                    { "slot": "boots", "filters": [] }
                ]
            }),
        )
        .unwrap_err();
        assert_eq!(error.code, "duplicate_slot");
    }

    #[test]
    fn explicit_mods_are_merged_into_a_guide_query_and_verified() {
        let ctx = temp_ctx("plan-guide");
        let mut saved = ctx.saved();
        saved.guides.push(GuideRecord {
            id: "g1".into(),
            build_id: "lightning-arrow-deadeye".into(),
            gear: vec![crate::guide::GuideGear {
                slot: "boots".into(),
                trade_query: json!({
                    "query": {
                        "status": { "option": "online" },
                        "stats": [{ "type": "and", "filters": [
                            { "id": "explicit.stat_3299347043", "value": { "min": 60 }, "disabled": false }
                        ]}]
                    }
                })
                .to_string(),
                ..Default::default()
            }],
            ..Default::default()
        });
        model::save_state(&ctx.state_path(), &saved).unwrap();

        let request = PlanSlotRequest {
            slot: "boots".into(),
            category: "armour".into(),
            filters: vec![
                FilterRequest {
                    stat: "cold_res".into(),
                    min: Some(35.0),
                },
                FilterRequest {
                    stat: "life".into(),
                    min: None,
                },
            ],
        };
        let plan = build_slot_plan(
            &ctx,
            &ctx.saved(),
            &request,
            "Standard",
            &budget::Budget::default(),
            &budget::Rates::default(),
            "exalted",
        );
        assert!(plan.unmatched.is_empty(), "{:?}", plan.unmatched);
        let ids: Vec<&str> = plan
            .applied
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        assert!(ids.contains(&"explicit.stat_4220027924"));
        assert!(ids.contains(&"explicit.stat_3299347043"));
        let body = plan.body["query"].to_string();
        assert!(body.contains("explicit.stat_4220027924"));
        assert!(body.contains("explicit.stat_3299347043"));
        // The explicit life value replaced the guide's 60 instead of duplicating it.
        assert_eq!(body.matches("explicit.stat_3299347043").count(), 1);
        assert!(body.contains("\"min\":50.0"));
    }

    #[test]
    fn upgrade_plan_uses_build_priority_and_no_equipped_facts() {
        let ctx = temp_ctx("upgrade-plan");
        let result = upgrade_plan(
            &ctx,
            json!({
                "league": "Standard",
                "auto": false,
                "budget": { "global": { "amount": 20, "currency": "exalted" } }
            }),
        )
        .unwrap();
        assert_eq!(result["basis"], "build_priority");
        assert_eq!(result["equipped_known"], false);
        assert!(result["slots"].as_array().unwrap().len() >= 4);
        assert_eq!(result["lots_received"], false);
        assert_eq!(result["totals"]["message"], NO_LOTS_MESSAGE);
        assert!(result["basis_note"].as_str().unwrap().contains("приоритет"));
    }

    #[test]
    fn imported_items_are_not_treated_as_equipped() {
        let ctx = temp_ctx("not-equipped");
        let mut saved = ctx.saved();
        let parsed = crate::item::parse(sample_item()).unwrap();
        saved.items.push(crate::model::ItemRecord {
            id: "item-1".into(),
            ts: model::now_iso(),
            profile: saved.active.clone(),
            raw: sample_item().into(),
            analysis: crate::item::analyze(&parsed, 24),
            parsed,
            dangers: Vec::new(),
            ai: None,
            price_history: Vec::new(),
        });
        model::save_state(&ctx.state_path(), &saved).unwrap();

        let result = evaluate_item(&ctx, json!({ "text": sample_item() })).unwrap();
        assert_eq!(result["equipped_known"], false);
        assert_eq!(result["equipped_source"], "none");
        assert_eq!(result["verdict"], ITEM_VERDICT_FITS);
    }

    #[test]
    fn explicit_equipped_item_is_compared() {
        let ctx = temp_ctx("explicit-equipped");
        let equipped = "Rarity: Normal\nSimple Ring\n--------\n+5 to maximum Life\n";
        let result = evaluate_item(
            &ctx,
            json!({ "text": sample_item(), "equipped_text": equipped }),
        )
        .unwrap();
        assert_eq!(result["equipped_known"], true);
        assert_eq!(result["equipped_source"], "explicit");
    }

    #[test]
    fn explicit_equipped_item_of_another_slot_is_ignored() {
        let ctx = temp_ctx("equipped-slot");
        let equipped =
            "Item Class: Boots\nRarity: Normal\nRawhide Boots\n--------\n+5 to maximum Life\n";
        let result = evaluate_item(
            &ctx,
            json!({ "text": sample_item(), "equipped_text": equipped }),
        )
        .unwrap();
        assert_eq!(result["equipped_known"], false);
        assert_eq!(result["equipped_source"], "none");
    }

    #[test]
    fn old_zone_stays_last_known_after_a_settings_change() {
        use chrono::{Duration, Utc};
        let ctx = temp_ctx("freshness");
        let mut saved = ctx.saved();
        let old_ts = (Utc::now() - Duration::hours(8)).to_rfc3339();
        saved.history.push(crate::model::HistoryEvent {
            id: "h1".into(),
            ts: old_ts,
            profile: saved.active.clone(),
            kind: "zone".into(),
            level: 24,
            zone: "The Ardura Caravan".into(),
            act: 2,
            title: "Караван".into(),
            detail: String::new(),
        });
        if let Some(profile) = saved.profile_mut() {
            profile.updated = model::now_iso();
        }
        model::save_state(&ctx.state_path(), &saved).unwrap();

        let state = state_summary(&ctx);
        assert_eq!(state["zone_basis"], "last_known");
        assert_eq!(state["stale"], true);
        assert_eq!(state["game"]["last_event_kind"], "zone");
        assert!(state["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("последняя известная зона")));
    }

    #[test]
    fn per_item_budget_caps_each_slot_query() {
        let ctx = temp_ctx("per-item");
        let budget = budget::Budget {
            global: None,
            per_item: Some(budget::Money::new(10.0, "exalted")),
        };
        let rates = budget::Rates::default();
        for slot in ["boots", "belt"] {
            let request = PlanSlotRequest {
                slot: slot.into(),
                category: category_for_slot(slot).into(),
                filters: vec![FilterRequest {
                    stat: "life".into(),
                    min: None,
                }],
            };
            let plan = build_slot_plan(
                &ctx,
                &ctx.saved(),
                &request,
                "Standard",
                &budget,
                &rates,
                "exalted",
            );
            let price = &plan.body["query"]["filters"]["trade_filters"]["filters"]["price"];
            assert_eq!(price["max"], 10.0);
            assert_eq!(price["option"], "exalted");
        }
    }

    #[test]
    fn mixed_currency_without_a_rate_does_not_confirm_the_budget() {
        let ctx = temp_ctx("mixed-rate");
        let result = trade_plan(
            &ctx,
            json!({
                "league": "Standard",
                "auto": false,
                "budget": { "global": { "amount": 100, "currency": "exalted" } },
                "rates": { "league": "Standard" },
                "slots": [
                    { "slot": "boots", "filters": [] },
                    { "slot": "belt", "filters": [] }
                ],
                "quotes": [
                    { "slot": "boots", "name": "B", "price": { "amount": 10, "currency": "exalted" }, "league": "Standard", "url": "u1" },
                    { "slot": "belt", "name": "L", "price": { "amount": 1, "currency": "divine" }, "league": "Standard", "url": "u2" }
                ]
            }),
        )
        .unwrap();
        assert_eq!(result["lots_received"], true);
        assert_eq!(result["plan"]["budget_known"], false);
        assert_eq!(result["plan"]["fits"], false);
        assert!(result["plan"]["violations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("смешанных валют")));
    }

    #[test]
    fn a_quote_from_another_league_is_rejected() {
        let ctx = temp_ctx("league-mismatch");
        let result = trade_plan(
            &ctx,
            json!({
                "league": "Standard",
                "auto": false,
                "budget": { "global": { "amount": 100, "currency": "exalted" } },
                "rates": { "league": "Standard", "exalted_per_divine": 10.0, "source": "test" },
                "slots": [
                    { "slot": "boots", "filters": [] },
                    { "slot": "belt", "filters": [] }
                ],
                "quotes": [
                    { "slot": "boots", "name": "B", "price": { "amount": 10, "currency": "exalted" }, "league": "Hardcore", "url": "u1" },
                    { "slot": "belt", "name": "L", "price": { "amount": 10, "currency": "exalted" }, "league": "Standard", "url": "u2" }
                ]
            }),
        )
        .unwrap();
        assert_eq!(result["plan"]["fits"], false);
        let rejected = result["plan"]["rejected"].as_array().unwrap();
        assert!(rejected.iter().any(|item| item["reason"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("hardcore")));
    }

    #[test]
    fn slots_suggest_gives_build_filters_and_marks_unknown_slots() {
        let ctx = temp_ctx("suggest");
        let result = slots_suggest(&ctx, json!({ "slots": ["boots", "belt", "flask"] })).unwrap();
        let slots = result["slots"].as_array().unwrap();
        let boots = &slots[0];
        assert_eq!(boots["slot"], "boots");
        let keys: Vec<&str> = boots["filters"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(keys.contains(&"movement_speed"));
        assert_eq!(slots[2]["source"], "none");
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
