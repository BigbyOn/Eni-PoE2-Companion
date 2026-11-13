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
