//! Preparation for a future AI integration.
//!
//! v0.1 ships **no network client**: `ProviderInfo::available` is false for every
//! provider, nothing leaves the machine and no key is required. What exists is
//! the seam a provider will plug into:
//!
//! * a provider registry (OpenAI, DeepSeek) with models and key links,
//! * a request description that names the task and the local files it may use,
//! * DPAPI-protected key storage (Windows `CryptProtectData`), so a key can be
//!   saved for a later version without ever sitting in plain text in JSON.

use crate::builds::BuildDoc;
use crate::model::{Profile, Shot};
use base64::Engine;
use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub default_model: &'static str,
    pub models: &'static [&'static str],
    pub key_url: &'static str,
    /// False in v0.1: the interface exists, the network call does not.
    pub available: bool,
}

pub const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: "openai",
        label: "OpenAI",
        default_model: "gpt-4o-mini",
        models: &["gpt-4o-mini", "gpt-4o", "gpt-4.1-mini"],
        key_url: "https://platform.openai.com/api-keys",
        available: false,
    },
    ProviderInfo {
        id: "deepseek",
        label: "DeepSeek",
        default_model: "deepseek-chat",
        models: &["deepseek-chat", "deepseek-reasoner"],
        key_url: "https://platform.deepseek.com/api_keys",
        available: false,
    },
];

pub fn provider(id: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// What the player wants to know. The enum is the contract for a future client.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    AnalyseItem,
    CompareItems,
    ReviewPassives,
    CompareToBuild,
    NextUpgrades,
    Answer,
}

impl Task {
    pub fn as_str(self) -> &'static str {
        match self {
            Task::AnalyseItem => "analyse_item",
            Task::CompareItems => "compare_items",
            Task::ReviewPassives => "review_passives",
            Task::CompareToBuild => "compare_to_build",
            Task::NextUpgrades => "next_upgrades",
            Task::Answer => "answer",
        }
    }
}

/// A local, self-contained description of a request. No key, no endpoint, no
/// image bytes: a future provider implementation adds those itself.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Request {
    pub task: String,
    pub provider: String,
    pub model: String,
    /// Local file names (inside the screenshots directory) the provider may read.
    pub images: Vec<ImageRef>,
    pub context: Context,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ImageRef {
    pub file: String,
    pub kind: String,
    pub w: u32,
    pub h: u32,
    pub sha256: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Context {
    pub character: String,
    pub class: String,
    pub ascendancy: String,
    pub level: u32,
    pub act: u32,
    pub zone: String,
    pub build_id: String,
    pub build_name: String,
    pub build_stage: String,
    pub build_patch: String,
}

/// Builds the request description for one of the future tasks. Pure function,
/// only reads data that is already on disk.
pub fn prepare(task: Task, provider_id: &str, model: &str, shots: &[Shot], profile: &Profile, build: &BuildDoc, lang: &str) -> Request {
    let provider = provider(provider_id);
    let model = if model.trim().is_empty() {
        provider.map(|p| p.default_model).unwrap_or("").to_string()
    } else {
        model.to_string()
    };
    let stage = build.stage_for(profile.level);
    Request {
        task: task.as_str().to_string(),
        provider: provider_id.to_string(),
        model,
        images: shots
            .iter()
            .map(|s| ImageRef { file: s.file.clone(), kind: s.kind.clone(), w: s.w, h: s.h, sha256: s.sha256.clone() })
            .collect(),
        context: Context {
            character: profile.name.clone(),
            class: profile.class.clone(),
            ascendancy: profile.ascendancy.clone(),
            level: profile.level,
            act: profile.act,
            zone: profile.zone.clone(),
            build_id: build.id.clone(),
            build_name: build.name.get(lang).to_string(),
            build_stage: stage.title.get(lang).to_string(),
            build_patch: build.patch.clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// DPAPI key storage
// ---------------------------------------------------------------------------

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;
/// Binds the encrypted blob to this application.
const ENTROPY: &[u8] = b"Eni-PoE2-Companion/key/v1";

#[cfg(windows)]
mod dpapi {
    use super::*;

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            data_in: *const DataBlob,
            description: *const u16,
            entropy: *const DataBlob,
            reserved: *mut std::ffi::c_void,
            prompt: *mut std::ffi::c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
        #[allow(dead_code)]
        fn CryptUnprotectData(
            data_in: *const DataBlob,
            description: *mut *mut u16,
            entropy: *const DataBlob,
            reserved: *mut std::ffi::c_void,
            prompt: *mut std::ffi::c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    fn protect(secret: &str) -> Result<Vec<u8>, String> {
        let mut input = secret.as_bytes().to_vec();
        let mut entropy = ENTROPY.to_vec();
        let (pin, pent) = (
            DataBlob { cb_data: input.len() as u32, pb_data: input.as_mut_ptr() },
            DataBlob { cb_data: entropy.len() as u32, pb_data: entropy.as_mut_ptr() },
        );
        let mut out = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        let ok = unsafe {
            CryptProtectData(&pin, std::ptr::null(), &pent, std::ptr::null_mut(), std::ptr::null_mut(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err(format!("DPAPI: не удалось зашифровать ключ (код {})", std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)));
        }
        let bytes = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
        unsafe { LocalFree(out.pb_data as *mut _) };
        Ok(bytes)
    }

    #[allow(dead_code)]
    fn unprotect(blob: &[u8]) -> Result<String, String> {
        let mut input = blob.to_vec();
        let mut entropy = ENTROPY.to_vec();
        let (pin, pent) = (
            DataBlob { cb_data: input.len() as u32, pb_data: input.as_mut_ptr() },
            DataBlob { cb_data: entropy.len() as u32, pb_data: entropy.as_mut_ptr() },
        );
        let mut out = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(&pin, std::ptr::null_mut(), &pent, std::ptr::null_mut(), std::ptr::null_mut(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err("DPAPI: ключ не удалось расшифровать (другой пользователь Windows?)".into());
        }
        let bytes = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
        unsafe { LocalFree(out.pb_data as *mut _) };
        String::from_utf8(bytes).map_err(|_| "DPAPI: ключ повреждён".to_string())
    }

    pub fn store(secret: &str) -> Result<String, String> {
        if secret.trim().is_empty() {
            return Err("Пустой ключ".into());
        }
        Ok(base64::engine::general_purpose::STANDARD.encode(protect(secret)?))
    }

    #[allow(dead_code)]
    pub fn load(blob: &str) -> Result<String, String> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(blob).map_err(|_| "DPAPI: повреждённая строка ключа".to_string())?;
        unprotect(&bytes)
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn store(_secret: &str) -> Result<String, String> {
        Err("Безопасное хранение ключа реализовано только для Windows".into())
    }
    pub fn load(_blob: &str) -> Result<String, String> {
        Err("Безопасное хранение ключа реализовано только для Windows".into())
    }
}

/// Encrypts a key with the Windows user profile and returns base64 for state.json.
pub fn protect_key(secret: &str) -> Result<String, String> {
    dpapi::store(secret)
}

#[allow(dead_code)] // a later AI client decrypts the stored key
pub fn unprotect_key(blob: &str) -> Result<String, String> {
    dpapi::load(blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile { name: "Эни".into(), level: 6, ..Default::default() }
    }

    fn shot() -> Shot {
        Shot {
            id: "s1".into(),
            ts: "2026-11-12T20:00:00+03:00".into(),
            profile: "p".into(),
            kind: "item".into(),
            file: "shot-1.png".into(),
            thumb: "shot-1-thumb.png".into(),
            w: 100,
            h: 80,
            sha256: "ab".repeat(32),
            note: String::new(),
            zone: "Clearfell".into(),
            level: 6,
        }
    }

    #[test]
    fn registry_has_openai_and_deepseek_but_is_offline() {
        assert!(provider("openai").is_some());
        assert!(provider("deepseek").is_some());
        assert!(provider("nope").is_none());
        assert!(PROVIDERS.iter().all(|p| !p.available), "v0.1 must not talk to a network");
        assert!(PROVIDERS.iter().all(|p| p.key_url.starts_with("https://")));
    }

    #[test]
    fn prepared_request_has_no_secret_and_no_image_bytes() {
        let build = BuildDoc::parse(crate::builds::BUNDLED).unwrap();
        let req = prepare(Task::AnalyseItem, "deepseek", "", &[shot()], &profile(), &build, "ru");
        assert_eq!(req.task, "analyse_item");
        assert_eq!(req.model, "deepseek-chat", "default model from the registry");
        assert_eq!(req.images.len(), 1);
        assert_eq!(req.images[0].file, "shot-1.png");
        assert_eq!(req.context.level, 6);
        assert_eq!(req.context.build_stage, "6–14 уровень");
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("api_key"));
        assert!(!json.contains("data:image"), "no image payload in the description");
    }

    #[test]
    fn key_is_encrypted_and_roundtrips() {
        let secret = "sk-test-0123456789";
        let blob = protect_key(secret).expect("DPAPI protect");
        assert_ne!(blob, secret);
        assert!(!blob.contains("sk-test"));
        assert_eq!(unprotect_key(&blob).unwrap(), secret);
        assert!(protect_key("").is_err());
        assert!(unprotect_key("not-base64").is_err());
    }
}

