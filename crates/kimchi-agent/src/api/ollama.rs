//! Ollama's `/api/chat` with tools, streamed as NDJSON. Nothing leaves the computer.

use std::time::Duration;

use serde_json::{Value, json};

use super::{Api, Call, Step, openai};
use crate::http::{self, Lines};
use crate::tools::SYSTEM_PROMPT;
use crate::{Message, Part, Role, Run, ToolDef};

/// Installed models, newest first as Ollama lists them.
pub(crate) async fn models(http: &reqwest::Client, base: &str) -> Result<Vec<String>, String> {
    let r = http
        .get(format!("{base}/api/tags"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|_| format!("Ollama isn't running at {base}. Start it, or choose another provider in Settings › Agent."))?;
    let v: Value = r.json().await.map_err(|e| format!("Ollama at {base} answered oddly: {e}"))?;
    Ok(v["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(str::to_string)).collect())
}

/// Whether `model` takes pictures (Ollama lists `vision` among its capabilities). Unknown: no.
pub(crate) async fn sees(http: &reqwest::Client, base: &str, model: &str) -> bool {
    let shown = http.post(format!("{base}/api/show")).json(&json!({ "model": model })).timeout(Duration::from_secs(5)).send().await;
    let Ok(r) = shown else { return false };
    let v: Value = r.json().await.unwrap_or_default();
    v["capabilities"].as_array().is_some_and(|c| c.iter().any(|c| c == "vision"))
}

/// Pictures go in a user message after the tool answers (the latest few, as for OpenAI).
fn wire(messages: &[Message], vision: bool) -> Vec<Value> {
    let recent = super::recent_pictures(messages, vision);
    let mut out = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
    for (i, m) in messages.iter().enumerate() {
        let text = m.text();
        match m.role {
            Role::User => {
                for p in &m.parts {
                    if let Part::ToolResult { name, output, .. } = p {
                        out.push(json!({ "role": "tool", "tool_name": name, "content": output }));
                    }
                }
                let (shown, older) = super::pictures_of(m, i, &recent);
                if !shown.is_empty() || older > 0 {
                    let images: Vec<&str> = shown.iter().map(|(_, data)| *data).collect();
                    out.push(json!({ "role": "user", "content": super::pictures_note(shown.len(), older), "images": images }));
                }
                if !text.is_empty() {
                    out.push(json!({ "role": "user", "content": text }));
                }
            }
            Role::Assistant => {
                let calls: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::ToolUse { name, input, .. } => Some(json!({ "function": { "name": name, "arguments": input } })),
                        _ => None,
                    })
                    .collect();
                let mut msg = json!({ "role": "assistant", "content": text });
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                out.push(msg);
            }
        }
    }
    out
}

pub(super) async fn step(api: &Api, run: &Run, defs: &[ToolDef], messages: &[Message], round: usize) -> Result<Step, String> {
    let body = json!({ "model": api.model, "messages": wire(messages, api.sees()), "tools": openai::tools(defs), "stream": true });
    let url = format!("{}/api/chat", api.base);
    let response = http::post(&run.cancel, &format!("Ollama at {}", api.base), || api.http.post(&url), &body).await?;
    let mut lines = Lines::new(response);
    let mut text = String::new();
    let mut calls: Vec<(String, Result<Value, String>)> = vec![];
    let mut done = false;
    while let Some(line) = lines.next().await? {
        let Ok(chunk) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(e) = chunk["error"].as_str() {
            return Err(format!("Ollama error: {e}"));
        }
        let msg = &chunk["message"];
        if let Some(t) = msg["content"].as_str() {
            text.push_str(t);
            run.text(t);
        }
        for tc in msg["tool_calls"].as_array().into_iter().flatten() {
            let f = &tc["function"];
            let args = match &f["arguments"] {
                Value::String(s) => super::parse_args(s),
                Value::Null => Ok(json!({})),
                v => Ok(v.clone()),
            };
            calls.push((f["name"].as_str().unwrap_or("").to_string(), args));
        }
        if chunk["done"].as_bool() == Some(true) {
            run.usage(chunk["prompt_eval_count"].as_u64().unwrap_or(0), chunk["eval_count"].as_u64().unwrap_or(0));
            if chunk["done_reason"] == "length" {
                return Err("The reply reached its length limit; no unfinished command was run.".into());
            }
            done = true;
            break;
        }
    }
    if !done {
        return Err("Ollama: the connection closed before the reply finished. Try again.".into());
    }
    let mut parts = vec![];
    if !text.is_empty() {
        parts.push(Part::Text { text });
    }
    let mut out = vec![];
    for (i, (name, input)) in calls.into_iter().enumerate().filter(|(_, c)| !c.0.is_empty()) {
        // Ollama has no call ids; these only pair calls with results in the thread.
        let id = format!("call_{}_{round}_{i}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
        parts.push(Part::ToolUse { id: id.clone(), name: name.clone(), input: input.clone().unwrap_or_else(|_| json!({})) });
        out.push(Call { id, name, input });
    }
    Ok(Step { parts, calls: out })
}
