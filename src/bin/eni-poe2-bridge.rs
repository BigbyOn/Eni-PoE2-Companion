//! Headless JSON bridge between Eni-PoE2-Companion and Eni-VTuber.
//!
//! The modules below are the *same source files* the Tauri app compiles, so the
//! bridge cannot drift from the UI: state, builds, campaign, item parsing, the
//! RU→EN trade tables, the trade API client and the budget/decision logic are
//! shared, not copied.
//!
//! Protocol: one JSON object per line (or `--request '<json>'` for a single
//! call). The response is always one line:
//!
//! ```json
//! {"id":"1","ok":true,"result":{...}}
//! {"id":"2","ok":false,"error":{"code":"no_build","message":"..."}}
//! ```
//!
//! The bridge never starts the UI, never touches the game window and only uses
//! the network when a method explicitly asks for it (`trade.price`,
//! `trade.search` with `auto:true`).

#![allow(dead_code)]

#[path = "../budget.rs"]
mod budget;
#[path = "../builds.rs"]
mod builds;
#[path = "../campaign.rs"]
mod campaign;
#[path = "../companion.rs"]
mod companion;
#[path = "../decisions.rs"]
mod decisions;
#[path = "../guide.rs"]
mod guide;
#[path = "../item.rs"]
mod item;
#[path = "../model.rs"]
mod model;
#[path = "../scout.rs"]
mod scout;
#[path = "../trade.rs"]
mod trade;
#[path = "../trade_i18n.rs"]
mod trade_i18n;

use serde_json::{json, Value};
use std::io::Read;

fn handle(ctx: &companion::Context, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

    if method.is_empty() {
        return json!({
            "id": id,
            "ok": false,
            "error": { "code": "no_method", "message": "В запросе нет поля method." }
        });
    }

    match companion::dispatch(ctx, &method, params) {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err(error) => json!({ "id": id, "ok": false, "error": error }),
    }
}

fn emit(value: Value) {
    println!("{}", value);
}

fn main() {
    let mut request_arg: Option<String> = None;
    let mut data_dir: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--request" | "-r" => request_arg = args.next(),
            "--data-dir" => data_dir = args.next(),
            "--help" | "-h" => {
                eprintln!(
                    "eni-poe2-bridge: JSON-lines on stdin, or --request '<json>'. \
                     --data-dir <path> overrides ENI_COMPANION_DATA_DIR."
                );
                return;
            }
            other => eprintln!("unknown argument: {other}"),
        }
    }
    if let Some(dir) = data_dir {
        std::env::set_var("ENI_COMPANION_DATA_DIR", dir);
    }
    let ctx = companion::Context::resolve();

    if let Some(text) = request_arg {
        let text = text.trim();
        let request: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                emit(json!({
                    "id": null,
                    "ok": false,
                    "error": { "code": "bad_json", "message": format!("Не разобран запрос: {error}") }
                }));
                return;
            }
        };
        emit(handle(&ctx, &request));
        return;
    }

    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        emit(json!({
            "ok": false,
            "error": { "code": "stdin", "message": "Не удалось прочитать stdin." }
        }));
        return;
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        emit(json!({
            "ok": false,
            "error": { "code": "empty", "message": "Пустой запрос: передай JSON в stdin." }
        }));
        return;
    }

    // A JSON array is a batch; otherwise one object, or JSON lines.
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        match value {
            Value::Array(items) => {
                for item in items {
                    emit(handle(&ctx, &item));
                }
                return;
            }
            other => {
                emit(handle(&ctx, &other));
                return;
            }
        }
    }

    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(request) => emit(handle(&ctx, &request)),
            Err(error) => emit(json!({
                "ok": false,
                "error": { "code": "bad_json", "message": format!("Строка не разобрана: {error}") }
            })),
        }
    }
}
