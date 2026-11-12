//! Parses item text copied from the game with Ctrl+C and scores it against the
//! active build.
//!
//! The clipboard text is the only machine-readable item source PoE2 offers: the
//! log never contains item stats. Parsing is deliberately tolerant — English and
//! Russian clients, extra property lines, missing sections — and the analysis is
//! a keyword heuristic, not a replacement for a damage calculator.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct ParsedItem {
    pub class: String,
    pub rarity: String,
    pub name: String,
    pub base: String,
    pub item_level: Option<u32>,
    pub quality: Option<i32>,
    pub requirements: Vec<String>,
    pub mods: Vec<String>,
    pub flags: Vec<String>,
    /// Which gear slot the class points at ("ring", "boots", "bow", …).
    pub slot: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Analysis {
    /// Higher is better; the UI only uses it for ordering and a colour hint.
    pub score: i32,
    /// Build priorities the item actually covers, in Russian.
    pub matches: Vec<String>,
    /// What is missing or suspicious, in Russian.
    pub warnings: Vec<String>,
}

const SEPARATOR: &str = "--------";

fn field(line: &str, keys: &[&str]) -> Option<String> {
    let lower = line.trim().to_lowercase();
    for key in keys {
        if let Some(rest) = lower.strip_prefix(&key.to_lowercase()) {
            let value = rest.trim_start_matches(':').trim();
            if !value.is_empty() {
                // Return the original casing of the value.
                let idx = line.len() - value.len();
                return Some(line[idx..].trim().to_string());
            }
        }
    }
    None
}

fn first_number(value: &str) -> Option<i32> {
    let digits: String = value.chars().skip_while(|c| !c.is_ascii_digit() && *c != '-' && *c != '+').take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+').collect();
    digits.trim_start_matches('+').parse().ok()
}

/// Slot hint from the localised item class.
fn slot_of(class: &str) -> String {
    let c = class.to_lowercase();
    let table: &[(&[&str], &str)] = &[
        (&["ring", "кольц"], "ring"),
        (&["amulet", "амулет"], "amulet"),
        (&["belt", "пояс"], "belt"),
        (&["body armour", "нагрудник", "доспех"], "body"),
        (&["helmet", "шлем"], "helmet"),
        (&["glove", "перчат"], "gloves"),
        (&["boot", "сапог", "обувь"], "boots"),
        (&["shield", "щит"], "shield"),
        (&["quiver", "колчан"], "quiver"),
        (&["bow", "лук"], "bow"),
        (&["crossbow", "арбалет"], "crossbow"),
        (&["wand", "жезл"], "wand"),
        (&["staff", "посох"], "staff"),
        (&["sceptre", "скипетр"], "sceptre"),
        (&["mace", "булав", "молот"], "mace"),
        (&["axe", "топор"], "axe"),
        (&["sword", "меч"], "sword"),
        (&["spear", "копь"], "spear"),
        (&["waystone", "вайстоун", "вайстон"], "waystone"),
        (&["flask", "флакон"], "flask"),
        (&["jewel", "самоцвет"], "jewel"),
    ];
    for (keys, slot) in table {
        if keys.iter().any(|k| c.contains(k)) {
            return (*slot).to_string();
        }
    }
    String::new()
}

fn is_property(line: &str) -> bool {
    // Properties are "Key: value" lines ("Physical Damage: 45-88"), while a
    // modifier may merely contain the same words ("... 15% of Physical Damage").
    let Some((key, _)) = line.split_once(':') else { return false };
    let key = key.trim().to_lowercase();
    const KEYS: &[&str] = &[
        "quality",
        "physical damage",
        "elemental damage",
        "critical strike chance",
        "attacks per second",
        "weapon range",
        "armour",
        "evasion rating",
        "energy shield",
        "ward",
        "chance to block",
        "spirit",
        "stack size",
        "item level",
        "level",
        "str",
        "dex",
        "int",
        "requires",
        "sockets",
        "качество",
        "физический урон",
        "урон от стихий",
        "шанс критического удара",
        "атак в секунду",
        "дальность",
        "броня",
        "уклонение",
        "энергетический щит",
        "шанс блокировки",
        "дух",
        "размер стопки",
        "уровень предмета",
        "уровень",
        "сила",
        "ловкость",
        "интеллект",
        "требуется",
        "требования",
        "гнёзда",
    ];
    KEYS.iter().any(|k| key == *k || key.starts_with(k))
}

/// Parses the clipboard text. `None` means "this does not look like an item".
pub fn parse(text: &str) -> Option<ParsedItem> {
    let text = text.replace('\r', "");
    let mut lines: Vec<&str> = text.lines().map(|l| l.trim()).collect();
    if lines.is_empty() {
        return None;
    }
    // The game starts with the class line; accept text pasted with extra blank lines.
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    let mut item = ParsedItem::default();

    // Header: class, rarity, name, base.
    let mut header = Vec::new();
    for line in &lines {
        if *line == SEPARATOR {
            if !header.is_empty() {
                break;
            }
            continue;
        }
        if let Some(v) = field(line, &["Item Class", "Класс предмета"]) {
            item.class = v;
            continue;
        }
        if let Some(v) = field(line, &["Rarity", "Редкость"]) {
            item.rarity = v;
            continue;
        }
        header.push(*line);
    }
    if item.rarity.is_empty() && item.class.is_empty() {
        return None;
    }
    match header.len() {
        0 => {}
        1 => {
            item.name = header[0].to_string();
            item.base = header[0].to_string();
        }
        _ => {
            item.name = header[0].to_string();
            item.base = header[1].to_string();
        }
    }

    // Sections: properties, requirements, item level, then mods.
    let normalized = text.replace('\r', "");
    let sections: Vec<Vec<&str>> = normalized
        .split(SEPARATOR)
        .map(|s| s.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect())
        .collect();

    let mut mod_sections = Vec::new();
    for section in &sections {
        let mut is_mod_section = true;
        for line in section {
            if let Some(v) = field(line, &["Item Level", "Уровень предмета"]) {
                item.item_level = first_number(&v).map(|n| n.max(0) as u32);
                is_mod_section = false;
                continue;
            }
            if let Some(v) = field(line, &["Quality", "Качество"]) {
                item.quality = first_number(&v);
                is_mod_section = false;
                continue;
            }
            if field(line, &["Requirements", "Требования"]).is_some() {
                is_mod_section = false;
                continue;
            }
            if is_property(line) || line.starts_with("Level:") || line.starts_with("Уровень:") {
                if field(line, &["Requirements", "Требования"]).is_none() && !line.contains("Item Level") {
                    // Requirements block lines are kept for display when short.
                    if line.to_lowercase().starts_with("level") || line.to_lowercase().starts_with("уровень") || line.to_lowercase().starts_with("str") || line.to_lowercase().starts_with("dex") || line.to_lowercase().starts_with("int") || line.to_lowercase().starts_with("сила") || line.to_lowercase().starts_with("ловкость") || line.to_lowercase().starts_with("интеллект") {
                        if item.requirements.len() < 6 {
                            item.requirements.push((*line).to_string());
                        }
                    }
                }
                is_mod_section = false;
            }
            let l = line.to_lowercase();
            for (flag, words) in [
                ("corrupted", ["corrupted", "оскверн"]),
                ("unidentified", ["unidentified", "неопознан"]),
                ("mirrored", ["mirrored", "отзеркал"]),
                ("split", ["split", "раздел"]),
                ("fractured", ["fractured", "расколот"]),
            ] {
                if words.iter().any(|w| l.contains(w)) {
                    if !item.flags.iter().any(|f| f == flag) {
                        item.flags.push(flag.to_string());
                    }
                    is_mod_section = false;
                }
            }
        }
        if is_mod_section && !section.is_empty() {
            mod_sections.push(section.clone());
        }
    }

    // The header section and other non-mod sections end up in `mod_sections` too;
    // keep only lines that look like modifiers.
    for section in mod_sections {
        for line in section {
            let l = line.to_lowercase();
            if l == SEPARATOR || l == "requirements" || l == "требования" {
                continue;
            }
            if is_property(line) || field(line, &["Rarity", "Item Class", "Редкость", "Класс предмета"]).is_some() {
                continue;
            }
            if item.mods.iter().any(|m| m == line) || line == item.name || line == item.base {
                continue;
            }
            item.mods.push((*line).to_string());
        }
    }
    item.mods.truncate(24);
    item.slot = slot_of(&item.class);
    Some(item)
}

/// Keywords per build priority, English and Russian clients.
fn keywords(kind: &str) -> &'static [&'static str] {
    match kind {
        "life" => &["maximum life", "to life", "максимуму здоровья", "к здоровью", "здоровья"],
        "resist" => &[
            "fire resistance",
            "cold resistance",
            "lightning resistance",
            "chaos resistance",
            "elemental resistances",
            "сопротивлению огню",
            "сопротивлению холоду",
            "сопротивлению молнии",
            "сопротивлению хаосу",
            "сопротивления стихиям",
        ],
        "damage" => &["added", "increased", "damage", "attack speed", "critical", "урона", "урон", "скорости атаки", "критическ"],
        "attributes" => &["strength", "dexterity", "intelligence", "силе", "ловкости", "интеллекту"],
        "spirit" => &["spirit", "дух"],
        "movement" => &["movement speed", "скорость передвижения"],
        "mana" => &["mana", "мана", "маны"],
        _ => &[],
    }
}

fn line_matches(line: &str, kind: &str) -> bool {
    let l = line.to_lowercase();
    keywords(kind).iter().any(|k| l.contains(k))
}

/// A dangerous map/waystone modifier.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Danger {
    pub label: String,
    /// 2 = high, 1 = medium; the UI colours them differently.
    pub severity: u8,
}

/// Rules for map/waystone modifiers that kill characters (English and Russian).
const DANGER_RULES: &[(u8, &str, &[&str])] = &[
    (2, "рефлект урона", &["reflect", "отраж"]),
    (2, "нет регенерации", &["cannot regenerate", "не может восстанавливать", "не могут восстанавливать"]),
    (1, "снижено восстановление", &["less recovery", "сниженн", "меньше восстановления"]),
    (2, "дополнительный урон монстров", &["extra damage as", "дополнительного урона", "доп. урона", "дополнительным уроном"]),
    (2, "монстры невосприимчивы", &["monsters are immune", "монстры невосприимчивы", "иммунитет"]),
    (1, "крит монстров", &["increased critical strike chance", "повышенный шанс критического", "критического удара монстров"]),
    (1, "проклятие игроков", &["players are cursed", "игроки прокляты", "проклят"]),
    (2, "снижение максимума сопротивлений", &["maximum elemental resistances", "максимальных сопротивлений", "максимума сопротивлений"]),
    (1, "скорость монстров", &["attack and cast speed", "скорости атаки и сотворения", "скорости атаки"]),
    (1, "снижение зарядов флаконов", &["flask charges", "зарядов флаконов"]),
];

/// Dangerous modifiers of this item (meant for waystones/maps, safe for others).
pub fn dangers(item: &ParsedItem) -> Vec<Danger> {
    let mut out: Vec<Danger> = Vec::new();
    for line in &item.mods {
        let lower = line.to_lowercase();
        for (severity, label, patterns) in DANGER_RULES {
            if patterns.iter().any(|p| lower.contains(p)) && !out.iter().any(|d| d.label == *label) {
                out.push(Danger { label: (*label).to_string(), severity: *severity });
            }
        }
    }
    out.sort_by(|a, b| b.severity.cmp(&a.severity));
    out
}

/// Scores the item against the character's build priorities. Heuristic only.
pub fn analyze(item: &ParsedItem, level: u32) -> Analysis {
    let mut matches = Vec::new();
    let mut warnings = Vec::new();
    let mut score = 0i32;

    let has = |kind: &str| item.mods.iter().any(|m| line_matches(m, kind));

    if has("life") {
        matches.push("максимум жизни".to_string());
        score += 2;
    }
    if has("resist") {
        matches.push("сопротивления".to_string());
        score += 2;
    }
    if has("damage") {
        matches.push("урон".to_string());
        score += 1;
    }
    if has("attributes") {
        matches.push("характеристики".to_string());
        score += 1;
    }
    if has("spirit") {
        matches.push("дух".to_string());
        score += 2;
    }
    if has("movement") {
        matches.push("скорость передвижения".to_string());
        score += 2;
    }
    if has("mana") {
        matches.push("мана".to_string());
        score += 1;
    }

    let armour_slot = matches!(item.slot.as_str(), "body" | "helmet" | "gloves" | "boots" | "shield");
    if armour_slot && !has("life") {
        warnings.push("нет прибавки к максимуму жизни".to_string());
        score -= 1;
    }
    if armour_slot && !has("resist") {
        warnings.push("нет сопротивлений".to_string());
        score -= 1;
    }
    if item.flags.iter().any(|f| f == "unidentified") {
        warnings.push("предмет не опознан".to_string());
    }
    if let Some(ilvl) = item.item_level {
        if ilvl >= 1 && level >= 10 && ilvl + 12 < level {
            warnings.push(format!("низкий уровень предмета ({ilvl}) для {level}-го уровня"));
            score -= 1;
        }
    }
    if item.mods.is_empty() {
        warnings.push("в тексте не найдено модификаторов — проверь, что скопирован предмет, а не описание".to_string());
    }
    if item.rarity.to_lowercase().contains("normal") || item.rarity.to_lowercase().contains("обычн") {
        warnings.push("обычный предмет без модификаторов".to_string());
    }

    Analysis { score, matches, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN_RING: &str = "Item Class: Rings
Rarity: Rare
Doom Loop
Prismatic Ring
--------
Requirements:
Level: 32
--------
Item Level: 45
--------
+18 to maximum Life
+22% to Fire Resistance
+15% to Cold Resistance
+10% to all Elemental Resistances
--------
Corrupted
";

    const RU_BOW: &str = "Класс предмета: Луки
Редкость: Редкий
Гибель Грома
Двухструнный лук
--------
Качество: +12%
Физический урон: 41-76
Шанс критического удара: 5.00%
Атак в секунду: 1.20
--------
Требования:
Уровень: 45
Ловкость: 38
--------
Уровень предмета: 52
--------
+25 к ловкости
+30% к сопротивлению молнии
+15% к сопротивлению огню
--------
Осквернён
";

    #[test]
    fn parses_an_english_ring() {
        let i = parse(EN_RING).expect("ring parses");
        assert_eq!(i.class, "Rings");
        assert_eq!(i.rarity, "Rare");
        assert_eq!(i.name, "Doom Loop");
        assert_eq!(i.base, "Prismatic Ring");
        assert_eq!(i.item_level, Some(45));
        assert_eq!(i.slot, "ring");
        assert!(i.flags.iter().any(|f| f == "corrupted"));
        assert!(i.mods.iter().any(|m| m.contains("Fire Resistance")), "{:?}", i.mods);
        // Property lines must not be treated as modifiers.
        assert!(!i.mods.iter().any(|m| m.starts_with("Level")), "{:?}", i.mods);
    }

    #[test]
    fn parses_a_russian_bow() {
        let i = parse(RU_BOW).expect("bow parses");
        assert_eq!(i.class, "Луки");
        assert_eq!(i.name, "Гибель Грома");
        assert_eq!(i.base, "Двухструнный лук");
        assert_eq!(i.item_level, Some(52));
        assert_eq!(i.quality, Some(12));
        assert_eq!(i.slot, "bow");
        assert!(i.mods.iter().any(|m| m.contains("сопротивлению молнии")), "{:?}", i.mods);
        assert!(i.requirements.iter().any(|r| r.contains("45")), "{:?}", i.requirements);
    }

    #[test]
    fn rejects_text_that_is_not_an_item() {
        assert!(parse("привет, это не предмет").is_none());
        assert!(parse("").is_none());
        assert!(parse("Item Class: Rings\nRarity: Normal\nIron Ring\n").is_some());
    }

    #[test]
    fn analysis_rewards_resistances_and_life() {
        let i = parse(EN_RING).unwrap();
        let a = analyze(&i, 45);
        assert!(a.matches.iter().any(|m| m == "максимум жизни"));
        assert!(a.matches.iter().any(|m| m == "сопротивления"));
        assert!(a.score > 0);
    }

    #[test]
    fn analysis_warns_about_bare_armour() {
        let bare = "Item Class: Body Armours\nRarity: Rare\nWaste\nVaal Cuirass\n--------\nItem Level: 20\n--------\n+5 to Strength\n";
        let i = parse(bare).unwrap();
        let a = analyze(&i, 40);
        assert!(a.warnings.iter().any(|w| w.contains("жизн")), "{:?}", a.warnings);
        assert!(a.warnings.iter().any(|w| w.contains("сопротивл")), "{:?}", a.warnings);
    }

    #[test]
    fn waystone_dangers_are_recognised() {
        let map = "Item Class: Waystones
Rarity: Rare
Grim Path
Waystone
--------
Item Level: 79
--------
Monsters reflect 15% of Physical Damage
Players cannot Regenerate Life, Mana or Energy Shield
Monsters deal 30% extra damage as Fire
--------
";
        let i = parse(map).unwrap();
        let d = dangers(&i);
        assert!(d.iter().any(|x| x.label.contains("рефлект")), "{d:?}");
        assert!(d.iter().any(|x| x.label.contains("регенерации")), "{d:?}");
        assert!(d.iter().any(|x| x.label.contains("дополнительный")), "{d:?}");
        assert_eq!(d[0].severity, 2, "high severity first");
        // A normal ring has no map dangers.
        let ring = parse(EN_RING).unwrap();
        assert!(dangers(&ring).is_empty());
    }

    #[test]
    fn russian_waystone_dangers() {
        let map = "Класс предмета: Вайстоуны
Редкость: Редкий
Мрачный путь
Вайстоун
--------
Уровень предмета: 79
--------
Монстры отражают 15% физического урона
Игроки не могут восстанавливать здоровье
--------
";
        let i = parse(map).unwrap();
        let d = dangers(&i);
        assert!(d.iter().any(|x| x.label.contains("рефлект")), "{d:?}");
        assert!(d.iter().any(|x| x.label.contains("регенерации")), "{d:?}");
    }

    #[test]
    fn analysis_flags_unidentified_and_normal() {
        let plain = "Item Class: Rings\nRarity: Normal\nIron Ring\n--------\nItem Level: 5\n";
        let a = analyze(&parse(plain).unwrap(), 5);
        assert!(a.warnings.iter().any(|w| w.contains("обычн")), "{:?}", a.warnings);
    }
}
