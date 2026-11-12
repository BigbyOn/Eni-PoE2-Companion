//! Turns `Client.txt` / `LatestClient.txt` lines into events.
//!
//! The game writes English engine lines even for localized clients, but a
//! localized client can translate the human-readable parts (zone names, reward
//! sentences) and some builds write UTF-16. Both are handled here:
//!
//! * engine tags are matched case-insensitively,
//! * the area id is read from the quoted string (language independent),
//! * level-up and reward lines accept English and Russian wording,
//! * UTF-8, UTF-8 with BOM and UTF-16LE files are all decoded.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 BOM",
            Encoding::Utf16Le => "UTF-16LE",
        }
    }
}

impl Default for Encoding {
    fn default() -> Self {
        Encoding::Utf8
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The tail switched to this file: the game was restarted or the log rotated.
    Opened { path: PathBuf, encoding: Encoding },
    /// `[SCENE] Set Source [The Riverbank]`
    Scene(String),
    /// `Generating level 14 area "G1_7" with seed 123`
    Area { id: String, level: u32 },
    /// `: Name (Class) is now level 12`
    Level { name: Option<String>, class: Option<String>, level: u32 },
    /// `: Name has received +10% to [Resistances|Cold Resistance].`
    Reward(String),
    /// The file got shorter: it was truncated or replaced.
    Truncated,
}

/// Rewards in the log start with one of these after the `] : ` prefix.
const REWARD_MARKERS: &[&str] = &[
    " has received ",
    " has been awarded ",
    "You have received ",
    "Вы получили ",
    " получил ",
    " получила ",
    " получили ",
    " получено ",
    "Получено ",
];

const LEVEL_MARKERS: &[&str] = &[
    " is now level ",
    " now level ",
    // Russian client: `: Name (Class) достигает 2 уровня`
    " достигает уровня ",
    " достигает ",
    "теперь уровень ",
    "теперь уровня ",
    "достиг уровня ",
    "достигла уровня ",
    "получил уровень ",
    "получила уровень ",
];

/// Case-insensitive `find` that stays safe for UTF-16-free, ASCII markers.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    if let Some(i) = hay.find(needle) {
        return Some(i);
    }
    let lower = hay.to_lowercase();
    // Only reuse the offset when lowercasing kept the byte length (ASCII markers).
    if lower.len() != hay.len() {
        return None;
    }
    lower.find(&needle.to_lowercase())
}

fn digits(s: &str) -> Option<(u32, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some((s[..end].parse().ok()?, &s[end..]))
}

/// `: Name (Class) is now level 12` -> name, class and level.
fn split_level(prefix: &str) -> (Option<String>, Option<String>) {
    let prefix = prefix.trim().trim_end_matches(')');
    let Some(open) = prefix.rfind('(') else {
        let name = prefix.trim_start_matches([':', ' ']).trim();
        return (clean_name(name), None);
    };
    let class = prefix[open + 1..].trim();
    let name = prefix[..open].trim_start_matches([':', ' ']).trim();
    (clean_name(name), clean_name(class))
}

fn clean_name(s: &str) -> Option<String> {
    let s = s.trim().trim_matches('"').trim();
    // Party chat and system noise are not a character name.
    if s.is_empty() || s.starts_with('#') || s.contains('[') || s.len() > 48 {
        return None;
    }
    Some(s.to_string())
}

/// The client also reports screens that are not zones ("Акт 1", "Act 1").
fn is_pseudo_zone(name: &str) -> bool {
    let lower = name.trim().to_lowercase();
    let rest = lower.strip_prefix("акт ").or_else(|| lower.strip_prefix("act ")).unwrap_or("");
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || "ivx".contains(c))
}

pub fn parse(line: &str) -> Option<Event> {
    let line = line.trim_end_matches(['\r', '\n']);

    // 1. Zone name in brackets behind the engine tag.
    if let Some(tag) = find_ci(line, "[SCENE]") {
        let rest = &line[tag + "[SCENE]".len()..];
        if let (Some(open), Some(close)) = (rest.find('['), rest.rfind(']')) {
            if close > open + 1 {
                let name = rest[open + 1..close].trim();
                // "(null)" marks a loading screen / character select.
                if !name.is_empty() && !name.starts_with('(') && !is_pseudo_zone(name) {
                    return Some(Event::Scene(name.to_string()));
                }
            }
        }
        return None;
    }

    // 2. Area id and area level. The id is always double quoted, so this works
    //    for every client language.
    for marker in ["Generating level ", "Генерация уровня "] {
        if let Some(i) = find_ci(line, marker) {
            let (level, rest) = digits(&line[i + marker.len()..])?;
            let open = rest.find('"')?;
            let after = &rest[open + 1..];
            let close = after.find('"')?;
            let id = after[..close].trim();
            if !id.is_empty() {
                return Some(Event::Area { id: id.to_string(), level });
            }
        }
    }

    // System messages: everything behind `] : `. Chat uses `] #name:`, so it
    // cannot be mistaken for a level-up or a reward.
    let msg = line.find("] : ").map(|i| &line[i + 4..]);

    // 3. Level up. Both the name and the class in parentheses are required:
    //    without them a party line like "is now level 33" would overwrite the
    //    tracked character's level.
    for marker in LEVEL_MARKERS {
        let Some(i) = msg.and_then(|m| find_ci(m, marker)) else { continue };
        let text = msg.unwrap_or(line);
        let Some((level, _)) = digits(&text[i + marker.len()..]) else { continue };
        let (name, class) = split_level(&text[..i]);
        if let (Some(name), Some(class)) = (name, class) {
            return Some(Event::Level { name: Some(name), class: Some(class), level });
        }
    }

    // 4. Permanent rewards such as resistances, spirit or passive points.
    if let Some(msg) = msg {
        // Chat channels (#trade, @whisper, %party, $global) are not system lines.
        if msg.trim_start().starts_with(['#', '@', '%', '$', '&']) {
            return None;
        }
        for marker in REWARD_MARKERS {
            if let Some(j) = find_ci(msg, marker) {
                let text = msg[j + marker.len()..].trim();
                if !text.is_empty() {
                    return Some(Event::Reward(text.to_string()));
                }
            }
        }
    }

    None
}

/// Detects the encoding of the first bytes and how many bytes to skip.
pub fn sniff(bytes: &[u8]) -> (Encoding, usize) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (Encoding::Utf8Bom, 3);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return (Encoding::Utf16Le, 0);
    }
    // UTF-16LE without BOM: every second byte of ASCII text is zero.
    let probe = &bytes[..bytes.len().min(512)];
    if probe.len() >= 8 {
        let odd_zeros = probe.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
        let pairs = probe.len() / 2;
        if pairs > 0 && odd_zeros * 10 > pairs * 3 {
            return (Encoding::Utf16Le, 0);
        }
    }
    (Encoding::Utf8, 0)
}

/// Decodes one complete line (without its newline) in the file's encoding.
pub fn decode_line(bytes: &[u8], enc: Encoding) -> String {
    match enc {
        Encoding::Utf16Le => {
            let mut u16s = Vec::with_capacity(bytes.len() / 2);
            for chunk in bytes.chunks_exact(2) {
                u16s.push(u16::from_le_bytes([chunk[0], chunk[1]]));
            }
            String::from_utf16_lossy(&u16s)
        }
        Encoding::Utf8Bom if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) => {
            String::from_utf8_lossy(&bytes[3..]).into_owned()
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_english_client_lines() {
        assert_eq!(
            parse("2025/10/07 19:55:37 47359781 775aed1e [INFO Client 36300] [SCENE] Set Source [The Riverbank]"),
            Some(Event::Scene("The Riverbank".into()))
        );
        assert_eq!(
            parse("2025/10/07 19:55:36 47358812 775aed1e [INFO Client 36300] [SCENE] Set Source [(null)]"),
            None
        );
        assert_eq!(
            parse("2025/10/07 19:55:36 47358875 2caa1a34 [DEBUG Client 36300] Generating level 1 area \"G1_1\" with seed 1608264788"),
            Some(Event::Area { id: "G1_1".into(), level: 1 })
        );
        assert_eq!(
            parse("2025/10/07 20:01:02 47400000 cff945b9 [INFO Client 36300] : sudo_zap (Sorceress) is now level 12\r\n"),
            Some(Event::Level { name: Some("sudo_zap".into()), class: Some("Sorceress".into()), level: 12 })
        );
        assert_eq!(
            parse("2026/10/09 11:54:02 13665296 3ef23348 [INFO Client 32720] : gem_override has received +10% to [Resistances|Cold Resistance]."),
            Some(Event::Reward("+10% to [Resistances|Cold Resistance].".into()))
        );
        assert_eq!(
            parse("2026/10/07 21:00:00 1 3ef23348 [INFO Client 31752] : You have received 2 Passive Skill Points."),
            Some(Event::Reward("2 Passive Skill Points.".into()))
        );
        assert_eq!(parse("2026/10/07 21:49:02 1 3ef23348 [INFO Client 31752] #Someone: I have received nothing"), None);
        assert_eq!(parse("2025/10/07 20:01:02 [INFO Client 36300] Connecting to instance server"), None);
    }

    #[test]
    fn parses_russian_client_lines() {
        // Formats taken from a real Russian client log (character name replaced).
        assert_eq!(
            parse("2026/11/12 20:00:00 1 aabbccdd [INFO Client 1234] [SCENE] Set Source [Берег реки]"),
            Some(Event::Scene("Берег реки".into()))
        );
        assert_eq!(
            parse("2026/11/12 20:05:00 1 aabbccdd [INFO Client 1234] : Эни (Охотница) достигает 7 уровня"),
            Some(Event::Level { name: Some("Эни".into()), class: Some("Охотница".into()), level: 7 })
        );
        assert_eq!(
            parse("2026/11/12 20:06:00 1 aabbccdd [INFO Client 1234] : Игрок Эни получил +10% к сопротивлению [Resistances|холоду]."),
            Some(Event::Reward("+10% к сопротивлению [Resistances|холоду].".into()))
        );
        assert_eq!(
            parse("2026/11/12 20:07:00 1 aabbccdd [INFO Client 1234] : Вы получили 2 очка пассивных умений."),
            Some(Event::Reward("2 очка пассивных умений.".into()))
        );
        assert_eq!(
            parse("2026/11/12 20:08:00 1 aabbccdd [INFO Client 1234] Generating level 12 area \"G1_9\" with seed 5"),
            Some(Event::Area { id: "G1_9".into(), level: 12 })
        );
        // The act screen is not a zone.
        assert_eq!(parse("2026/11/12 20:09:00 1 aabbccdd [INFO Client 1234] [SCENE] Set Source [Акт 1]"), None);
        assert_eq!(parse("2026/11/12 20:09:01 1 aabbccdd [INFO Client 1234] [SCENE] Set Source [(unknown)]"), None);
    }

    #[test]
    fn detects_encodings_and_decodes() {
        let utf8 = "привет".as_bytes();
        assert_eq!(sniff(utf8).0, Encoding::Utf8);

        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(utf8);
        let (enc, skip) = sniff(&bom);
        assert_eq!((enc, skip), (Encoding::Utf8Bom, 3));
        assert_eq!(decode_line(&bom[skip..], enc), "привет");

        // UTF-16LE without BOM is only guessed from a largely ASCII file; real
        // logs start with a timestamp, so the guess holds.
        let utf16: Vec<u8> = "[INFO Client 1] [SCENE] привет".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let (enc, skip) = sniff(&utf16);
        assert_eq!((enc, skip), (Encoding::Utf16Le, 0));
        assert!(decode_line(&utf16[skip..], enc).contains("привет"));

        // A byte that is not valid UTF-8 must not panic or swallow the line.
        let broken = [b'a', 0xFF, b'b'];
        assert!(decode_line(&broken, Encoding::Utf8).starts_with('a'));
    }

    #[test]
    fn ignores_chat_and_garbage() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("[INFO Client 1] : [Party] Someone (Witch) is now level 9"), None);
        assert_eq!(parse("[INFO Client 1] : #trade Someone has received a thing"), None);
        // Without a character name a level-up is ignored: it could be a party member.
        assert_eq!(parse("[INFO Client 1] : is now level 33"), None);
    }
}
