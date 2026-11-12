//! Tails the game's Client.txt and turns the lines we care about into events.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[derive(Debug, PartialEq, Clone)]
pub enum Event {
    /// `[SCENE] Set Source [The Riverbank]`: display name of the zone just entered.
    Scene(String),
    /// `Generating level 14 area "G1_7" with seed ...`
    Area { id: String, level: u32 },
    /// `: Name (Class) is now level 12`
    Level(u32),
    /// `: gem_override has received +10% to [Resistances|Cold Resistance].` or
    /// `: You have received 2 Passive Skill Points.`: the text after "received".
    Reward(String),
    /// The tail switched to a (new) log file.
    Opened,
}

pub fn parse(line: &str) -> Option<Event> {
    if let Some(i) = line.find("[SCENE] Set Source [") {
        let rest = &line[i + "[SCENE] Set Source [".len()..];
        let name = rest[..rest.rfind(']')?].trim();
        // "(null)" / "(unknown)" mark loading screens and character select.
        if name.is_empty() || name.starts_with('(') {
            return None;
        }
        return Some(Event::Scene(name.to_string()));
    }
    if let Some(i) = line.find("Generating level ") {
        let rest = &line[i + "Generating level ".len()..];
        let (level, rest) = rest.split_once(" area \"")?;
        let id = &rest[..rest.find('"')?];
        return Some(Event::Area { id: id.to_string(), level: level.trim().parse().ok()? });
    }
    // System messages start with "] : ", chat lines with "] #name:" and the like.
    if let Some(msg) = line.find("] : ").map(|i| &line[i + 4..]) {
        if let Some(i) = msg.find(" has received ").map(|i| i + " has received ".len()).or_else(|| msg.strip_prefix("You have received ").map(|_| "You have received ".len())) {
            return Some(Event::Reward(msg[i..].trim().to_string()));
        }
    }
    if let Some(i) = line.find(") is now level ") {
        // ponytail: also fires for party members, add a character-name filter if that bites
        let n = &line[i + ") is now level ".len()..];
        return Some(Event::Level(n.trim().parse().ok()?));
    }
    None
}

/// Fallback until the game runs (then the log next to its exe wins): the most
/// recently written log among the usual standalone and Steam install paths.
pub fn find_client_log() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from(r"C:\Program Files (x86)\Grinding Gear Games\Path of Exile 2\logs\Client.txt")];
    for drive in ['C', 'D', 'E', 'F', 'G', 'H'] {
        for steam in [r"Program Files (x86)\Steam", "Steam", "SteamLibrary"] {
            candidates.push(format!(r"{drive}:\{steam}\steamapps\common\Path of Exile 2\logs\Client.txt").into());
        }
    }
    candidates.into_iter().filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p))).max().map(|(_, p)| p)
}

/// Follows whatever file `want` returns. On every switch it replays the latest
/// state from the end of the new file, then reads only appended lines.
pub fn spawn_tail(want: impl Fn() -> Option<PathBuf> + Send + 'static, on_event: impl Fn(Event) + Send + 'static) {
    thread::spawn(move || {
        let (mut path, mut offset, mut partial) = (None::<PathBuf>, 0, String::new());
        loop {
            let w = want();
            if w != path {
                path = w;
                partial.clear();
                if let Some(p) = &path {
                    on_event(Event::Opened);
                    offset = catch_up(p, &on_event).unwrap_or(0);
                }
            }
            if let Some(p) = &path {
                read_new(p, &mut offset, &mut partial, &on_event);
            }
            thread::sleep(Duration::from_millis(250));
        }
    });
}

fn read_new(path: &Path, offset: &mut u64, partial: &mut String, on_event: &impl Fn(Event)) -> Option<()> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < *offset {
        *offset = 0; // log was truncated or replaced
    }
    if len == *offset {
        return None;
    }
    file.seek(SeekFrom::Start(*offset)).ok()?;
    let mut buf = Vec::new();
    *offset += file.read_to_end(&mut buf).ok()? as u64;
    partial.push_str(&String::from_utf8_lossy(&buf));
    while let Some(nl) = partial.find('\n') {
        let line: String = partial.drain(..=nl).collect();
        if let Some(e) = parse(&line) {
            on_event(e);
        }
    }
    Some(())
}

/// Emits the last level, area and scene from the final 64 KB so the overlay
/// starts in the right place. Returns the file length to continue from.
fn catch_up(path: &Path, on_event: &impl Fn(Event)) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let (mut level, mut area, mut scene) = (None, None, None);
    for e in String::from_utf8_lossy(&buf).lines().filter_map(parse) {
        match e {
            Event::Level(_) => level = Some(e),
            Event::Area { .. } => area = Some(e),
            Event::Scene(_) => scene = Some(e),
            // Old rewards must not replay: the pointer already moved past them.
            Event::Reward(_) | Event::Opened => {}
        }
    }
    [level, area, scene].into_iter().flatten().for_each(on_event);
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_lines() {
        assert_eq!(
            parse("2025/10/07 19:55:37 47359781 775aed1e [INFO Client 36300] [SCENE] Set Source [The Riverbank]"),
            Some(Event::Scene("The Riverbank".into()))
        );
        assert_eq!(parse("2025/10/07 19:55:36 47358812 775aed1e [INFO Client 36300] [SCENE] Set Source [(null)]"), None);
        assert_eq!(
            parse("2025/10/07 19:55:36 47358875 2caa1a34 [DEBUG Client 36300] Generating level 1 area \"G1_1\" with seed 1608264788"),
            Some(Event::Area { id: "G1_1".into(), level: 1 })
        );
        assert_eq!(
            parse("2025/10/07 20:01:02 47400000 cff945b9 [INFO Client 36300] : sudo_zap (Sorceress) is now level 12\r\n"),
            Some(Event::Level(12))
        );
        assert_eq!(parse("2025/10/07 20:01:02 [INFO Client 36300] Connecting to instance server"), None);
        assert_eq!(
            parse("2026/10/09 11:54:02 13665296 3ef23348 [INFO Client 32720] : gem_override has received +10% to [Resistances|Cold Resistance]."),
            Some(Event::Reward("+10% to [Resistances|Cold Resistance].".into()))
        );
        assert_eq!(
            parse("2026/10/07 21:00:00 1 3ef23348 [INFO Client 31752] : You have received 2 Passive Skill Points."),
            Some(Event::Reward("2 Passive Skill Points.".into()))
        );
        assert_eq!(parse("2026/10/07 21:49:02 1 3ef23348 [INFO Client 31752] #Someone: I have received nothing"), None);
    }

    #[test]
    fn tail_follows_appended_lines_and_switches_files() {
        let tmp = |n: &str| std::env::temp_dir().join(format!("poe2overlay-test-{}-{n}.txt", std::process::id()));
        let (a, b) = (tmp("a"), tmp("b"));
        std::fs::write(&a, "x [SCENE] Set Source [Clearfell]\n").unwrap();
        std::fs::write(&b, "x [SCENE] Set Source [Ogham Village]\n").unwrap();
        let want = std::sync::Arc::new(std::sync::Mutex::new(a.clone()));
        let w = want.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_tail(move || Some(w.lock().unwrap().clone()), move |e| tx.send(e).unwrap());
        let wait = || rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(wait(), Event::Opened);
        assert_eq!(wait(), Event::Scene("Clearfell".into()));
        let mut f = std::fs::OpenOptions::new().append(true).open(&a).unwrap();
        std::io::Write::write_all(&mut f, b"x [SCENE] Set Source [The Grel").unwrap();
        std::io::Write::write_all(&mut f, b"wood]\r\n").unwrap();
        assert_eq!(wait(), Event::Scene("The Grelwood".into()));
        *want.lock().unwrap() = b.clone();
        assert_eq!(wait(), Event::Opened);
        assert_eq!(wait(), Event::Scene("Ogham Village".into()));
        let _ = (std::fs::remove_file(a), std::fs::remove_file(b));
    }
}
