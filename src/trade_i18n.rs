//! Russian client support for the English-only trade API.
//!
//! The official trade API knows only English item bases and stat ids. The game
//! client writes localized item text, so Russian items cannot be priced without
//! a translation table. The tables in `assets/trade_i18n` are derived from the
//! Exiled Exchange 2 data files (MIT, see NOTICE): `ru_items.json` maps a
//! Russian base/unique name to its English `refName`, `ru_stats.json` maps a
//! Russian modifier template to an English stat id.
//!
//! Translation is best-effort: when a base or modifier is missing, the item is
//! passed through unchanged and the caller falls back to the modifier search.

use crate::item::ParsedItem;
use crate::trade::StatFilter;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(serde::Deserialize)]
struct StatEntry {
    /// English stat text, for diagnostics.
    r: String,
    /// Russian templates with `#` placeholders.
    m: Vec<String>,
    /// Trade API stat id (`explicit.stat_...`).
    id: String,
}

static ITEMS: OnceLock<HashMap<String, String>> = OnceLock::new();
static STATS: OnceLock<Vec<StatEntry>> = OnceLock::new();
static TAGS: OnceLock<HashMap<String, String>> = OnceLock::new();

fn norm(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .replace('ё', "е")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Replaces numbers (with optional sign) by `#`, drops signs, collapses spaces.
fn shape(value: &str) -> String {
    let mut out = String::new();
    let mut in_number = false;
    for ch in value.to_lowercase().replace('ё', "е").chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == ',' {
            if !in_number {
                out.push('#');
                in_number = true;
            }
        } else if ch == '+' || ch == '-' {
            // A sign only belongs to the following number.
        } else {
            in_number = false;
            out.push(ch);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn items() -> &'static HashMap<String, String> {
    ITEMS.get_or_init(|| {
        let raw = include_str!("../assets/trade_i18n/ru_items.json");
        let parsed: HashMap<String, String> = serde_json::from_str(raw).unwrap_or_default();
        parsed.into_iter().map(|(ru, en)| (norm(&ru), en)).collect()
    })
}

fn stats() -> &'static Vec<StatEntry> {
    STATS.get_or_init(|| {
        let raw = include_str!("../assets/trade_i18n/ru_stats.json");
        serde_json::from_str(raw).unwrap_or_default()
    })
}

fn tags() -> &'static HashMap<String, String> {
    TAGS.get_or_init(|| {
        let raw = include_str!("../assets/trade_i18n/item_tags.json");
        let parsed: HashMap<String, String> = serde_json::from_str(raw).unwrap_or_default();
        parsed.into_iter().map(|(name, tag)| (norm(&name), tag)).collect()
    })
}

/// Trade API tag (`regal-shard`, `divine`, …) for a base/unique name.
pub fn trade_tag(name: &str) -> Option<String> {
    tags().get(&norm(name)).cloned()
}

/// English base or unique name for a Russian one, when the table knows it.
pub fn translate_name(name: &str) -> Option<String> {
    items().get(&norm(name)).cloned()
}

/// Translates base/name in place. Mods are kept for diagnostics; price filters
/// come from `filters` because stat ids are more reliable than English text.
pub fn localize(mut item: ParsedItem) -> ParsedItem {
    if let Some(base) = translate_name(&item.base) {
        item.base = base;
    }
    if let Some(name) = translate_name(&item.name) {
        item.name = name;
    }
    item
}

fn numbers(value: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let negative = chars[i] == '-';
        if chars[i].is_ascii_digit() || negative {
            let start = if negative { i + 1 } else { i };
            let mut j = start;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.' || chars[j] == ',') {
                j += 1;
            }
            if j > start {
                let text: String = chars[start..j].iter().collect::<String>().replace(',', ".");
                if let Ok(value) = text.parse::<f64>() {
                    out.push(if negative { -value } else { value });
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Modifier filters for Russian mods, matching templates from the table.
/// Returns the filters and the number of unrecognised lines.
pub fn filters(mods: &[String], limit: usize, headroom: u8) -> (Vec<StatFilter>, usize) {
    let factor = 1.0 - f64::from(headroom.min(50)) / 100.0;
    let mut out: Vec<StatFilter> = Vec::new();
    let mut unmatched = 0;
    for line in mods {
        let wanted = shape(line);
        if wanted.is_empty() {
            continue;
        }
        let found = stats()
            .iter()
            .find(|entry| entry.m.iter().any(|matcher| shape(matcher) == wanted));
        let Some(entry) = found else {
            unmatched += 1;
            continue;
        };
        let values = numbers(line);
        let Some(first) = values.first().copied() else {
            unmatched += 1;
            continue;
        };
        if first < 0.0 {
            unmatched += 1;
            continue;
        }
        let min = (first * factor).floor().max(1.0);
        out.push(StatFilter {
            id: entry.id.clone(),
            min,
            max: None,
            text: entry.r.clone(),
        });
        if out.len() >= limit {
            break;
        }
    }
    (out, unmatched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ParsedItem;

    #[test]
    fn translates_a_russian_base() {
        assert_eq!(translate_name("Студёный посох").as_deref(), Some("Gelid Staff"));
        assert!(translate_name("Такой базы нет").is_none());
        assert_eq!(trade_tag("Осколок царей").as_deref(), Some("regal-shard"));
    }

    #[test]
    fn builds_stat_filters_for_russian_mods() {
        let mods = vec!["+20 к интеллекту".to_string(), "+15% к сопротивлению холоду".to_string()];
        let (filters, unmatched) = filters(&mods, 2, 10);
        assert_eq!(unmatched, 0);
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].id, "explicit.stat_328541901");
        assert!(filters[0].min >= 18.0);
    }

    #[test]
    fn localize_replaces_base_and_name() {
        let item = ParsedItem {
            name: "Студёный посох".into(),
            base: "Студёный посох".into(),
            ..Default::default()
        };
        let item = localize(item);
        assert_eq!(item.base, "Gelid Staff");
    }

    #[test]
    fn parses_and_translates_real_ru_item() {
        let text = "Класс предмета: Шлемы\nРедкость: Редкий\nИспорченный спаситель\nРжавый большой шлем\n--------\nБроня: 29\n--------\nТребуется: Уровень 13\n--------\n+23 к максимуму здоровья\n+10 к силе\n+5 к интеллекту\nРегенерация 5.8 здоровья в секунду";
        let item = crate::item::parse(text).expect("parse");
        println!("MODS={:?} BASE={} NAME={} RARITY={} CLASS={}", item.mods, item.base, item.name, item.rarity, item.class);
        let (filters, unmatched) = filters(&item.mods, 3, 10);
        println!("FILTERS={:?} UNMATCHED={}", filters, unmatched);
        assert!(!filters.is_empty(), "russian mods should translate");
    }
}
