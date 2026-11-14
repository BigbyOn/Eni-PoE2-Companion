//! Recognises a trade whisper ("I would like to buy your ...") in EN and RU.
//!
//! PoE2 logs whispers as `@From Name: text` after the engine tag (older clients
//! dropped the From/To marker). We keep only structural facts: item, price,
//! league and the optional stash position for a reply.

use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct TradeRequest {
    pub buyer: String,
    pub item: String,
    pub price: String,
    pub league: String,
    pub stash: Option<String>,
    pub left: Option<u32>,
    pub top: Option<u32>,
    pub raw: String,
    pub incoming: bool,
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    haystack.to_lowercase().find(&needle.to_lowercase())
}

fn cut_after<'a>(text: &'a str, needles: &[&str]) -> Option<(&'a str, &'a str)> {
    for needle in needles {
        if let Some(i) = find_ci(text, needle) {
            let start = i + needle.len();
            return Some((&text[start..], &text[..i]));
        }
    }
    None
}

fn clean(value: &str) -> String {
    value.trim().trim_matches(|c: char| c == '.' || c == ',' || c == ';' || c == '"').trim().to_string()
}

fn digit_after(text: &str, needles: &[&str]) -> Option<u32> {
    for needle in needles {
        if let Some(i) = find_ci(text, needle) {
            let rest = text[i + needle.len()..].trim_start();
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(value) = digits.parse() {
                return Some(value);
            }
        }
    }
    None
}

fn quoted_after(text: &str, needles: &[&str]) -> Option<String> {
    for needle in needles {
        if let Some(i) = find_ci(text, needle) {
            let rest = &text[i + needle.len()..];
            let end = rest.find('"')?;
            let value = rest[..end].trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Returns a request only for an incoming sale; outgoing purchase whispers are
/// ignored (the player already knows what they are buying).
pub fn parse(buyer: &str, text: &str, incoming: Option<bool>) -> Option<TradeRequest> {
    if incoming == Some(false) {
        return None;
    }
    let (after_buy, _) = cut_after(
        text,
        &[
            "i would like to buy your ",
            "i'd like to buy your ",
            "i want to buy your ",
            "хочу купить у вас ",
            "хочу приобрести у вас ",
        ],
    )?;
    let (after_item, item) = cut_after(
        after_buy,
        &[" listed for ", " for ", " за "],
    )?;
    let item = clean(item);
    if item.is_empty() || item.chars().count() > 160 {
        return None;
    }
    let (after_price, price) = cut_after(after_item, &[" in ", " в лиге "])?;
    let price = clean(price);
    let league = clean(after_price.split('(').next().unwrap_or(""));
    if price.is_empty() || league.is_empty() {
        return None;
    }
    let stash = quoted_after(text, &["stash tab \"", "сундук \""]);
    let left = digit_after(text, &["position: left ", "позиция: слева ", "слева "]);
    let top = digit_after(text, &["top ", "сверху "]);
    Some(TradeRequest {
        buyer: buyer.trim().to_string(),
        item,
        price,
        league,
        stash,
        left,
        top,
        raw: text.chars().take(400).collect(),
        incoming: incoming.unwrap_or(true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_english_trade_whisper() {
        let text = "Hi, I would like to buy your Doom Loop Prismatic Ring listed for 5 exalted in Forbidden Rites (stash tab \"sell\"; position: left 3, top 7)";
        let req = parse("Buyer#1234", text, Some(true)).expect("trade request");
        assert_eq!(req.item, "Doom Loop Prismatic Ring");
        assert_eq!(req.price, "5 exalted");
        assert_eq!(req.league, "Forbidden Rites");
        assert_eq!(req.stash.as_deref(), Some("sell"));
        assert_eq!((req.left, req.top), (Some(3), Some(7)));
        assert!(req.incoming);
    }

    #[test]
    fn parses_russian_trade_whisper() {
        let text = "Здравствуйте, хочу купить у вас Кольцо Судьбы за 2 divine в лиге Forbidden Rites (Сундук \"продажа\"; позиция: слева 4, сверху 9)";
        let req = parse("Покупатель", text, None).expect("trade request");
        assert_eq!(req.item, "Кольцо Судьбы");
        assert_eq!(req.price, "2 divine");
        assert_eq!(req.league, "Forbidden Rites");
        assert_eq!(req.stash.as_deref(), Some("продажа"));
    }

    #[test]
    fn ignores_outgoing_and_non_trade_whispers() {
        assert!(parse("B", "I would like to buy your X listed for 1 exalted in L", Some(false)).is_none());
        assert!(parse("B", "hey, are you online?", Some(true)).is_none());
    }
}
