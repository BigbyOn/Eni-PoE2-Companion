//! Price check through the official Path of Exile 2 trade API.
//!
//! Endpoints (documented by GGG and used by every trade tool):
//! * `GET  /api/trade2/data/leagues` — league list
//! * `POST /api/trade2/search/poe2/{league}` — search, returns result hashes
//! * `GET  /api/trade2/fetch/{hashes}?query={id}` — listings with prices
//!
//! Rules kept here:
//! * only `https://www.pathofexile.com`, redirects disabled;
//! * one search + one fetch per explicit user click, minimum interval between
//!   clicks, `Retry-After` honoured on 429;
//! * a descriptive User-Agent, as GGG asks API clients to send;
//! * no stat-id mapping yet: uniques and bases give a useful range, rares only
//!   a base-type orientation (the UI says so and links to the trade site).

use crate::item::ParsedItem;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const BASE: &str = "https://www.pathofexile.com";
/// GGG asks tools to identify themselves; keep the repository in the string.
const CONTACT: &str = "github.com/BigbyOn/Eni-PoE2-Companion";

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct PriceResult {
    pub league: String,
    pub total: u64,
    pub currency: String,
    pub min: f64,
    pub median: f64,
    pub samples: Vec<String>,
    pub trade_url: String,
    /// Human-readable caveat for the UI (e.g. rare items without stat filters).
    pub note: String,
}

#[derive(Deserialize)]
struct Leagues {
    #[serde(default)]
    result: Vec<League>,
}

#[derive(Deserialize)]
struct League {
    id: String,
    #[serde(default)]
    realm: String,
}

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    total: u64,
    #[serde(default)]
    result: Vec<String>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct Fetch {
    #[serde(default)]
    result: Vec<Fetched>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Fetched {
    #[serde(default)]
    listing: Listing,
}

#[derive(Default, Deserialize)]
struct Listing {
    #[serde(default)]
    price: Option<Price>,
}

#[derive(Deserialize)]
struct Price {
    #[serde(default)]
    amount: f64,
    #[serde(default)]
    currency: String,
}

fn is_unique(item: &ParsedItem) -> bool {
    let r = item.rarity.to_lowercase();
    r.contains("unique") || r.contains("уник")
}

fn rarity_option(item: &ParsedItem) -> Option<&'static str> {
    let r = item.rarity.to_lowercase();
    if is_unique(item) {
        Some("unique")
    } else if r.contains("rare") || r.contains("редк") {
        Some("rare")
    } else if r.contains("magic") || r.contains("волшеб") {
        Some("magic")
    } else if r.contains("normal") || r.contains("обычн") {
        Some("normal")
    } else {
        None
    }
}

/// Trade query for an item. Pure, so it is unit tested.
pub fn build_query(item: &ParsedItem) -> serde_json::Value {
    let mut query = serde_json::json!({ "status": { "option": "online" } });
    if is_unique(item) && !item.name.trim().is_empty() {
        query["name"] = serde_json::json!(item.name.trim());
    }
    let base = if item.base.trim().is_empty() { item.name.trim() } else { item.base.trim() };
    if !base.is_empty() {
        query["type"] = serde_json::json!(base);
    }
    let mut filters = serde_json::Map::new();
    if let Some(rarity) = rarity_option(item) {
        filters.insert("rarity".into(), serde_json::json!({ "option": rarity }));
    }
    if !filters.is_empty() {
        query["filters"] = serde_json::json!({ "type_filters": { "filters": filters } });
    }
    query
}

/// Percent-encoding for the `?q=` parameter of the trade site.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn build_trade_url(league: &str, query: &serde_json::Value) -> String {
    let body = serde_json::json!({ "query": query, "sort": { "price": "asc" } });
    format!("{BASE}/trade2/search/poe2/{}?q={}", encode(league), encode(&body.to_string()))
}

/// Median of the listings in the most common currency.
fn summarize_listings(listings: &[(f64, String)]) -> (String, f64, f64, Vec<String>) {
    if listings.is_empty() {
        return (String::new(), 0.0, 0.0, Vec::new());
    }
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (_, cur) in listings {
        *counts.entry(cur.as_str()).or_default() += 1;
    }
    let currency = counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c.to_string()).unwrap_or_default();
    let mut amounts: Vec<f64> = listings.iter().filter(|(_, c)| *c == currency).map(|(a, _)| *a).collect();
    if amounts.is_empty() {
        amounts = listings.iter().map(|(a, _)| *a).collect();
    }
    amounts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min = amounts.first().copied().unwrap_or(0.0);
    let median = amounts[amounts.len() / 2];
    let samples = amounts.iter().take(5).map(|a| format!("{a} {currency}")).collect();
    (currency, min, median, samples)
}

pub fn parse_search(json: &str) -> Result<(String, u64, Vec<String>), String> {
    let s: Search = serde_json::from_str(json).map_err(|e| format!("Ответ поиска не разобран: {e}"))?;
    if let Some(err) = s.error {
        return Err(format!("Trade API: {}", err.message));
    }
    let id = s.id.ok_or("Trade API не вернул id поиска")?;
    Ok((id, s.total, s.result))
}

pub fn parse_fetch(json: &str) -> Result<Vec<(f64, String)>, String> {
    let f: Fetch = serde_json::from_str(json).map_err(|e| format!("Ответ списка не разобран: {e}"))?;
    if let Some(err) = f.error {
        return Err(format!("Trade API: {}", err.message));
    }
    Ok(f.result
        .into_iter()
        .filter_map(|x| x.listing.price)
        .map(|p| (p.amount, p.currency))
        .collect())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
}

fn ua() -> String {
    format!("Eni-PoE2-Companion/{} ({CONTACT})", env!("CARGO_PKG_VERSION"))
}

fn get_json(url: &str) -> Result<String, String> {
    let mut res = agent()
        .get(url)
        .header("User-Agent", &ua())
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("Сеть: {e}"))?;
    let status = res.status().as_u16();
    let retry = res.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let body = res.body_mut().read_to_string().map_err(|e| format!("Чтение ответа: {e}"))?;
    if status == 429 {
        return Err(format!("Слишком часто: trade API просит подождать {} с.", retry.unwrap_or_else(|| "60".into())));
    }
    if !(200..300).contains(&status) {
        return Err(format!("Trade API HTTP {status}: {}", body.chars().take(200).collect::<String>()));
    }
    Ok(body)
}

/// Picks the current PoE2 challenge league, falling back to Standard.
pub fn resolve_league(preferred: &str) -> Result<String, String> {
    let text = get_json(&format!("{BASE}/api/trade2/data/leagues"))?;
    let leagues: Leagues = serde_json::from_str(&text).map_err(|e| format!("Список лиг не разобран: {e}"))?;
    let poe2: Vec<&League> = leagues.result.iter().filter(|l| l.realm == "poe2" || l.realm.is_empty()).collect();
    if !preferred.trim().is_empty() && poe2.iter().any(|l| l.id == preferred.trim()) {
        return Ok(preferred.trim().to_string());
    }
    if let Some(l) = poe2.iter().find(|l| !l.id.starts_with("HC") && l.id != "Standard" && l.id != "Hardcore") {
        return Ok(l.id.clone());
    }
    Ok(poe2.first().map(|l| l.id.clone()).unwrap_or_else(|| "Standard".to_string()))
}

/// Full price check: league, search, first page of listings, summary.
pub fn check(item: &ParsedItem, league_pref: &str) -> Result<PriceResult, String> {
    let league = resolve_league(league_pref)?;
    let query = build_query(item);
    let body = serde_json::json!({ "query": query, "sort": { "price": "asc" } });
    let url = format!("{BASE}/api/trade2/search/poe2/{}", encode(&league));
    let mut res = agent()
        .post(&url)
        .header("User-Agent", &ua())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|e| format!("Сеть: {e}"))?;
    let status = res.status().as_u16();
    let retry = res.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let text = res.body_mut().read_to_string().map_err(|e| format!("Чтение ответа: {e}"))?;
    if status == 429 {
        return Err(format!("Слишком часто: trade API просит подождать {} с.", retry.unwrap_or_else(|| "60".into())));
    }
    if !(200..300).contains(&status) {
        return Err(format!("Trade API HTTP {status}: {}", text.chars().take(200).collect::<String>()));
    }
    let (id, total, hashes) = parse_search(&text)?;
    let mut result = PriceResult {
        league: league.clone(),
        total,
        trade_url: build_trade_url(&league, &query),
        note: String::new(),
        ..Default::default()
    };
    if hashes.is_empty() {
        result.note = "Подходящих предложений нет (или предмет не торгуется).".into();
        return Ok(result);
    }
    let hashes: Vec<&str> = hashes.iter().take(10).map(|s| s.as_str()).collect();
    let fetch_url = format!("{BASE}/api/trade2/fetch/{}?query={}", hashes.join(","), encode(&id));
    let fetch_text = get_json(&fetch_url)?;
    let listings = parse_fetch(&fetch_text)?;
    let (currency, min, median, samples) = summarize_listings(&listings);
    if listings.is_empty() {
        result.note = "Лоты есть, но без указанной цены.".into();
    }
    result.currency = currency;
    result.min = min;
    result.median = median;
    result.samples = samples;
    if !is_unique(item) {
        result.note = "Ориентир по типу предмета и редкости, без учёта модификаторов — точную цену смотри на trade.".into();
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring() -> ParsedItem {
        ParsedItem {
            class: "Rings".into(),
            rarity: "Unique".into(),
            name: "Doom Loop".into(),
            base: "Prismatic Ring".into(),
            ..Default::default()
        }
    }

    #[test]
    fn unique_query_uses_name_and_type() {
        let q = build_query(&ring());
        assert_eq!(q["name"], "Doom Loop");
        assert_eq!(q["type"], "Prismatic Ring");
        assert_eq!(q["filters"]["type_filters"]["filters"]["rarity"]["option"], "unique");
        assert_eq!(q["status"]["option"], "online");
    }

    #[test]
    fn rare_query_has_no_name_but_a_rarity_filter() {
        let item = ParsedItem { rarity: "Rare".into(), name: "Doom Loop".into(), base: "Prismatic Ring".into(), ..Default::default() };
        let q = build_query(&item);
        assert!(q.get("name").is_none(), "rare names are not searchable");
        assert_eq!(q["type"], "Prismatic Ring");
        assert_eq!(q["filters"]["type_filters"]["filters"]["rarity"]["option"], "rare");
    }

    #[test]
    fn russian_rarity_is_recognised() {
        let item = ParsedItem { rarity: "Уникальный".into(), name: "Кольцо".into(), base: "Призматическое кольцо".into(), ..Default::default() };
        let q = build_query(&item);
        assert_eq!(q["filters"]["type_filters"]["filters"]["rarity"]["option"], "unique");
    }

    #[test]
    fn trade_url_is_encoded_and_points_at_poe2() {
        let q = build_query(&ring());
        let url = build_trade_url("Standard", &q);
        assert!(url.starts_with("https://www.pathofexile.com/trade2/search/poe2/Standard?q="));
        assert!(!url.contains('{'));
        assert!(!url.contains('"'));
        assert!(url.contains("%22query%22"));
    }

    #[test]
    fn search_response_is_parsed() {
        let json = r#"{"id":"abc123","total":42,"result":["h1","h2"]}"#;
        assert_eq!(parse_search(json).unwrap(), ("abc123".into(), 42, vec!["h1".into(), "h2".into()]));
        let err = r#"{"error":{"message":"Invalid query"}}"#;
        assert!(parse_search(err).unwrap_err().contains("Invalid query"));
    }

    #[test]
    fn fetch_response_is_parsed() {
        let json = r#"{"result":[
            {"listing":{"price":{"amount":12,"currency":"chaos"}}},
            {"listing":{"price":{"amount":20,"currency":"chaos"}}},
            {"listing":{}}
        ]}"#;
        let prices = parse_fetch(json).unwrap();
        assert_eq!(prices.len(), 2);
        let (currency, min, median, samples) = summarize_listings(&prices);
        assert_eq!((currency.as_str(), min, median), ("chaos", 12.0, 20.0));
        assert_eq!(samples.len(), 2);
    }

    #[test]
    fn summarise_picks_the_most_common_currency() {
        let prices = vec![(1.0, "divine".to_string()), (10.0, "chaos".to_string()), (12.0, "chaos".to_string())];
        let (currency, min, median, _) = summarize_listings(&prices);
        assert_eq!(currency, "chaos");
        assert_eq!((min, median), (10.0, 12.0));
    }

    #[test]
    fn empty_listings_are_safe() {
        let (currency, min, median, samples) = summarize_listings(&[]);
        assert!(currency.is_empty());
        assert_eq!((min, median), (0.0, 0.0));
        assert!(samples.is_empty());
    }
}
