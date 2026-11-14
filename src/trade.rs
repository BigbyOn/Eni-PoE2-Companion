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

/// One searchable stat from `/api/trade2/data/stats`, reduced to a shape:
/// numbers are replaced by `#` so an item line can be matched against it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StatDef {
    pub id: String,
    /// e.g. `# to maximum life`
    pub shape: String,
    /// explicit / implicit / crafted / fractured / rune / …
    pub kind: String,
}

/// A filter for one modifier of the item.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct StatFilter {
    pub id: String,
    pub min: f64,
    pub max: Option<f64>,
    pub text: String,
}

#[derive(Deserialize)]
struct StatsResponse {
    #[serde(default)]
    result: Vec<StatGroup>,
}

#[derive(Deserialize)]
struct StatGroup {
    #[serde(default)]
    id: String,
    #[serde(default)]
    entries: Vec<StatEntry>,
}

#[derive(Deserialize)]
struct StatEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    r#type: String,
}

/// Canonical shape of a modifier line: lowercase, numbers and signs -> `#`.
pub fn shape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_number = false;
    for ch in text.chars() {
        let numeric = ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+';
        if numeric {
            if !in_number {
                // Keep a leading sign out of the placeholder itself.
                if ch == '-' || ch == '+' {
                    continue;
                }
                out.push('#');
                in_number = true;
            }
        } else {
            in_number = false;
            for low in ch.to_lowercase() {
                out.push(low);
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn numbers(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || ch == '.' || ((ch == '-' || ch == '+') && current.is_empty()) {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(v) = current.parse::<f64>() {
                out.push(v);
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        if let Ok(v) = current.parse::<f64>() {
            out.push(v);
        }
    }
    out
}

/// Parses the stats dataset into shapes.
pub fn parse_stats(json: &str) -> Result<Vec<StatDef>, String> {
    let resp: StatsResponse = serde_json::from_str(json).map_err(|e| format!("Статы не разобраны: {e}"))?;
    let mut out = Vec::new();
    for group in resp.result {
        for entry in group.entries {
            let kind = if entry.r#type.is_empty() { group.id.clone() } else { entry.r#type };
            let shape = shape(&entry.text);
            if shape.is_empty() || entry.id.is_empty() {
                continue;
            }
            out.push(StatDef { id: entry.id, shape, kind });
        }
    }
    if out.is_empty() {
        return Err("Статы пусты".into());
    }
    Ok(out)
}

fn is_defensive(shape: &str) -> bool {
    shape.contains("maximum life")
        || shape.contains("resistance")
        || shape.contains("spirit")
        || shape.contains("maximum mana")
}

/// Picks up to `limit` item modifiers that exist in the stats dataset.
///
/// Defensive lines (life/resistances) come first, because they define the price
/// of a rare item much more than a small damage roll.
pub fn match_stats(item: &ParsedItem, stats: &[StatDef], limit: usize, headroom: u8) -> (Vec<StatFilter>, Vec<String>) {
    let factor = 1.0 - f64::from(headroom.min(50)) / 100.0;
    let mut found: Vec<(bool, usize, StatFilter)> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();
    for (index, line) in item.mods.iter().enumerate() {
        let values: Vec<f64> = numbers(line);
        let Some(first) = values.first().copied() else { continue };
        // Negative modifiers (uniques, curses) do not make a useful "min" filter.
        if first <= 0.0 {
            continue;
        }
        let target = shape(line);
        let Some(def) = stats.iter().find(|s| s.shape == target && s.kind == "explicit").or_else(|| stats.iter().find(|s| s.shape == target)) else {
            unmatched.push(line.clone());
            continue;
        };
        // 10% headroom: the player compares with *similar* items, not only with
        // rolls that are at least as good.
        let min = (first * factor).floor().max(1.0);
        let max = if def.shape.matches('#').count() >= 2 && values.len() >= 2 { Some(values[1]) } else { None };
        found.push((
            is_defensive(&def.shape),
            index,
            StatFilter { id: def.id.clone(), min, max, text: line.clone() },
        ));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    (found.into_iter().take(limit).map(|(_, _, f)| f).collect(), unmatched)
}


const BASE: &str = "https://www.pathofexile.com";

fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
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
    /// Modifiers that did not match the stat catalogue.
    #[serde(default)]
    pub unmatched: Vec<String>,
    /// How many modifiers were used as filters.
    #[serde(default)]
    pub matched: usize,
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
    #[serde(default)]
    item: FetchedItem,
}

#[derive(Default, Deserialize)]
struct Listing {
    #[serde(default)]
    price: Option<Price>,
    #[serde(default)]
    account: Account,
    #[serde(default)]
    whisper: String,
}

#[derive(Default, Deserialize)]
struct Account {
    #[serde(default)]
    name: String,
    #[serde(default)]
    online: Option<serde_json::Value>,
}

#[derive(Default, Deserialize)]
struct FetchedItem {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "typeLine")]
    type_line: String,
    #[serde(default)]
    rarity: String,
}

/// Divine price in exalted, as shown on the trade page.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct CurrencyRate {
    pub league: String,
    pub exalted_per_divine: f64,
    /// Chaos per divine, when the source knows it.
    #[serde(default)]
    pub chaos_per_divine: Option<f64>,
    pub offers: usize,
    /// "exchange" (bulk offers) or "listings" (priced Divine Orb listings).
    pub source: String,
    pub updated: String,
}

/// One listing of a text search, ready for the UI.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct ListingView {
    pub price: f64,
    pub currency: String,
    pub name: String,
    pub base: String,
    pub rarity: String,
    pub account: String,
    /// The seller is online right now.
    pub online: bool,
    pub whisper: String,
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
    build_query_with(item, &[])
}

/// Same query, plus `and`-combined stat filters for the item's modifiers.
pub fn build_query_with(item: &ParsedItem, stat_filters: &[StatFilter]) -> serde_json::Value {
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
    if !stat_filters.is_empty() {
        let list: Vec<serde_json::Value> = stat_filters
            .iter()
            .map(|f| {
                let mut value = serde_json::json!({ "min": f.min });
                if let Some(max) = f.max {
                    value["max"] = serde_json::json!(max);
                }
                serde_json::json!({ "id": f.id, "value": value, "disabled": false })
            })
            .collect();
        query["stats"] = serde_json::json!([{ "type": "and", "filters": list }]);
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

/// Parses listings with seller and whisper text (text search).
pub fn parse_listings(json: &str) -> Result<Vec<ListingView>, String> {
    let f: Fetch = serde_json::from_str(json).map_err(|e| format!("Ответ списка не разобран: {e}"))?;
    if let Some(err) = f.error {
        return Err(format!("Trade API: {}", err.message));
    }
    Ok(f.result
        .into_iter()
        .filter_map(|x| {
            let price = x.listing.price?;
            Some(ListingView {
                price: price.amount,
                currency: price.currency,
                name: x.item.name,
                base: x.item.type_line,
                rarity: x.item.rarity,
                online: x.listing.account.online.is_some(),
                account: x.listing.account.name,
                whisper: x.listing.whisper,
            })
        })
        .collect())
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

/// Free-text item search: `type`/name online listings sorted by price.
#[derive(Clone, Debug)]
pub struct SearchOpts {
    pub category: String,
    pub max_price: Option<f64>,
    pub currency: String,
    pub online: bool,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self { category: String::new(), max_price: None, currency: String::new(), online: true }
    }
}

/// Builds the official search body: text plus optional category, price cap and online status.
pub fn search_body(text: &str, opts: &SearchOpts) -> serde_json::Value {
    let mut query = serde_json::Map::new();
    if opts.online {
        query.insert("status".into(), serde_json::json!({ "option": "online" }));
    }
    query.insert("type".into(), serde_json::json!(text));
    let mut filters = serde_json::Map::new();
    let category = opts.category.trim();
    if !category.is_empty() && category != "any" {
        filters.insert(
            "type_filters".into(),
            serde_json::json!({ "filters": { "category": { "option": category } } }),
        );
    }
    if let Some(max) = opts.max_price.filter(|p| *p > 0.0) {
        let currency = if opts.currency.trim().is_empty() { "exalted" } else { opts.currency.trim() };
        filters.insert(
            "trade_filters".into(),
            serde_json::json!({ "filters": { "price": { "max": max, "option": currency } } }),
        );
    }
    if !filters.is_empty() {
        query.insert("filters".into(), serde_json::Value::Object(filters));
    }
    serde_json::json!({ "query": serde_json::Value::Object(query), "sort": { "price": "asc" } })
}

pub fn search_text(query_text: &str, league_pref: &str) -> Result<(Vec<ListingView>, String), String> {
    search_filtered(query_text, league_pref, &SearchOpts::default())
}

pub fn search_filtered(
    query_text: &str,
    league_pref: &str,
    opts: &SearchOpts,
) -> Result<(Vec<ListingView>, String), String> {
    let text = query_text.trim();
    if text.chars().count() < 2 {
        return Err("Слишком короткий запрос: минимум 2 символа".into());
    }
    let league = resolve_league(league_pref)?;
    let url = format!("{BASE}/api/trade2/search/poe2/{}", encode(&league));
    let body = search_body(text, opts);
    let mut res = agent()
        .post(&url)
        .header("User-Agent", &ua())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|e| format!("Сеть: {e}"))?;
    let status = res.status().as_u16();
    let text_body = res.body_mut().read_to_string().map_err(|e| format!("Чтение ответа: {e}"))?;
    if status == 429 {
        return Err("Слишком часто: trade API просит подождать.".into());
    }
    if !(200..300).contains(&status) {
        return Err(format!("Trade API HTTP {status}: {}", text_body.chars().take(200).collect::<String>()));
    }
    let (id, _total, hashes) = parse_search(&text_body)?;
    if hashes.is_empty() {
        return Ok((Vec::new(), format!("{BASE}/trade2/search/poe2/{}/{}", encode(&league), id)));
    }
    let hashes: Vec<&str> = hashes.iter().take(10).map(|s| s.as_str()).collect();
    let fetch_url = format!("{BASE}/api/trade2/fetch/{}?query={}", hashes.join(","), encode(&id));
    let listings = parse_listings(&get_json(&fetch_url)?)?;
    Ok((listings, format!("{BASE}/trade2/search/poe2/{}/{}", encode(&league), id)))
}

fn build_text_url(league: &str, text: &str) -> String {
    let body = serde_json::json!({ "query": { "status": { "option": "online" }, "type": text }, "sort": { "price": "asc" } });
    format!("{BASE}/trade2/search/poe2/{}?q={}", encode(league), encode(&body.to_string()))
}

/// Cheapest and median price of a listing set (most common currency).
pub fn summarize_prices(listings: &[ListingView]) -> (String, f64, f64) {
    let pairs: Vec<(f64, String)> = listings.iter().map(|l| (l.price, l.currency.clone())).collect();
    let (currency, min, median, _) = summarize_listings(&pairs);
    (currency, min, median)
}

/// Runs a ready-made request from a build guide (Mobalytics stores the whole
/// search body, we only add the league).
pub fn search_raw(query_json: &str, league_pref: &str) -> Result<(Vec<ListingView>, String), String> {
    let body: serde_json::Value = serde_json::from_str(query_json).map_err(|e| format!("Запрос гайда не разобран: {e}"))?;
    if body.get("query").is_none() {
        return Err("В запросе гайда нет поля query".into());
    }
    let league = resolve_league(league_pref)?;
    let url = format!("{BASE}/api/trade2/search/poe2/{}", encode(&league));
    let mut res = agent()
        .post(&url)
        .header("User-Agent", &ua())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|e| format!("Сеть: {e}"))?;
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().map_err(|e| format!("Чтение ответа: {e}"))?;
    if status == 429 {
        return Err("Слишком часто: trade API просит подождать.".into());
    }
    if !(200..300).contains(&status) {
        return Err(format!("Trade API HTTP {status}: {}", text.chars().take(200).collect::<String>()));
    }
    let (id, _total, hashes) = parse_search(&text)?;
    let trade_url = format!("{BASE}/trade2/search/poe2/{}/{}", encode(&league), id);
    if hashes.is_empty() {
        return Ok((Vec::new(), trade_url));
    }
    let hashes: Vec<&str> = hashes.iter().take(10).map(|s| s.as_str()).collect();
    let fetch_url = format!("{BASE}/api/trade2/fetch/{}?query={}", hashes.join(","), encode(&id));
    let listings = parse_listings(&get_json(&fetch_url)?)?;
    Ok((listings, trade_url))
}

fn poe2_leagues() -> Result<Vec<String>, String> {
    let text = get_json(&format!("{BASE}/api/trade2/data/leagues"))?;
    let leagues: Leagues = serde_json::from_str(&text).map_err(|e| format!("Список лиг не разобран: {e}"))?;
    Ok(leagues
        .result
        .iter()
        .filter(|l| l.realm == "poe2" || l.realm.is_empty())
        .map(|l| l.id.clone())
        .filter(|id| !id.starts_with("HC"))
        .collect())
}

/// Ratios from bulk-exchange offers: `exchange` is what the seller wants,
/// `item` is what they give (1 divine for N exalted).
pub fn parse_exchange_ratios(json: &str) -> Vec<f64> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let Some(result) = value.get("result").and_then(|v| v.as_object()) else { return Vec::new() };
    let mut ratios = Vec::new();
    for listing in result.values() {
        let Some(offers) = listing.get("listing").and_then(|l| l.get("offers")).and_then(|o| o.as_array()) else { continue };
        for offer in offers {
            let want = offer.get("exchange").map(|e| (e.get("amount").and_then(|a| a.as_f64()).unwrap_or(0.0), e.get("currency").and_then(|c| c.as_str()).unwrap_or("")));
            let give = offer.get("item").map(|i| (i.get("amount").and_then(|a| a.as_f64()).unwrap_or(0.0), i.get("currency").and_then(|c| c.as_str()).unwrap_or("")));
            let (Some((want_amount, want_currency)), Some((give_amount, give_currency))) = (want, give) else { continue };
            if want_currency == "exalted" && give_currency == "divine" && want_amount > 0.0 && give_amount > 0.0 {
                ratios.push(want_amount / give_amount);
            }
        }
    }
    ratios
}

/// Prices of Divine Orb listings that are priced in exalted.
pub fn parse_divine_prices(json: &str) -> Vec<f64> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let Some(result) = value.get("result").and_then(|v| v.as_array()) else { return Vec::new() };
    let mut prices = Vec::new();
    for entry in result {
        let Some(price) = entry.get("listing").and_then(|l| l.get("price")) else { continue };
        if price.get("currency").and_then(|c| c.as_str()) == Some("exalted") {
            if let Some(amount) = price.get("amount").and_then(|a| a.as_f64()) {
                if amount > 0.0 {
                    prices.push(amount);
                }
            }
        }
    }
    prices
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

fn rate_for(league: &str) -> Option<CurrencyRate> {
    // 1) Bulk exchange: buy divine with exalted.
    let body = serde_json::json!({
        "exchange": { "status": { "option": "online" }, "have": ["exalted"], "want": ["divine"], "minimum": 1 }
    });
    if let Ok(res) = agent()
        .post(&format!("{BASE}/api/trade2/exchange/poe2/{}", encode(league)))
        .header("User-Agent", &ua())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(body.to_string().as_bytes())
    {
        let mut res = res;
        if (200..300).contains(&res.status().as_u16()) {
            if let Ok(text) = res.body_mut().read_to_string() {
                let ratios = parse_exchange_ratios(&text);
                if !ratios.is_empty() {
                    return Some(CurrencyRate {
                        league: league.to_string(),
                        exalted_per_divine: median(ratios.clone()),
                        chaos_per_divine: None,
                        offers: ratios.len(),
                        source: "exchange".into(),
                        updated: now_iso(),
                    });
                }
            }
        }
    }
    // 2) Fallback: priced Divine Orb listings.
    let query = serde_json::json!({
        "query": {
            "status": { "option": "online" },
            "type": "Divine Orb",
            "filters": { "trade_filters": { "filters": { "price": { "option": "exalted" } } } }
        },
        "sort": { "price": "asc" }
    });
    let res = agent()
        .post(&format!("{BASE}/api/trade2/search/poe2/{}", encode(league)))
        .header("User-Agent", &ua())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(query.to_string().as_bytes())
        .ok()?;
    let mut res = res;
    if !(200..300).contains(&res.status().as_u16()) {
        return None;
    }
    let text = res.body_mut().read_to_string().ok()?;
    let (id, _total, hashes) = parse_search(&text).ok()?;
    if hashes.is_empty() {
        return None;
    }
    let hashes: Vec<&str> = hashes.iter().take(10).map(|s| s.as_str()).collect();
    let fetch_url = format!("{BASE}/api/trade2/fetch/{}?query={}", hashes.join(","), encode(&id));
    let prices = parse_divine_prices(&get_json(&fetch_url).ok()?);
    if prices.is_empty() {
        return None;
    }
    Some(CurrencyRate {
        league: league.to_string(),
        exalted_per_divine: median(prices.clone()),
        chaos_per_divine: None,
        offers: prices.len(),
        source: "listings".into(),
        updated: now_iso(),
    })
}

/// Divine price for the configured league, then for Standard.
pub fn currency_rate(league_pref: &str) -> Result<CurrencyRate, String> {
    let mut candidates: Vec<String> = Vec::new();
    if !league_pref.trim().is_empty() {
        candidates.push(league_pref.trim().to_string());
    } else if let Ok(league) = resolve_league("") {
        candidates.push(league);
    }
    if let Ok(leagues) = poe2_leagues() {
        for league in leagues {
            if !candidates.contains(&league) {
                candidates.push(league);
            }
        }
    }
    candidates.truncate(3);
    for league in candidates {
        if let Some(rate) = rate_for(&league) {
            return Ok(rate);
        }
    }
    Err("Официальный API не отдал ни одного предложения обмена — попробуй позже.".into())
}

/// Downloads the stat dataset (one request, cached by the caller).
pub fn fetch_stats() -> Result<Vec<StatDef>, String> {
    let text = get_json(&format!("{BASE}/api/trade2/data/stats"))?;
    parse_stats(&text)
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
///
/// With `stats` the item's modifiers become filters; if that leaves no results,
/// the search is repeated without them and the note says so.
pub fn check(item: &ParsedItem, league_pref: &str, stats: Option<&[StatDef]>, limit: usize, headroom: u8) -> Result<PriceResult, String> {
    let league = resolve_league(league_pref)?;
    let (filters, unmatched) = stats
        .map(|s| match_stats(item, s, limit.min(3), headroom))
        .unwrap_or_default();
    let mut result = search(item, &league, &filters, !filters.is_empty())?;
    result.unmatched = unmatched;
    result.matched = filters.len();
    if result.total == 0 && filters.len() > 2 {
        result = search(item, &league, &filters[..2], true)?;
        if result.total > 0 {
            result.note = "Похожие: 2 главных мода с допуском 10%.".into();
        }
    }
    if result.total == 0 && !filters.is_empty() {
        result = search(item, &league, &[], false)?;
        result.note = "По модификаторам ничего не найдено — показаны цены по базе и редкости.".into();
    }
    if !result.unmatched.is_empty() {
        let base = result.note.trim().to_string();
        result.note = if base.is_empty() {
            format!("Не распознано модов: {}.", result.unmatched.len())
        } else {
            format!("{base} Не распознано модов: {}.", result.unmatched.len())
        };
    }
    Ok(result)
}

fn search(item: &ParsedItem, league: &str, stat_filters: &[StatFilter], used_stats: bool) -> Result<PriceResult, String> {
    let query = build_query_with(item, stat_filters);
    let body = serde_json::json!({ "query": query, "sort": { "price": "asc" } });
    let url = format!("{BASE}/api/trade2/search/poe2/{}", encode(league));
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
        league: league.to_string(),
        total,
        trade_url: format!("{BASE}/trade2/search/poe2/{}/{}", encode(league), id),
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
    if used_stats {
        result.note = format!("Учтено модификаторов: {}.", stat_filters.len());
    } else if !is_unique(item) {
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
    fn shapes_ignore_numbers_and_signs() {
        assert_eq!(shape("+18 to maximum Life"), "# to maximum life");
        assert_eq!(shape("+22% to Fire Resistance"), "#% to fire resistance");
        assert_eq!(shape("Adds 5 to 10 Physical Damage"), "adds # to # physical damage");
        assert_eq!(shape("-15% to Cold Resistance"), "#% to cold resistance");
    }

    #[test]
    fn stats_dataset_is_parsed_and_matched() {
        let json = r##"{"result":[
            {"id":"explicit","entries":[
                {"id":"explicit.stat_1","text":"# to maximum Life","type":"explicit"},
                {"id":"explicit.stat_2","text":"#% to Fire Resistance","type":"explicit"},
                {"id":"explicit.stat_3","text":"Adds # to # Physical Damage","type":"explicit"},
                {"id":"explicit.stat_4","text":"# to Strength","type":"explicit"}
            ]}
        ]}"##;
        let stats = parse_stats(json).unwrap();
        assert_eq!(stats.len(), 4);
        let item = ParsedItem {
            rarity: "Rare".into(),
            mods: vec![
                "+18 to maximum Life".into(),
                "+22% to Fire Resistance".into(),
                "+5 to Strength".into(),
                "+1 to a stat nobody knows".into(),
            ],
            ..Default::default()
        };
        let (filters, _unmatched) = match_stats(&item, &stats, 3, 10);
        assert_eq!(filters.len(), 3);
        assert_eq!(filters[0].id, "explicit.stat_1", "life is defensive first");
        assert_eq!(filters[0].min, 16.0);
        assert!(filters.iter().any(|f| f.id == "explicit.stat_2" && f.min == 19.0));
        let q = build_query_with(&item, &filters);
        assert_eq!(q["stats"][0]["type"], "and");
        assert_eq!(q["stats"][0]["filters"].as_array().unwrap().len(), 3);
        assert_eq!(q["stats"][0]["filters"][0]["value"]["min"], 16.0);
    }

    #[test]
    fn two_number_stats_use_min_and_max() {
        let json = r##"{"result":[{"id":"explicit","entries":[{"id":"explicit.stat_3","text":"Adds # to # Physical Damage","type":"explicit"}]}]}"##;
        let stats = parse_stats(json).unwrap();
        let item = ParsedItem { mods: vec!["Adds 5 to 10 Physical Damage".into()], ..Default::default() };
        let (filters, _unmatched) = match_stats(&item, &stats, 3, 10);
        assert_eq!(filters.len(), 1);
        assert_eq!((filters[0].min, filters[0].max), (4.0, Some(10.0)));
    }

    #[test]
    fn text_search_listings_are_parsed() {
        let json = r#"{"result":[{"listing":{"price":{"amount":3,"currency":"divine"},"account":{"name":"seller#1234"},"whisper":"@seller Hi, I would like to buy"},"item":{"name":"Doom Loop","typeLine":"Prismatic Ring","rarity":"rare"}}]}"#;
        let listings = parse_listings(json).unwrap();
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].account, "seller#1234");
        assert!(listings[0].whisper.contains("would like to buy"));
        assert_eq!(listings[0].base, "Prismatic Ring");
    }

    #[test]
    fn text_search_url_is_encoded() {
        let url = build_text_url("Standard", "Prismatic Ring");
        assert!(url.starts_with("https://www.pathofexile.com/trade2/search/poe2/Standard?q="));
        assert!(url.contains("Prismatic%20Ring"));
    }

    #[test]
    fn exchange_ratios_are_parsed() {
        let json = r#"{"result":{"a":{"listing":{"offers":[
            {"exchange":{"currency":"exalted","amount":499},"item":{"currency":"divine","amount":1}},
            {"exchange":{"currency":"exalted","amount":510},"item":{"currency":"divine","amount":1}},
            {"exchange":{"currency":"divine","amount":1},"item":{"currency":"exalted","amount":1}}
        ]}}},"total":1}"#;
        let ratios = parse_exchange_ratios(json);
        assert_eq!(ratios, vec![499.0, 510.0]);
    }

    #[test]
    fn divine_listing_prices_are_parsed() {
        let json = r#"{"result":[
            {"listing":{"price":{"amount":499,"currency":"exalted"}}},
            {"listing":{"price":{"amount":1,"currency":"divine"}}},
            {"listing":{"price":{"amount":505,"currency":"exalted"}}}
        ]}"#;
        assert_eq!(parse_divine_prices(json), vec![499.0, 505.0]);
        assert_eq!(median(vec![505.0, 499.0, 510.0]), 505.0);
    }

    #[test]
    fn search_body_applies_filters() {
        let opts = SearchOpts {
            category: "accessory".into(),
            max_price: Some(5.0),
            currency: "exalted".into(),
            online: true,
        };
        let body = search_body("Stellar Amulet", &opts);
        assert_eq!(body["query"]["filters"]["type_filters"]["filters"]["category"]["option"], "accessory");
        assert_eq!(body["query"]["filters"]["trade_filters"]["filters"]["price"]["max"], 5.0);
        assert_eq!(body["query"]["filters"]["trade_filters"]["filters"]["price"]["option"], "exalted");
        assert_eq!(body["query"]["status"]["option"], "online");
        let offline = search_body("Stellar Amulet", &SearchOpts { online: false, ..Default::default() });
        assert!(offline["query"].get("status").is_none());
        assert!(offline["query"].get("filters").is_none());
    }

    #[test]
    fn empty_listings_are_safe() {
        let (currency, min, median, samples) = summarize_listings(&[]);
        assert!(currency.is_empty());
        assert_eq!((min, median), (0.0, 0.0));
        assert!(samples.is_empty());
    }
}
