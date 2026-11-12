//! Finds the game's log file: next to the running game's exe, in a Steam
//! library, in the standalone install, or wherever the user points us.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub size: u64,
}

/// File names PoE2 uses. Newer clients write `LatestClient.txt`, older ones
/// `Client.txt`; both are accepted, newest first.
pub const LOG_NAMES: [&str; 2] = ["Client.txt", "LatestClient.txt"];

/// Picks the most recently written candidate; ties prefer `Client.txt`.
pub fn pick_newest(candidates: impl IntoIterator<Item = Candidate>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .max_by_key(|c| (c.modified, c.path.ends_with("Client.txt")))
        .map(|c| c.path)
}

/// All log files of one `logs` directory.
pub fn log_files_in(dir: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();
    for name in LOG_NAMES {
        let path = dir.join(name);
        if let Ok(meta) = path.metadata() {
            if meta.is_file() {
                out.push(Candidate { path, modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), size: meta.len() });
            }
        }
    }
    out
}

fn newest_in_logs(dir: &Path) -> Option<PathBuf> {
    pick_newest(log_files_in(dir))
}

/// `<exe dir>\logs\Client.txt` (or `LatestClient.txt`) of the running game.
pub fn for_game_exe(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?.join("logs");
    newest_in_logs(&dir)
}

/// Reads `steamapps/libraryfolders.vdf` and returns every `"path"` value.
pub fn parse_vdf_paths(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for raw in text.lines() {
        // A VDF line looks like:   "path"		"D:\\SteamLibrary"
        let parts: Vec<&str> = raw.split('"').collect();
        if parts.len() < 4 {
            continue;
        }
        let (key, value) = (parts[1].trim(), parts[3].trim());
        if key.eq_ignore_ascii_case("path") && !value.is_empty() {
            out.push(PathBuf::from(value.replace("\\\\", "\\")));
        }
    }
    out
}

#[cfg(windows)]
fn registry_steam_path() -> Option<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let sub = HSTRING::from("Software\\Valve\\Steam");
    let value = HSTRING::from("SteamPath");
    let mut buf = [0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &sub,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
    };
    if status.0 != 0 {
        return None;
    }
    let chars = (len as usize / 2).saturating_sub(1).min(buf.len());
    let text = String::from_utf16_lossy(&buf[..chars]).trim_end_matches('\0').to_string();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

#[cfg(not(windows))]
fn registry_steam_path() -> Option<PathBuf> {
    None
}

/// Steam root directories: registry value, the usual install paths and the
/// libraries listed in `libraryfolders.vdf`.
pub fn steam_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(p) = registry_steam_path() {
        roots.push(p);
    }
    for drive in ['C', 'D', 'E', 'F', 'G', 'H'] {
        roots.push(PathBuf::from(format!(r"{drive}:\Program Files (x86)\Steam")));
        roots.push(PathBuf::from(format!(r"{drive}:\Steam")));
        roots.push(PathBuf::from(format!(r"{drive}:\SteamLibrary")));
    }
    let mut libraries = Vec::new();
    for root in &roots {
        for vdf in [root.join("steamapps").join("libraryfolders.vdf"), root.join("config").join("libraryfolders.vdf")] {
            if let Ok(text) = std::fs::read_to_string(vdf) {
                libraries.extend(parse_vdf_paths(&text));
            }
        }
    }
    roots.extend(libraries);
    roots.sort();
    roots.dedup();
    roots.retain(|p| p.is_dir());
    roots
}

fn standalone_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(base) = std::env::var(var) {
            dirs.push(PathBuf::from(base).join("Grinding Gear Games").join("Path of Exile 2").join("logs"));
        }
    }
    for drive in ['C', 'D', 'E', 'F'] {
        dirs.push(PathBuf::from(format!(r"{drive}:\Grinding Gear Games\Path of Exile 2\logs")));
        dirs.push(PathBuf::from(format!(r"{drive}:\Games\Path of Exile 2\logs")));
        dirs.push(PathBuf::from(format!(r"{drive}:\Path of Exile 2\logs")));
    }
    dirs
}

/// Every log the app can find without a running game.
pub fn search_candidates() -> Vec<Candidate> {
    let mut dirs = standalone_dirs();
    for root in steam_roots() {
        dirs.push(root.join("steamapps").join("common").join("Path of Exile 2").join("logs"));
    }
    let mut out = Vec::new();
    for dir in dirs {
        out.extend(log_files_in(&dir));
    }
    out
}

/// The newest log among the usual install paths, or None.
pub fn find_client_log() -> Option<PathBuf> {
    pick_newest(search_candidates())
}

/// Checks a path the user picked. Returns a readable message on failure.
pub fn validate_manual(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("Файл не найден: {}", path.display()));
    }
    if !path.is_file() {
        return Err(format!("Это не файл: {}", path.display()));
    }
    let size = path.metadata().map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(format!("Файл пуст: {}", path.display()));
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn vdf_parser_reads_library_paths() {
        let vdf = r#"
"libraryfolders"
{
    "0"
    {
        "path"		"C:\\Program Files (x86)\\Steam"
        "label"		""
    }
    "1"
    {
        "path"		"D:\\SteamLibrary"
        "apps"
        {
            "2694490"		"12345"
        }
    }
}
"#;
        let paths = parse_vdf_paths(vdf);
        assert_eq!(paths, vec![PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"D:\SteamLibrary")]);
    }

    #[test]
    fn newest_candidate_wins_and_client_txt_breaks_ties() {
        let now = SystemTime::now();
        let older = now - Duration::from_secs(60);
        let cands = vec![
            Candidate { path: PathBuf::from(r"D:\a\LatestClient.txt"), modified: now, size: 10 },
            Candidate { path: PathBuf::from(r"D:\a\Client.txt"), modified: now, size: 10 },
        ];
        assert_eq!(pick_newest(cands).unwrap(), PathBuf::from(r"D:\a\Client.txt"));

        let cands = vec![
            Candidate { path: PathBuf::from(r"D:\a\LatestClient.txt"), modified: now, size: 10 },
            Candidate { path: PathBuf::from(r"D:\a\Client.txt"), modified: older, size: 10 },
        ];
        assert_eq!(pick_newest(cands).unwrap(), PathBuf::from(r"D:\a\LatestClient.txt"));
    }

    #[test]
    fn finds_both_log_names_in_a_directory() {
        let dir = std::env::temp_dir().join(format!("eni-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Client.txt"), "a").unwrap();
        std::fs::write(dir.join("LatestClient.txt"), "b").unwrap();
        std::fs::write(dir.join("other.txt"), "c").unwrap();
        let found = log_files_in(&dir);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|c| c.path.file_name().unwrap() != "other.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn manual_path_is_validated() {
        let dir = std::env::temp_dir().join(format!("eni-discover-m-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Client.txt");
        std::fs::write(&file, "x [SCENE] Set Source [Clearfell]\n").unwrap();
        assert!(validate_manual(&file).is_ok());
        assert!(validate_manual(&dir).is_err());
        assert!(validate_manual(&dir.join("nope.txt")).is_err());
        std::fs::write(&file, "").unwrap();
        assert!(validate_manual(&file).is_err(), "empty log is rejected");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
