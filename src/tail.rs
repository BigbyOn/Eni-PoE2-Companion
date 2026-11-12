//! Follows the game log: opening, catch-up, appended lines, truncation/rotation.
//!
//! `Tailer::poll` does one step and is fully testable without threads;
//! `spawn` wraps it in the usual 250 ms loop.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::logparse::{decode_line, parse, sniff, Encoding, Event};

/// How much history is replayed when the log is (re)opened.
pub const CATCH_UP_BYTES: u64 = 64 * 1024;

#[derive(Default)]
pub struct Tailer {
    path: Option<PathBuf>,
    offset: u64,
    enc: Encoding,
    partial: Vec<u8>,
}

impl Tailer {
    pub fn new() -> Self {
        Tailer::default()
    }

    #[allow(dead_code)]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// One poll. Switches to `want` when it changes, replays the recent state
    /// when a file is opened, then emits every complete line written since.
    pub fn poll(&mut self, want: Option<PathBuf>, emit: &mut impl FnMut(Event)) {
        if want != self.path {
            self.path = want;
            self.partial.clear();
            self.offset = 0;
            if let Some(p) = self.path.clone() {
                self.open(&p, emit);
            }
        }
        let Some(path) = self.path.clone() else { return };
        self.read_new(&path, emit);
    }

    fn open(&mut self, path: &Path, emit: &mut impl FnMut(Event)) {
        let Ok(mut file) = File::open(path) else { return };
        let Ok(len) = file.metadata().map(|m| m.len()) else { return };

        let mut head = vec![0u8; 4096.min(len as usize)];
        let skip = if head.is_empty() {
            0
        } else {
            let _ = file.read_exact(&mut head);
            let (enc, skip) = sniff(&head);
            self.enc = enc;
            skip
        };
        emit(Event::Opened { path: path.to_path_buf(), encoding: self.enc });

        // Replay the last known zone/area/level so a restart mid-run lands in
        // the right place. Rewards are not replayed: they are already counted.
        let start = len.saturating_sub(CATCH_UP_BYTES).max(skip as u64);
        if file.seek(SeekFrom::Start(start)).is_ok() {
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_ok() {
                if start > 0 {
                    // Skip the first, possibly partial, line.
                    if let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                        buf.drain(..=nl);
                    } else {
                        buf.clear();
                    }
                }
                let mut state: Vec<Event> = Vec::new();
                for line in split_lines(&buf, self.enc) {
                    match parse(&line) {
                        Some(e @ Event::Scene(_)) => replace_kind(&mut state, e),
                        Some(e @ Event::Area { .. }) => replace_kind(&mut state, e),
                        Some(e @ Event::Level { .. }) => replace_kind(&mut state, e),
                        _ => {}
                    }
                }
                for e in state {
                    emit(e);
                }
            }
        }
        self.offset = len;
    }

    fn read_new(&mut self, path: &Path, emit: &mut impl FnMut(Event)) {
        let Ok(mut file) = File::open(path) else { return };
        let Ok(len) = file.metadata().map(|m| m.len()) else { return };
        if len < self.offset {
            // The game truncated or replaced the file.
            emit(Event::Truncated);
            self.partial.clear();
            self.offset = 0;
            if len > CATCH_UP_BYTES * 16 {
                // Rotated file with a lot of old content: replay the tail only.
                self.open(path, emit);
                return;
            }
        }
        if len == self.offset {
            return;
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            return;
        }
        self.offset += buf.len() as u64;
        self.partial.extend_from_slice(&buf);
        while let Some(nl) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=nl).collect();
            let text = decode_line(&line, self.enc);
            if let Some(e) = parse(&text) {
                emit(e);
            }
        }
        // Keep a runaway partial line (binary garbage) from growing forever.
        if self.partial.len() > 1 << 20 {
            self.partial.clear();
        }
    }
}

fn split_lines(buf: &[u8], enc: Encoding) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in buf.iter().enumerate() {
        if *b == b'\n' {
            out.push(decode_line(&buf[start..i], enc));
            start = i + 1;
        }
    }
    if start < buf.len() {
        out.push(decode_line(&buf[start..], enc));
    }
    out
}

fn replace_kind(state: &mut Vec<Event>, e: Event) {
    let idx = state.iter().position(|o| std::mem::discriminant(o) == std::mem::discriminant(&e));
    match idx {
        Some(i) => state[i] = e,
        None => state.push(e),
    }
}

/// Follows whatever file `want` returns, forever, on a background thread.
pub fn spawn(want: impl Fn() -> Option<PathBuf> + Send + 'static, on_event: impl Fn(Event) + Send + 'static) {
    thread::spawn(move || {
        let mut tail = Tailer::new();
        loop {
            tail.poll(want(), &mut |e| on_event(e));
            thread::sleep(Duration::from_millis(250));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eni-tail-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("Client.txt")
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn poll(t: &mut Tailer, want: PathBuf) -> Vec<Event> {
        let mut out = Vec::new();
        t.poll(Some(want), &mut |e| out.push(e));
        out
    }

    #[test]
    fn replays_state_then_follows_appends_and_partial_lines() {
        let path = tmp("follow");
        append(&path, "[INFO Client 1] [SCENE] Set Source [Clearfell]\n");
        append(&path, "[INFO Client 1] : Eni (Ranger) is now level 6\n");
        let mut t = Tailer::new();
        let events = poll(&mut t, path.clone());
        assert!(matches!(events[0], Event::Opened { .. }), "opening is announced first");
        assert!(events.contains(&Event::Scene("Clearfell".into())));
        assert!(events.contains(&Event::Level { name: Some("Eni".into()), class: Some("Ranger".into()), level: 6 }));

        assert!(poll(&mut t, path.clone()).is_empty(), "nothing new, nothing emitted");

        // A line split across two writes must surface only when it is complete.
        append(&path, "[INFO Client 1] [SCENE] Set Source [The Grel");
        assert!(poll(&mut t, path.clone()).is_empty());
        append(&path, "wood]\r\n");
        assert_eq!(poll(&mut t, path.clone()), vec![Event::Scene("The Grelwood".into())]);

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn reports_truncation_and_starts_over() {
        let path = tmp("truncate");
        let mut t = Tailer::new();
        append(&path, "[INFO Client 1] [SCENE] Set Source [Clearfell]\n");
        poll(&mut t, path.clone());

        std::fs::write(&path, "").unwrap();
        let events = poll(&mut t, path.clone());
        assert_eq!(events, vec![Event::Truncated]);

        append(&path, "[INFO Client 1] [SCENE] Set Source [Ogham Village]\n");
        assert_eq!(poll(&mut t, path.clone()), vec![Event::Scene("Ogham Village".into())]);

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn switches_between_client_and_latest_client() {
        let a = tmp("switch-a");
        let b = a.with_file_name("LatestClient.txt");
        append(&a, "[INFO Client 1] [SCENE] Set Source [Clearfell]\n");
        append(&b, "[INFO Client 1] [SCENE] Set Source [The Riverbank]\n");
        let mut t = Tailer::new();
        assert!(poll(&mut t, a.clone()).contains(&Event::Scene("Clearfell".into())));
        let events = poll(&mut t, b.clone());
        assert!(matches!(events[0], Event::Opened { .. }));
        assert_eq!(events.last(), Some(&Event::Scene("The Riverbank".into())));
        assert_eq!(t.path(), Some(b.as_path()));

        std::fs::remove_dir_all(a.parent().unwrap()).unwrap();
    }

    #[test]
    fn reads_utf16le_logs() {
        let path = tmp("utf16");
        let text = "[INFO Client 1] [SCENE] Set Source [Берег реки]\r\n";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        std::fs::write(&path, &bytes).unwrap();
        let mut t = Tailer::new();
        let events = poll(&mut t, path.clone());
        assert!(matches!(events[0], Event::Opened { encoding: Encoding::Utf16Le, .. }));
        assert_eq!(events.last(), Some(&Event::Scene("Берег реки".into())));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn skips_a_missing_file_without_panicking() {
        let mut t = Tailer::new();
        let mut out = Vec::new();
        for _ in 0..3 {
            t.poll(Some(PathBuf::from(r"Z:\definitely\not\here\Client.txt")), &mut |e| out.push(e));
        }
        assert!(out.is_empty());
    }
}
