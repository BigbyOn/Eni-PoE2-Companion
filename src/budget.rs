//! Hard budget rules and league-safe currency conversion.
//!
//! The language model may translate "до 20 экзальтов" into a number, but the
//! decision whether a set of items fits is made here, by code:
//!
//! * the **global** limit is a hard ceiling for the whole set;
//! * the **per-item** limit is checked for every quote;
//! * quotes from another league are rejected, never mixed;
//! * conversion uses the rate of the **same league**, with its source and age;
//! * if a rate is unknown, the set is reported as "cannot be verified", never
//!   as "fits".
//!
//! Nothing here performs network requests or looks at the UI.

use serde::{Deserialize, Serialize};

pub const EXALTED: &str = "exalted";
pub const DIVINE: &str = "divine";
pub const CHAOS: &str = "chaos";

const EPSILON: f64 = 1e-9;
/// A rate older than this may not be used to confirm a mixed-currency set.
pub const MAX_RATE_AGE_SECONDS: u64 = 6 * 3600;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Money {
    pub amount: f64,
    #[serde(default)]
    pub currency: String,
}

impl Money {
    pub fn new(amount: f64, currency: &str) -> Self {
        Money {
            amount,
            currency: currency.to_string(),
        }
    }
}

/// Currency rate of one league. Mirrors `trade::CurrencyRate` and `scout::Scout`
/// so the UI and the voice bridge report the same numbers.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Rates {
    #[serde(default)]
    pub league: String,
    #[serde(default)]
    pub exalted_per_divine: Option<f64>,
    #[serde(default)]
    pub chaos_per_divine: Option<f64>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub as_of: String,
    #[serde(default)]
    pub age_seconds: Option<u64>,
}

impl Rates {
    pub fn known(&self) -> bool {
        self.exalted_per_divine.map(|v| v > 0.0).unwrap_or(false)
    }

    /// A stale rate may still be displayed, but it must not confirm a
    /// mixed-currency budget.
    pub fn is_stale(&self) -> bool {
        self.age_seconds
            .map(|seconds| seconds > MAX_RATE_AGE_SECONDS)
            .unwrap_or(true)
    }

    pub fn note(&self) -> String {
        if !self.known() {
            return "Курс валют этой лиги неизвестен: нельзя утверждать, что комплект укладывается в бюджет."
                .to_string();
        }
        let age = match self.age_seconds {
            Some(secs) if secs < 120 => format!("{secs} с назад"),
            Some(secs) if secs < 7200 => format!("{} мин назад", secs / 60),
            Some(secs) => format!("{} ч назад", secs / 3600),
            None => "свежесть неизвестна".to_string(),
        };
        format!(
            "Курс лиги {}: 1 divine = {:.2} exalted, источник {}, обновлён {}.",
            if self.league.is_empty() {
                "?"
            } else {
                &self.league
            },
            self.exalted_per_divine.unwrap_or(0.0),
            if self.source.is_empty() {
                "не указан"
            } else {
                &self.source
            },
            age
        )
    }
}

/// Convert `amount` from one currency to another using the same-league rates.
/// `None` means "cannot be verified", which callers must not treat as zero.
pub fn convert(amount: f64, from: &str, to: &str, rates: &Rates) -> Option<f64> {
    let from = normalize_currency(from);
    let to = normalize_currency(to);
    if from.is_empty() || to.is_empty() {
        return None;
    }
    if from == to {
        return Some(amount);
    }

    // Everything goes through divine, the anchor of both rate sources.
    let to_divine = |value: f64, unit: &str| -> Option<f64> {
        match unit {
            DIVINE => Some(value),
            EXALTED => rates
                .exalted_per_divine
                .filter(|rate| *rate > 0.0)
                .map(|rate| value / rate),
            CHAOS => rates
                .chaos_per_divine
                .filter(|rate| *rate > 0.0)
                .map(|rate| value / rate),
            _ => None,
        }
    };
    let from_divine = |value: f64, unit: &str| -> Option<f64> {
        match unit {
            DIVINE => Some(value),
            EXALTED => rates
                .exalted_per_divine
                .filter(|rate| *rate > 0.0)
                .map(|rate| value * rate),
            CHAOS => rates
                .chaos_per_divine
                .filter(|rate| *rate > 0.0)
                .map(|rate| value * rate),
            _ => None,
        }
    };

    let divine = to_divine(amount, &from)?;
    from_divine(divine, &to)
}

pub fn normalize_currency(value: &str) -> String {
    match value.trim().to_lowercase().as_str() {
        "ex" | "exalt" | "exalted" | "экзальт" | "экзальтов" | "экзальта" => {
            EXALTED.to_string()
        }
        "div" | "divine" | "дивайн" | "дивин" | "божественная" => {
            DIVINE.to_string()
        }
        "chaos" | "хаос" => CHAOS.to_string(),
        other => other.to_string(),
    }
}

pub fn display_currency(value: &str) -> &'static str {
    match normalize_currency(value).as_str() {
        EXALTED => "ex",
        DIVINE => "div",
        CHAOS => "chaos",
        _ => "",
    }
}

pub fn format_money(money: &Money) -> String {
    let unit = display_currency(&money.currency);
    if unit.is_empty() {
        format!("{:.0} {}", money.amount, money.currency)
    } else {
        format!("{:.0} {unit}", money.amount)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Budget {
    pub global: Option<Money>,
    pub per_item: Option<Money>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Quote {
    #[serde(default)]
    pub slot: String,
    #[serde(default)]
    pub name: String,
    pub price: Money,
    #[serde(default)]
    pub league: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub source: String,
    /// Priority order inside the set: lower goes first when the budget cannot
    /// take everything.
    #[serde(default)]
    pub priority: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct QuoteDecision {
    pub slot: String,
    pub name: String,
    pub price: Money,
    pub converted: Option<Money>,
    pub league: String,
    pub url: String,
    pub accepted: bool,
    pub reason: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct PlanResult {
    /// The requested set, as a whole, fits the budget and is verifiable.
    pub fits: bool,
    pub budget_known: bool,
    pub currency: String,
    pub league: String,
    pub set_total: Option<Money>,
    pub accepted_total: Option<Money>,
    pub remaining: Option<Money>,
    pub accepted: Vec<QuoteDecision>,
    pub rejected: Vec<QuoteDecision>,
    pub violations: Vec<String>,
    pub alternatives: Vec<String>,
    pub rate_note: String,
}

/// Plan a set of quotes against the hard limits.
///
/// `fits` describes the **requested set**. When it is false, `accepted` may
/// still contain a smaller subset in priority order, but callers must present it
/// as a partial option, never as "the set".
pub fn plan(quotes: &[Quote], budget: &Budget, rates: &Rates, league: &str) -> PlanResult {
    let currency = budget
        .global
        .as_ref()
        .map(|m| normalize_currency(&m.currency))
        .or_else(|| {
            budget
                .per_item
                .as_ref()
                .map(|m| normalize_currency(&m.currency))
        })
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| EXALTED.to_string());

    let mut violations: Vec<String> = Vec::new();
    let mut alternatives: Vec<String> = Vec::new();
    let mut budget_known = true;

    if !league.is_empty() {
        if !rates.league.is_empty() && rates.league != league {
            budget_known = false;
            violations.push(format!(
                "Курс валют относится к лиге {}, а поиск — в лиге {league}.",
                rates.league
            ));
        }
    }

    let per_item_limit = budget.per_item.as_ref().and_then(|limit| {
        let converted = convert(limit.amount, &limit.currency, &currency, rates);
        if converted.is_none() {
            budget_known = false;
            violations.push(format!(
                "Неизвестен курс для лимита на предмет ({}) в {}.",
                format_money(limit),
                display_currency(&currency)
            ));
        }
        converted
    });
    let global_limit = budget.global.as_ref().and_then(|limit| {
        let converted = convert(limit.amount, &limit.currency, &currency, rates);
        if converted.is_none() {
            budget_known = false;
            violations.push(format!(
                "Неизвестен курс для общего лимита ({}) в {}.",
                format_money(limit),
                display_currency(&currency)
            ));
        }
        converted
    });

    // Priority order: what the player asked to consider first.
    let mut ordered: Vec<&Quote> = quotes.iter().collect();
    ordered.sort_by_key(|quote| quote.priority);

    let mut decisions: Vec<QuoteDecision> = Vec::new();
    let mut set_total = 0.0;
    let mut every_quote_convertible = true;
    let mut every_quote_allowed = true;
    let mut seen_slots: Vec<String> = Vec::new();
    let mixed_currencies = quotes
        .iter()
        .any(|quote| normalize_currency(&quote.price.currency) != currency);

    for quote in ordered {
        if !quote.slot.is_empty() {
            if seen_slots.iter().any(|slot| slot == &quote.slot) {
                violations.push(format!(
                    "Слот {} указан дважды: альтернативные лоты одного слота не складываются в комплект.",
                    quote.slot
                ));
                every_quote_allowed = false;
                decisions.push(QuoteDecision {
                    slot: quote.slot.clone(),
                    name: quote.name.clone(),
                    price: quote.price.clone(),
                    converted: None,
                    league: quote.league.clone(),
                    url: quote.url.clone(),
                    accepted: false,
                    reason: "дубликат слота".into(),
                });
                continue;
            }
            seen_slots.push(quote.slot.clone());
        }
        let converted_amount = convert(quote.price.amount, &quote.price.currency, &currency, rates);
        let mut reason = String::new();
        let mut allowed = true;

        if !league.is_empty() && !quote.league.is_empty() && quote.league != league {
            allowed = false;
            every_quote_allowed = false;
            reason = format!(
                "другая лига ({} вместо {league}) — не смешиваем",
                quote.league
            );
        }

        if allowed {
            match converted_amount {
                Some(amount) => {
                    set_total += amount;
                    if let Some(limit) = per_item_limit {
                        if amount > limit + EPSILON {
                            allowed = false;
                            every_quote_allowed = false;
                            reason = format!(
                                "дороже лимита на предмет ({:.0} {} > {:.0} {})",
                                amount,
                                display_currency(&currency),
                                limit,
                                display_currency(&currency)
                            );
                            alternatives.push(format!(
                                "{}: искать дешевле {:.0} {} или убрать желаемый мод.",
                                if quote.name.is_empty() {
                                    &quote.slot
                                } else {
                                    &quote.name
                                },
                                limit,
                                display_currency(&currency)
                            ));
                        }
                    }
                }
                None => {
                    allowed = false;
                    every_quote_convertible = false;
                    budget_known = false;
                    reason = format!(
                        "неизвестен курс {} → {}",
                        quote.price.currency,
                        display_currency(&currency)
                    );
                    violations.push(format!(
                        "{}: {}",
                        if quote.name.is_empty() {
                            &quote.slot
                        } else {
                            &quote.name
                        },
                        reason
                    ));
                }
            }
        }

        decisions.push(QuoteDecision {
            slot: quote.slot.clone(),
            name: quote.name.clone(),
            price: quote.price.clone(),
            converted: converted_amount.map(|amount| Money::new(amount, &currency)),
            league: quote.league.clone(),
            url: quote.url.clone(),
            accepted: allowed,
            reason,
        });
    }

    if !every_quote_convertible {
        violations
            .push("Часть цен не переводится в общую валюту: комплект нельзя проверить.".into());
    }

    // Mixed currencies may only be compared with a fresh rate of the same league.
    if mixed_currencies && (rates.is_stale() || !rates.known()) {
        budget_known = false;
        violations.push(
            "Для смешанных валют нужен свежий курс той же лиги: курс отсутствует или устарел, бюджет не подтверждается."
                .into(),
        );
    }

    // Greedy selection in priority order for the partial suggestion.
    let mut accepted: Vec<QuoteDecision> = Vec::new();
    let mut rejected: Vec<QuoteDecision> = Vec::new();
    let mut accepted_total = 0.0;
    for mut decision in decisions {
        let amount = decision
            .converted
            .as_ref()
            .map(|money| money.amount)
            .unwrap_or(0.0);
        if !decision.accepted {
            decision.accepted = false;
            rejected.push(decision);
            continue;
        }
        if let Some(limit) = global_limit {
            if accepted_total + amount > limit + EPSILON {
                decision.accepted = false;
                decision.reason = format!(
                    "не влезает в общий бюджет: {:.0} + {:.0} > {:.0} {}",
                    accepted_total,
                    amount,
                    limit,
                    display_currency(&currency)
                );
                rejected.push(decision);
                continue;
            }
        }
        accepted_total += amount;
        accepted.push(decision);
    }

    let set_fits = budget_known
        && every_quote_allowed
        && every_quote_convertible
        && global_limit
            .map(|limit| set_total <= limit + EPSILON)
            .unwrap_or(true);

    if let Some(limit) = global_limit {
        if set_total > limit + EPSILON {
            violations.push(format!(
                "Комплект {:.0} {} при общем лимите {:.0} {} — не укладывается.",
                set_total,
                display_currency(&currency),
                limit,
                display_currency(&currency)
            ));
            alternatives.push(format!(
                "Убрать одну позицию или поднять общий лимит: сейчас помещается {} из {}.",
                accepted.len(),
                accepted.len() + rejected.len()
            ));
        }
    }

    PlanResult {
        fits: set_fits,
        budget_known,
        currency: currency.clone(),
        league: league.to_string(),
        set_total: Some(Money::new(set_total, &currency)),
        accepted_total: Some(Money::new(accepted_total, &currency)),
        remaining: global_limit
            .map(|limit| Money::new((limit - accepted_total).max(0.0), &currency)),
        accepted,
        rejected,
        violations,
        alternatives,
        rate_note: rates.note(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates() -> Rates {
        Rates {
            league: "Standard".into(),
            exalted_per_divine: Some(10.0),
            chaos_per_divine: Some(90.0),
            source: "poe2scout".into(),
            as_of: "2026-11-15T12:00:00Z".into(),
            age_seconds: Some(120),
        }
    }

    fn quote(slot: &str, amount: f64, currency: &str) -> Quote {
        Quote {
            slot: slot.into(),
            name: slot.into(),
            price: Money::new(amount, currency),
            league: "Standard".into(),
            url: format!("https://www.pathofexile.com/trade2/search/poe2/Standard/{slot}"),
            source: "trade".into(),
            priority: 0,
        }
    }

    fn budget(global: Option<f64>, per_item: Option<f64>) -> Budget {
        Budget {
            global: global.map(|value| Money::new(value, EXALTED)),
            per_item: per_item.map(|value| Money::new(value, EXALTED)),
        }
    }

    #[test]
    fn two_items_of_fifteen_cannot_fit_a_twenty_limit() {
        let quotes = vec![
            quote("boots", 15.0, EXALTED),
            quote("gloves", 15.0, EXALTED),
        ];
        let result = plan(&quotes, &budget(Some(20.0), None), &rates(), "Standard");

        assert!(!result.fits, "set of 30 ex must not fit a 20 ex limit");
        assert!(
            result.accepted.len() < 2,
            "both items must never be offered as one set"
        );
        assert_eq!(result.set_total.as_ref().unwrap().amount, 30.0);
        assert!(result
            .violations
            .iter()
            .any(|v| v.contains("не укладывается")));
    }

    #[test]
    fn set_fits_when_limit_is_enough() {
        let quotes = vec![
            quote("boots", 15.0, EXALTED),
            quote("gloves", 15.0, EXALTED),
        ];
        let result = plan(&quotes, &budget(Some(30.0), None), &rates(), "Standard");

        assert!(result.fits);
        assert_eq!(result.accepted.len(), 2);
        assert!((result.remaining.as_ref().unwrap().amount - 0.0).abs() < EPSILON);
        assert!(result.violations.is_empty());
    }

    #[test]
    fn per_item_limit_is_hard() {
        let quotes = vec![quote("boots", 15.0, EXALTED)];
        let result = plan(&quotes, &budget(None, Some(12.0)), &rates(), "Standard");

        assert!(!result.fits);
        assert!(result.accepted.is_empty());
        assert!(result.rejected[0].reason.contains("дороже лимита"));
    }

    #[test]
    fn other_leagues_are_rejected_not_mixed() {
        let mut other = quote("boots", 5.0, EXALTED);
        other.league = "Hardcore".into();
        let result = plan(&[other], &budget(Some(100.0), None), &rates(), "Standard");

        assert!(!result.fits);
        assert!(result.rejected[0].reason.contains("другая лига"));
    }

    #[test]
    fn unknown_rate_never_claims_the_set_fits() {
        let quotes = vec![quote("boots", 2.0, DIVINE)];
        let no_rate = Rates {
            league: "Standard".into(),
            ..Default::default()
        };
        let result = plan(&quotes, &budget(Some(50.0), None), &no_rate, "Standard");

        assert!(!result.budget_known);
        assert!(!result.fits);
        assert!(result
            .violations
            .iter()
            .any(|v| v.contains("неизвестен курс")));
    }

    #[test]
    fn conversion_between_currencies() {
        let rates = rates();
        let divine = convert(300.0, EXALTED, DIVINE, &rates).unwrap();
        assert!((divine - 30.0).abs() < EPSILON);
        let exalted = convert(30.0, DIVINE, EXALTED, &rates).unwrap();
        assert!((exalted - 300.0).abs() < EPSILON);
        assert!(convert(1.0, CHAOS, DIVINE, &rates).is_some());
        assert!(convert(1.0, "mirror", EXALTED, &rates).is_none());
    }

    #[test]
    fn global_limit_converted_from_another_currency() {
        let quotes = vec![quote("boots", 15.0, EXALTED)];
        let budget = Budget {
            global: Some(Money::new(2.0, DIVINE)), // 2 div = 20 ex
            per_item: None,
        };
        let result = plan(&quotes, &budget, &rates(), "Standard");
        assert!(result.fits);
        // The comparison currency follows the global limit: divine here.
        assert!((result.remaining.as_ref().unwrap().amount - 0.5).abs() < EPSILON);
        assert_eq!(result.currency, DIVINE);
    }

    #[test]
    fn duplicate_slot_quotes_never_form_a_set() {
        let quotes = vec![quote("boots", 5.0, EXALTED), quote("boots", 6.0, EXALTED)];
        let result = plan(&quotes, &budget(Some(100.0), None), &rates(), "Standard");
        assert!(!result.fits);
        assert!(result
            .violations
            .iter()
            .any(|v| v.contains("Слот boots указан дважды")));
        assert_eq!(result.accepted.len(), 1);
    }

    #[test]
    fn stale_rate_blocks_mixed_currency_confirmation() {
        let mut stale = rates();
        stale.age_seconds = Some(MAX_RATE_AGE_SECONDS + 1);
        let quotes = vec![quote("boots", 10.0, EXALTED), quote("belt", 1.0, DIVINE)];
        let result = plan(&quotes, &budget(Some(100.0), None), &stale, "Standard");
        assert!(result.budget_known == false);
        assert!(!result.fits);
        assert!(result.violations.iter().any(|v| v.contains("свежий курс")));
    }

    #[test]
    fn fresh_rate_allows_mixed_currency() {
        let quotes = vec![quote("boots", 10.0, EXALTED), quote("belt", 1.0, DIVINE)];
        let result = plan(&quotes, &budget(Some(100.0), None), &rates(), "Standard");
        assert!(result.budget_known);
        assert!(result.fits);
        assert_eq!(result.accepted.len(), 2);
    }

    #[test]
    fn no_budget_means_no_constraint_but_is_reported() {
        let quotes = vec![quote("boots", 15.0, EXALTED)];
        let result = plan(&quotes, &Budget::default(), &rates(), "Standard");
        assert!(result.fits);
        assert!(result.budget_known);
        assert_eq!(result.accepted.len(), 1);
    }
}
