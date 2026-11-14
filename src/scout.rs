//! Prices and the current league from poe2scout (keyless, 24h averages).
//!
//! The reference overlay by kwongjohn uses the same source; the working endpoint
//! is `https://api.poe2scout.com/{realm}/Leagues` (the older `/api/poe2` paths now
//! return the site's HTML). Each league row carries `IsCurrent` and `DivinePrice`
//! in exalted, which is exactly the rate the trade tab needs.

use serde::{Deserialize, Serialize};
use std::time::Duration;

const BASE: &str = "https://api.poe2scout.com/poe2";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Scout {
    pub league: String,
    /// Exalted per divine, 24h average.
    pub divine_price: f64,
    /// Chaos per divine, 24h average (when the API provides it).
    #[serde(default)]
    pub chaos_per_divine: f64,
    /// Base currency of the league (usually Exalted Orb).
    pub base_currency: String,
    pub fetched: String,
    pub source: String,
}

#[derive(Deserialize)]
struct League {
    #[serde(rename = "Value", default)]
    value: String,
    #[serde(rename = "IsCurrent", default)]
    is_current: bool,
    #[serde(rename = "DivinePrice", default)]
    divine_price: f64,
    #[serde(rename = "ChaosDivinePrice", default)]
    chaos_divine_price: f64,
    #[serde(rename = "BaseCurrencyText", default)]
    base_currency: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
}

fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Picks the current softcore league and its divine price.
pub fn parse(json: &str) -> Result<Scout, String> {
    let leagues: Vec<League> = serde_json::from_str(json).map_err(|e| format!("poe2scout: ответ не разобран: {e}"))?;
    let current = leagues
        .iter()
        .find(|l| l.is_current && !l.value.to_ascii_uppercase().starts_with("HC "))
        .or_else(|| leagues.iter().find(|l| l.is_current))
        .or_else(|| leagues.first())
        .ok_or("poe2scout: список лиг пуст")?;
    if current.divine_price <= 0.0 {
        return Err("poe2scout: не отдал цену Divine".into());
    }
    Ok(Scout {
        league: current.value.clone(),
        divine_price: current.divine_price,
        chaos_per_divine: current.chaos_divine_price,
        base_currency: current.base_currency.clone(),
        fetched: now_iso(),
        source: "poe2scout 24h".into(),
    })
}

pub fn fetch() -> Result<Scout, String> {
    let mut res = agent()
        .get(&format!("{BASE}/Leagues"))
        .header("User-Agent", "Eni-PoE2-Companion (github.com/BigbyOn/Eni-PoE2-Companion)")
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("poe2scout: сеть: {e}"))?;
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().map_err(|e| format!("poe2scout: чтение: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("poe2scout HTTP {status}"));
    }
    parse(&text)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct ScoutItem {
    /// poe2scout item id (used by the price-history endpoint).
    #[serde(default)]
    pub item_id: i64,
    /// Trade tag used by the official trade API (`regal-shard`, `divine`, …).
    pub api_id: String,
    pub text: String,
    pub category: String,
    /// Price in the league base currency (usually exalted).
    pub current_price: f64,
    pub icon: String,
}

#[derive(Deserialize)]
struct ItemRow {
    #[serde(rename = "ItemId", default)]
    item_id: i64,
    #[serde(rename = "ApiId", default)]
    api_id: Option<String>,
    #[serde(rename = "Text", default)]
    text: Option<String>,
    #[serde(rename = "CategoryApiId", default)]
    category: Option<String>,
    #[serde(rename = "CurrentPrice", default)]
    current_price: Option<f64>,
    #[serde(rename = "IconUrl", default)]
    icon: Option<String>,
}

/// Parses the league item list; only items with a trade tag and a positive
/// price are useful for a price card.
pub fn parse_items(json: &str) -> Result<Vec<ScoutItem>, String> {
    let rows: Vec<ItemRow> = serde_json::from_str(json).map_err(|e| format!("poe2scout items: ответ не разобран: {e}"))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let api_id = row.api_id?;
            let price = row.current_price?;
            if api_id.trim().is_empty() || price <= 0.0 {
                return None;
            }
            Some(ScoutItem {
                item_id: row.item_id,
                api_id,
                text: row.text.unwrap_or_default(),
                category: row.category.unwrap_or_default(),
                current_price: price,
                icon: row.icon.unwrap_or_default(),
            })
        })
        .collect())
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct PriceChange {
    pub item_id: i64,
    /// Latest price in the league base currency.
    pub price: f64,
    /// Price about 24 hours ago (or the oldest point).
    pub old_price: f64,
    pub change: f64,
    pub change_pct: f64,
}

#[derive(Deserialize)]
struct HistoryResponse {
    #[serde(rename = "ItemHistories", default)]
    histories: Vec<ItemHistory>,
}

#[derive(Deserialize)]
struct ItemHistory {
    #[serde(rename = "ItemId", default)]
    item_id: i64,
    #[serde(rename = "History", default)]
    history: Vec<HistoryPoint>,
}

#[derive(Deserialize)]
struct HistoryPoint {
    #[serde(rename = "Price", default)]
    price: f64,
    #[serde(rename = "Time", default)]
    time: String,
}

fn parse_time(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value).ok().map(|t| t.with_timezone(&chrono::Utc))
}

/// 24h change for every item that has at least two history points.
pub fn parse_price_history(json: &str) -> Result<Vec<PriceChange>, String> {
    let parsed: HistoryResponse = serde_json::from_str(json).map_err(|e| format!("poe2scout history: ответ не разобран: {e}"))?;
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
    let mut out = Vec::new();
    for entry in parsed.histories {
        let points: Vec<&HistoryPoint> = entry.history.iter().filter(|p| p.price > 0.0).collect();
        if points.len() < 2 {
            continue;
        }
        let current = points[0].price;
        let old = points
            .iter()
            .find(|p| parse_time(&p.time).map(|t| t <= cutoff).unwrap_or(false))
            .map(|p| p.price)
            .unwrap_or_else(|| points[points.len() - 1].price);
        if old <= 0.0 {
            continue;
        }
        out.push(PriceChange {
            item_id: entry.item_id,
            price: current,
            old_price: old,
            change: current - old,
            change_pct: (current - old) / old * 100.0,
        });
    }
    Ok(out)
}

pub fn fetch_price_history(league: &str) -> Result<Vec<PriceChange>, String> {
    let league = league.replace(' ', "%20");
    let mut res = agent()
        .get(&format!("{BASE}/Leagues/{league}/Items/PriceHistory"))
        .header("User-Agent", "Eni-PoE2-Companion (github.com/BigbyOn/Eni-PoE2-Companion)")
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("poe2scout history: сеть: {e}"))?;
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().map_err(|e| format!("poe2scout history: чтение: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("poe2scout history HTTP {status}"));
    }
    parse_price_history(&text)
}

pub fn fetch_items(league: &str) -> Result<Vec<ScoutItem>, String> {
    let league = league.replace(' ', "%20");
    let mut res = agent()
        .get(&format!("{BASE}/Leagues/{league}/Items"))
        .header("User-Agent", "Eni-PoE2-Companion (github.com/BigbyOn/Eni-PoE2-Companion)")
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("poe2scout items: сеть: {e}"))?;
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().map_err(|e| format!("poe2scout items: чтение: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("poe2scout items HTTP {status}"));
    }
    parse_items(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_current_softcore_league_and_divine_price() {
        let json = r#"[
            {"Value":"HC Forbidden Rites","IsCurrent":true,"DivinePrice":700},
            {"Value":"Forbidden Rites","IsCurrent":true,"DivinePrice":655.05,"BaseCurrencyText":"Exalted Orb"},
            {"Value":"Standard","IsCurrent":false,"DivinePrice":499}
        ]"#;
        let s = parse(json).unwrap();
        assert_eq!(s.league, "Forbidden Rites");
        assert_eq!(s.divine_price, 655.05);
        assert_eq!(s.base_currency, "Exalted Orb");
    }

    #[test]
    fn rejects_empty_or_zero_price_data() {
        assert!(parse("[]").is_err());
        assert!(parse(r#"[{"Value":"Standard","IsCurrent":true,"DivinePrice":0}]"#).is_err());
        assert!(parse("not json").is_err());
    }
}
