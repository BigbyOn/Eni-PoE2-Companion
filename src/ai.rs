//! AI integration: DeepSeek and OpenAI (both OpenAI-compatible chat APIs).
//!
//! Rules this module keeps:
//! * nothing is sent unless the user presses a button (no background calls);
//! * only HTTPS and only the two endpoints of the registry, redirects disabled;
//! * the API key lives DPAPI-encrypted in `state.json` and is only ever put into
//!   the `Authorization` header — never into a message, a log or an error string;
//! * images are attached only when the chosen model can actually see them
//!   (OpenAI vision models); text-only models get the note and the build context;
//! * the request description is written next to the screenshot first, so the
//!   user can see exactly what would be sent.

use crate::builds::BuildDoc;
use crate::model::{AiCfg, Profile, Shot};
use base64::Engine;
use serde::Serialize;
use std::time::Duration;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub default_model: &'static str,
    pub models: &'static [&'static str],
    pub key_url: &'static str,
    /// A provider is usable once a key is stored.
    pub available: bool,
    /// Chat completion endpoint (https only, fixed per provider).
    pub endpoint: &'static str,
    /// Supports `reasoning_effort` in the request body.
    pub reasoning: bool,
    /// At least one model of the provider can read images.
    pub vision: bool,
}

pub const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: "deepseek",
        label: "DeepSeek",
        default_model: "deepseek-chat",
        models: &["deepseek-chat", "deepseek-reasoner"],
        key_url: "https://platform.deepseek.com/api_keys",
        available: true,
        endpoint: "https://api.deepseek.com/chat/completions",
        reasoning: false,
        vision: false,
    },
    ProviderInfo {
        id: "openai",
        label: "OpenAI",
        default_model: "gpt-4o-mini",
        models: &["gpt-4o-mini", "gpt-4o", "gpt-4.1-mini", "gpt-4.1", "o4-mini"],
        key_url: "https://platform.openai.com/api-keys",
        available: true,
        endpoint: "https://api.openai.com/v1/chat/completions",
        reasoning: true,
        vision: true,
    },
];

pub fn provider(id: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// OpenAI vision models by id prefix. Anything unknown is treated as text-only,
/// so an image is never silently sent to a model that cannot read it.
pub fn model_supports_vision(provider_id: &str, model: &str) -> bool {
    let m = model.trim().to_lowercase();
    provider_id == "openai"
        && (m.starts_with("gpt-4o")
            || m.starts_with("gpt-4.1")
            || m.starts_with("gpt-5")
            || m.starts_with("o3")
            || m.starts_with("o4")
            || m.starts_with("chatgpt-4o"))
}

pub fn endpoint(provider_id: &str) -> Option<&'static str> {
    provider(provider_id).map(|p| p.endpoint)
}

// ---------------------------------------------------------------------------
// Request description (written to disk before anything is sent)
// ---------------------------------------------------------------------------

/// What the player wants to know.
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

    pub fn parse(s: &str) -> Task {
        match s {
            "compare_items" => Task::CompareItems,
            "review_passives" => Task::ReviewPassives,
            "compare_to_build" => Task::CompareToBuild,
            "next_upgrades" => Task::NextUpgrades,
            "answer" => Task::Answer,
            _ => Task::AnalyseItem,
        }
    }
}

/// A local, self-contained description of a request: no key, no image bytes.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Request {
    pub task: String,
    pub provider: String,
    pub model: String,
    pub reasoning: String,
    /// Local file names (inside the screenshots directory) the request refers to.
    pub images: Vec<ImageRef>,
    pub question: String,
    /// Item text copied from the game (Ctrl+C), if the request is about an item.
    #[serde(default)]
    pub item_text: Option<String>,
    pub context: Context,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ImageRef {
    pub file: String,
    pub kind: String,
    pub w: u32,
    pub h: u32,
    pub sha256: String,
    pub note: String,
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
    pub current_goal: String,
    pub next_steps: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn prepare(
    task: Task,
    cfg: &AiCfg,
    shots: &[Shot],
    profile: &Profile,
    build: &BuildDoc,
    lang: &str,
    question: &str,
    goal: &str,
    next: &[String],
    item_text: Option<&str>,
) -> Request {
    let provider_id = if provider(&cfg.provider).is_some() { cfg.provider.clone() } else { "deepseek".to_string() };
    let p = provider(&provider_id).expect("checked above");
    let model = if cfg.model.trim().is_empty() { p.default_model.to_string() } else { cfg.model.trim().to_string() };
    let stage = build.stage_for(profile.level);
    Request {
        task: task.as_str().to_string(),
        provider: provider_id,
        model,
        reasoning: normalize_reasoning(&cfg.reasoning).to_string(),
        images: shots
            .iter()
            .map(|s| ImageRef { file: s.file.clone(), kind: s.kind.clone(), w: s.w, h: s.h, sha256: s.sha256.clone(), note: s.note.clone() })
            .collect(),
        question: question.chars().take(1000).collect(),
        item_text: item_text.map(|t| t.chars().take(4000).collect()),
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
            current_goal: goal.to_string(),
            next_steps: next.to_vec(),
        },
    }
}

// ---------------------------------------------------------------------------
// Prompt and request body
// ---------------------------------------------------------------------------

pub fn normalize_reasoning(value: &str) -> &'static str {
    match value.trim().to_lowercase().as_str() {
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        _ => "auto",
    }
}

pub fn system_prompt(lang: &str) -> String {
    if lang == "en" {
        "You are Eni, a Path of Exile 2 companion. Answer in English, briefly and concretely. \
         Use only the supplied character, build and screenshot data; if something is not visible, \
         say which data is missing instead of inventing items or stats. Never give instructions that \
         automate gameplay. Format with short lines, no markdown tables."
            .to_string()
    } else {
        "Ты Eni — помощник по Path of Exile 2. Отвечай по-русски, коротко и по делу. \
         Опирайся только на переданные данные персонажа, билда и скриншота; если чего-то не видно, \
         скажи, каких данных не хватает, и не выдумывай предметы и характеристики. Не давай инструкций \
         по автоматизации игры. Пиши короткими строками, без markdown-таблиц."
            .to_string()
    }
}

fn task_instruction(task: Task, lang: &str) -> &'static str {
    match (task, lang == "en") {
        (Task::AnalyseItem, false) => "Разбери предмет на скриншоте: что это, ключевые модификаторы, для кого полезен и стоит ли менять текущую экипировку под этот билд.",
        (Task::AnalyseItem, true) => "Analyse the item in the screenshot: what it is, its key modifiers, who it suits and whether it should replace current gear for this build.",
        (Task::CompareItems, false) => "Сравни предметы на скриншотах: какой лучше для этого билда и почему.",
        (Task::CompareItems, true) => "Compare the items in the screenshots: which one is better for this build and why.",
        (Task::ReviewPassives, false) => "Посмотри на дерево навыков: какие узлы взять дальше и что перераспределить.",
        (Task::ReviewPassives, true) => "Review the passive tree: which nodes to take next and what to respec.",
        (Task::CompareToBuild, false) => "Сопоставь персонажа с выбранным билдом: чего не хватает на текущем этапе.",
        (Task::CompareToBuild, true) => "Compare the character with the chosen build: what is missing at the current stage.",
        (Task::NextUpgrades, false) => "Предложи 3–5 следующих улучшений: экипировка, камни, пассивные узлы. По приоритету.",
        (Task::NextUpgrades, true) => "Suggest 3–5 next upgrades: gear, gems, passive nodes. In priority order.",
        (Task::Answer, false) => "Ответь на вопрос игрока по прохождению, опираясь на контекст.",
        (Task::Answer, true) => "Answer the player's question about progression using the context.",
    }
}

pub fn build_prompt(req: &Request, lang: &str, vision: bool) -> String {
    let c = &req.context;
    let mut out = String::new();
    if lang == "en" {
        out.push_str(&format!("Character: {} ({}, {}), level {}, act {}.\n", c.character, c.class, c.ascendancy, c.level, c.act));
        out.push_str(&format!("Zone: {}.\nBuild: {} ({}), patch {}.\nStage: {}.\n", c.zone, c.build_name, c.build_id, c.build_patch, c.build_stage));
        out.push_str(&format!("Current campaign goal: {}.\n", c.current_goal));
        if !c.next_steps.is_empty() {
            out.push_str(&format!("Next steps: {}.\n", c.next_steps.join("; ")));
        }
    } else {
        out.push_str(&format!("Персонаж: {} ({}, {}), уровень {}, акт {}.\n", c.character, c.class, c.ascendancy, c.level, c.act));
        out.push_str(&format!("Локация: {}.\nБилд: {} ({}), патч {}.\nЭтап: {}.\n", c.zone, c.build_name, c.build_id, c.build_patch, c.build_stage));
        out.push_str(&format!("Текущая цель кампании: {}.\n", c.current_goal));
        if !c.next_steps.is_empty() {
            out.push_str(&format!("Следующие шаги: {}.\n", c.next_steps.join("; ")));
        }
    }
    if let Some(item) = req.item_text.as_deref().filter(|t| !t.trim().is_empty()) {
        out.push_str(if lang == "en" { "Item copied from the game:\n" } else { "Предмет, скопированный из игры:\n" });
        out.push_str(item.trim());
        out.push('\n');
    }
    if !req.images.is_empty() {
        let kinds: Vec<String> = req.images.iter().map(|i| format!("{} ({})", i.kind, i.file)).collect();
        out.push_str(&format!(
            "{}: {}\n",
            if lang == "en" { "Screenshots" } else { "Скриншоты" },
            kinds.join(", ")
        ));
        if !vision {
            out.push_str(if lang == "en" {
                "The chosen model cannot read images, so the picture itself is not sent.\n"
            } else {
                "Выбранная модель не читает изображения, поэтому сам снимок не отправляется.\n"
            });
        }
        if let Some(note) = req.images.iter().map(|i| i.note.trim()).find(|n| !n.is_empty()) {
            out.push_str(&format!("{}: {note}\n", if lang == "en" { "Player note" } else { "Заметка игрока" }));
        }
    }
    out.push('\n');
    out.push_str(task_instruction(Task::parse(&req.task), lang));
    out.push('\n');
    if !req.question.trim().is_empty() {
        out.push_str(&format!(
            "\n{}: {}",
            if lang == "en" { "Player question" } else { "Вопрос игрока" },
            req.question.trim()
        ));
    }
    out
}

/// Pure request-body builder, so tests can inspect exactly what would be sent.
pub fn build_body(req: &Request, prompt: &str, lang: &str, images: &[Vec<u8>], vision: bool) -> serde_json::Value {
    let mut messages = vec![
        serde_json::json!({ "role": "system", "content": system_prompt(lang) }),
    ];
    if vision && !images.is_empty() {
        let mut parts = vec![serde_json::json!({ "type": "text", "text": prompt })];
        for png in images {
            let b64 = base64::engine::general_purpose::STANDARD.encode(png);
            parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": format!("data:image/png;base64,{b64}") }
            }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": parts }));
    } else {
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));
    }
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "max_tokens": 1200,
    });
    if req.reasoning != "auto" {
        body["reasoning_effort"] = serde_json::json!(req.reasoning);
    }
    body
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Answer {
    pub text: String,
    pub reasoning: Option<String>,
    pub model: String,
    pub provider: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub image_sent: bool,
}

#[derive(serde::Deserialize)]
struct ApiResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    error: Option<ApiError>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(serde::Deserialize)]
struct Choice {
    #[serde(default)]
    message: Option<Message>,
}

#[derive(serde::Deserialize)]
struct Message {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(serde::Deserialize)]
struct ApiError {
    #[serde(default)]
    message: Option<String>,
}

#[derive(serde::Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

/// Extracts the answer text. Kept separate from the HTTP call so it is testable.
pub fn parse_response(json: &str) -> Result<Answer, String> {
    let resp: ApiResponse = serde_json::from_str(json).map_err(|e| format!("Ответ API не разобран: {e}"))?;
    if let Some(err) = resp.error {
        return Err(format!("Ошибка API: {}", err.message.unwrap_or_else(|| "без описания".into())));
    }
    let msg = resp.choices.into_iter().next().and_then(|c| c.message).ok_or("API вернул пустой ответ")?;
    let text = msg.content.unwrap_or_default().trim().to_string();
    let reasoning = msg.reasoning_content.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    if text.is_empty() && reasoning.is_none() {
        return Err("API вернул пустой текст".into());
    }
    let usage = resp.usage.unwrap_or(Usage { prompt_tokens: 0, completion_tokens: 0 });
    Ok(Answer {
        text: if text.is_empty() { reasoning.clone().unwrap_or_default() } else { text },
        reasoning,
        model: String::new(),
        provider: String::new(),
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        image_sent: false,
    })
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(90)))
        // No redirects: the endpoint is fixed and must not move elsewhere.
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
}

fn describe_http_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<ApiResponse>(body)
        .ok()
        .and_then(|r| r.error)
        .and_then(|e| e.message)
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status {
        401 | 403 => "Ключ отклонён (401/403). Проверьте ключ и доступ к модели.".to_string(),
        404 => format!("Модель или адрес не найдены (404). {detail}"),
        429 => format!("Слишком много запросов (429). Попробуйте позже. {detail}"),
        _ => format!("HTTP {status}: {detail}"),
    }
}

/// Talks to the provider. `key` is only used in the Authorization header.
pub fn complete(req: &Request, key: &str, lang: &str, images: Vec<Vec<u8>>) -> Result<Answer, String> {
    let url = endpoint(&req.provider).ok_or("Провайдер ИИ не выбран")?;
    if key.trim().is_empty() {
        return Err("Не задан API-ключ".into());
    }
    let vision = model_supports_vision(&req.provider, &req.model);
    let prompt = build_prompt(req, lang, vision);
    let body = build_body(req, &prompt, lang, &images, vision);
    let body = serde_json::to_string(&body).map_err(|e| e.to_string())?;

    let mut res = agent()
        .post(url)
        .header("Authorization", &format!("Bearer {}", key.trim()))
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("Сеть: {e}"))?;
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().map_err(|e| format!("Чтение ответа: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(describe_http_error(status, &text));
    }
    let mut answer = parse_response(&text)?;
    answer.model = req.model.clone();
    answer.provider = req.provider.clone();
    answer.image_sent = vision && !images.is_empty();
    Ok(answer)
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
            return Err("DPAPI: не удалось зашифровать ключ".into());
        }
        let bytes = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
        unsafe { LocalFree(out.pb_data as *mut _) };
        Ok(bytes)
    }

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

pub fn unprotect_key(blob: &str) -> Result<String, String> {
    dpapi::load(blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(provider: &str, model: &str, reasoning: &str) -> AiCfg {
        AiCfg { enabled: true, provider: provider.into(), model: model.into(), reasoning: reasoning.into(), key_protected: None }
    }

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
            note: "кольцо с молнией".into(),
            zone: "Clearfell".into(),
            level: 6,
        }
    }

    fn request(model: &str, reasoning: &str) -> Request {
        let build = BuildDoc::parse(crate::builds::BUNDLED).unwrap();
        prepare(Task::AnalyseItem, &cfg("deepseek", model, reasoning), &[shot()], &profile(), &build, "ru", "Что улучшить?", "Убить босса", &["Cemetery of the Eternals".into()], None)
    }

    #[test]
    fn registry_is_https_and_selectable() {
        assert_eq!(PROVIDERS.len(), 2);
        assert!(PROVIDERS.iter().all(|p| p.endpoint.starts_with("https://")));
        assert!(provider("deepseek").is_some() && provider("openai").is_some());
        assert!(provider("nope").is_none());
        assert!(PROVIDERS.iter().all(|p| p.key_url.starts_with("https://")));
    }

    #[test]
    fn vision_models_are_detected_and_text_only_is_default() {
        assert!(model_supports_vision("openai", "gpt-4o"));
        assert!(model_supports_vision("openai", "gpt-4o-mini"));
        assert!(model_supports_vision("openai", "gpt-4.1-mini"));
        assert!(!model_supports_vision("deepseek", "deepseek-chat"));
        assert!(!model_supports_vision("openai", "gpt-3.5-turbo"));
        assert!(!model_supports_vision("openai", "my-custom-model"));
    }

    #[test]
    fn prepared_request_has_no_secret_and_no_image_bytes() {
        let req = request("deepseek-chat", "auto");
        assert_eq!(req.task, "analyse_item");
        assert_eq!(req.model, "deepseek-chat");
        assert_eq!(req.reasoning, "auto");
        assert_eq!(req.images.len(), 1);
        assert_eq!(req.images[0].file, "shot-1.png");
        assert_eq!(req.context.level, 6);
        assert_eq!(req.context.build_stage, "6–14 уровень");
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("api_key"));
        assert!(!json.contains("data:image"), "no image payload in the description");
        assert!(json.contains("Что улучшить?"));
    }

    #[test]
    fn text_model_gets_no_image_part_and_a_note() {
        let req = request("deepseek-chat", "auto");
        let prompt = build_prompt(&req, "ru", false);
        assert!(prompt.contains("не читает изображения"));
        assert!(prompt.contains("Заметка игрока: кольцо с молнией"));
        let body = build_body(&req, &prompt, "ru", &[vec![1, 2, 3]], false);
        let text = serde_json::to_string(&body).unwrap();
        assert!(!text.contains("image_url"));
        assert!(text.contains("deepseek-chat"));
        assert!(!text.contains("reasoning_effort"), "auto must not send the field");
    }

    #[test]
    fn vision_model_gets_the_png_as_data_url() {
        let req = request("gpt-4o", "auto");
        let prompt = build_prompt(&req, "ru", true);
        let body = build_body(&req, &prompt, "ru", &[vec![9, 8, 7]], true);
        let text = serde_json::to_string(&body).unwrap();
        assert!(text.contains("image_url"));
        assert!(text.contains("data:image/png;base64,"));
        assert!(!text.contains("не читает изображения"));
    }

    #[test]
    fn reasoning_effort_is_sent_only_when_chosen() {
        let req = request("gpt-4o", "high");
        let body = build_body(&req, "x", "ru", &[], false);
        assert_eq!(body["reasoning_effort"], "high");
        let auto = request("gpt-4o", "auto");
        assert!(build_body(&auto, "x", "ru", &[], false).get("reasoning_effort").is_none());
        assert_eq!(normalize_reasoning("MEDIUM"), "medium");
        assert_eq!(normalize_reasoning("nonsense"), "auto");
    }

    #[test]
    fn response_is_parsed_with_usage_and_reasoning() {
        let json = r#"{"choices":[{"message":{"content":"Кольцо хорошее.","reasoning_content":"Сначала смотрю сопротивления."}}],
                       "usage":{"prompt_tokens":120,"completion_tokens":30}}"#;
        let a = parse_response(json).unwrap();
        assert_eq!(a.text, "Кольцо хорошее.");
        assert_eq!(a.reasoning.as_deref(), Some("Сначала смотрю сопротивления."));
        assert_eq!((a.prompt_tokens, a.completion_tokens), (120, 30));
    }

    #[test]
    fn api_errors_are_reported_without_leaking_the_key() {
        let json = r#"{"error":{"message":"invalid api key"}}"#;
        let err = parse_response(json).unwrap_err();
        assert!(err.contains("invalid api key"));
        assert!(parse_response("not json").is_err());
        assert!(parse_response(r#"{"choices":[]}"#).is_err());
        let d = describe_http_error(401, json);
        assert!(d.contains("Ключ отклонён"));
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

    #[test]
    fn item_text_goes_into_the_prompt() {
        let build = BuildDoc::parse(crate::builds::BUNDLED).unwrap();
        let raw = "Item Class: Rings\nRarity: Rare\nDoom Loop\n+18 to maximum Life\n";
        let req = prepare(
            Task::AnalyseItem,
            &cfg("deepseek", "deepseek-chat", "auto"),
            &[],
            &profile(),
            &build,
            "ru",
            "Стоит ли менять?",
            "Куда идти",
            &[],
            Some(raw),
        );
        let prompt = build_prompt(&req, "ru", false);
        assert!(prompt.contains("Предмет, скопированный из игры:"));
        assert!(prompt.contains("Doom Loop"));
        assert!(prompt.contains("Стоит ли менять?"));
        let body = build_body(&req, &prompt, "ru", &[], false);
        assert!(serde_json::to_string(&body).unwrap().contains("Doom Loop"));
    }

    #[test]
    fn prompt_contains_context_and_question() {
        let req = request("deepseek-chat", "auto");
        let p = build_prompt(&req, "ru", false);
        assert!(p.contains("Персонаж: Эни"));
        assert!(p.contains("уровень 6"));
        assert!(p.contains("Lightning Arrow"));
        assert!(p.contains("Текущая цель кампании: Убить босса"));
        assert!(p.contains("Вопрос игрока: Что улучшить?"));
    }
}
