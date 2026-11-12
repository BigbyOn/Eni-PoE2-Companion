//! Screenshot storage: PNG encoding, preview thumbnails, SHA-256, metadata.
//!
//! Everything stays local. A screenshot is written twice: the full image and a
//! 320 px thumbnail for the gallery, so the UI never loads big files.

use crate::model::{new_id, now_iso, Shot};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Width of the gallery thumbnail.
pub const THUMB_WIDTH: u32 = 320;

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    if rgba.len() != (w as usize) * (h as usize) * 4 {
        return Err("Буфер изображения не совпадает с размером".into());
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| format!("PNG: {e}"))?;
        writer.write_image_data(rgba).map_err(|e| format!("PNG: {e}"))?;
    }
    Ok(out)
}

#[allow(dead_code)] // used by the tests and by a later item view
pub fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::ALPHA);
    let mut reader = decoder.read_info().map_err(|e| format!("PNG: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("PNG: {e}"))?;
    if info.color_type != png::ColorType::Rgba {
        return Err("Ожидался PNG в формате RGBA".into());
    }
    buf.truncate(info.buffer_size());
    Ok((buf, info.width, info.height))
}

/// Simple box filter: averages blocks of pixels, which is fine for a preview.
pub fn downscale(rgba: &[u8], w: u32, h: u32, target_w: u32) -> (Vec<u8>, u32, u32) {
    if target_w == 0 || w <= target_w {
        return (rgba.to_vec(), w, h);
    }
    let factor = (w + target_w - 1) / target_w;
    let nw = (w / factor).max(1);
    let nh = (h / factor).max(1);
    let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
    let f = factor as usize;
    let (wi, hi) = (w as usize, h as usize);
    for ny in 0..nh as usize {
        for nx in 0..nw as usize {
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for y in (ny * f)..((ny + 1) * f).min(hi) {
                for x in (nx * f)..((nx + 1) * f).min(wi) {
                    let i = (y * wi + x) * 4;
                    r += rgba[i] as u32;
                    g += rgba[i + 1] as u32;
                    b += rgba[i + 2] as u32;
                    a += rgba[i + 3] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let o = (ny * nw as usize + nx) * 4;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
            out[o + 3] = (a / n) as u8;
        }
    }
    (out, nw, nh)
}

/// Screenshot directory plus its file operations. File names are generated here
/// and re-validated on read, so a hand-edited state file cannot point outside.
#[derive(Clone, Debug)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Store { dir: dir.into() }
    }

    fn safe_name(name: &str) -> Option<&str> {
        let last = name.rsplit(['/', '\\']).next()?;
        (last == name && !name.is_empty() && !name.contains("..")).then_some(name)
    }

    pub fn save_capture(&self, rgba: &[u8], w: u32, h: u32, shot: Shot) -> Result<Shot, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("Папка скриншотов: {e}"))?;
        let full = encode_png(rgba, w, h)?;
        let (thumb, tw, th) = downscale(rgba, w, h, THUMB_WIDTH);
        let thumb_png = encode_png(&thumb, tw, th)?;

        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let file = format!("shot-{stamp}-{}.png", shot.id);
        let thumb_file = format!("shot-{stamp}-{}-thumb.png", shot.id);
        std::fs::write(self.dir.join(&file), &full).map_err(|e| format!("Сохранение скриншота: {e}"))?;
        std::fs::write(self.dir.join(&thumb_file), &thumb_png).map_err(|e| format!("Сохранение миниатюры: {e}"))?;
        Ok(Shot {
            file,
            thumb: thumb_file,
            w,
            h,
            sha256: sha256_hex(&full),
            ..shot
        })
    }

    /// A data URL for the gallery. Falls back to the full image when the
    /// thumbnail is missing (older state files).
    pub fn thumb_data_url(&self, shot: &Shot) -> Result<String, String> {
        let thumb = Self::safe_name(&shot.thumb).and_then(|n| std::fs::read(self.dir.join(n)).ok());
        let bytes = match thumb {
            Some(b) => b,
            None => {
                let name = Self::safe_name(&shot.file).ok_or("Некорректное имя файла скриншота")?;
                std::fs::read(self.dir.join(name)).map_err(|e| format!("Чтение скриншота: {e}"))?
            }
        };
        Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
    }

    /// The full PNG, for the preview window.
    pub fn full_data_url(&self, shot: &Shot) -> Result<String, String> {
        let name = Self::safe_name(&shot.file).ok_or("Некорректное имя файла скриншота")?;
        let bytes = std::fs::read(self.dir.join(name)).map_err(|e| format!("Чтение скриншота: {e}"))?;
        Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
    }

    pub fn delete(&self, shot: &Shot) {
        for name in [&shot.file, &shot.thumb] {
            if let Some(name) = Self::safe_name(name) {
                let _ = std::fs::remove_file(self.dir.join(name));
            }
        }
    }

    pub fn reveal(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("Папка скриншотов: {e}"))?;
        // explorer.exe is invoked with one fixed argument (our own directory),
        // never with user input as a command line.
        std::process::Command::new("explorer.exe")
            .arg(self.dir.as_os_str())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Не удалось открыть проводник: {e}"))
    }
}

/// Metadata for a new screenshot; the image is filled in by `save_capture`.
pub fn new_shot(profile: &str, kind: &str, zone: &str, level: u32) -> Shot {
    Shot {
        id: new_id("shot"),
        ts: now_iso(),
        profile: profile.to_string(),
        kind: kind.to_string(),
        file: String::new(),
        thumb: String::new(),
        w: 0,
        h: 0,
        sha256: String::new(),
        note: String::new(),
        zone: zone.to_string(),
        level,
    }
}

pub fn default_shots_dir() -> PathBuf {
    let base = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Pictures").join("Eni-PoE2-Companion")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 128, 255]);
            }
        }
        v
    }

    #[test]
    fn png_roundtrip() {
        let (w, h) = (7u32, 5u32);
        let rgba = sample(w, h);
        let png = encode_png(&rgba, w, h).unwrap();
        let (back, bw, bh) = decode_png(&png).unwrap();
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, rgba);
        assert!(encode_png(&rgba, w + 1, h).is_err());
    }

    #[test]
    fn thumbnails_are_smaller_and_keep_the_aspect() {
        let (w, h) = (1280u32, 720u32);
        let rgba = sample(w, h);
        let (thumb, tw, th) = downscale(&rgba, w, h, 320);
        assert_eq!(tw, 320);
        assert_eq!(th, 180);
        assert_eq!(thumb.len(), (tw * th * 4) as usize);
        // Small images are left alone.
        let (same, sw, sh) = downscale(&rgba, w, h, 2000);
        assert_eq!((sw, sh), (w, h));
        assert_eq!(same.len(), rgba.len());
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn saves_two_files_and_reports_hash() {
        let dir = std::env::temp_dir().join(format!("eni-shots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        let (w, h) = (400u32, 225u32);
        let rgba = sample(w, h);
        let shot = store.save_capture(&rgba, w, h, new_shot("p1", "item", "Clearfell", 6)).unwrap();
        assert!(dir.join(&shot.file).is_file());
        assert!(dir.join(&shot.thumb).is_file());
        assert_eq!(shot.sha256.len(), 64);
        let url = store.thumb_data_url(&shot).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        store.delete(&shot);
        assert!(!dir.join(&shot.file).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn path_traversal_is_refused() {
        assert_eq!(Store::safe_name("ok.png"), Some("ok.png"));
        assert_eq!(Store::safe_name("../../etc/passwd"), None);
        assert_eq!(Store::safe_name(r"..\..\windows\system32"), None);
        let store = Store::new(std::env::temp_dir());
        let mut shot = new_shot("p", "item", "", 1);
        shot.file = "../../secret.png".into();
        assert!(store.thumb_data_url(&shot).is_err());
    }
}
