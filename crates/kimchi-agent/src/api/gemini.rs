//! The Gemini API's own `streamGenerateContent` (server-sent events) with function calling.
//!
//! Native rather than Google's OpenAI-compatible endpoint: Gemini 3 models sign their reasoning
//! (`thoughtSignature` on the parts of a reply) and want the signatures back on the following
//! requests of a tool loop; the native API carries them as they are. A reply's parts are kept
//! verbatim as one [`Part::Opaque`] and replayed to Gemini as sent; other providers read the
//! neutral text and tool parts next to it. Tools are declared with `parametersJsonSchema` (JSON
//! Schema as the registry writes it). ai.google.dev/api/generate-content and
//! ai.google.dev/gemini-api/docs/function-calling, 2026-10.

use serde_json::{Value, json};

use super::{Api, Call, Step};
use crate::http::{self, Lines};
use crate::tools::ToolSet;
use crate::{Message, Part, ProviderKind, Role, Run};

/// Prefix of the call ids kimchi makes up for models that give none.
const MADE_UP: &str = "gcall_";

fn with_id(mut part: Value, key: &str, id: &str) -> Value {
    if !id.starts_with(MADE_UP) && !id.is_empty() {
        part[key]["id"] = json!(id);
    }
    part
}

fn tools(set: &ToolSet) -> Value {
    let decls: Vec<Value> = set.defs.iter().map(|t| json!({ "name": t.name, "description": t.description, "parametersJsonSchema": t.schema })).collect();
    json!([{ "functionDeclarations": decls }])
}

/// The conversation as Gemini `contents`: roles alternate (`user` / `model`), a reply Gemini
/// wrote goes back as it came (signatures included), anything else is rebuilt from its parts.
pub(super) fn wire(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    for m in messages {
        let raw = m.parts.iter().find_map(|p| match p {
            Part::Opaque { provider: ProviderKind::Gemini, block } => block["parts"].as_array().cloned(),
            _ => None,
        });
        let parts: Vec<Value> = match raw {
            Some(parts) if m.role == Role::Assistant => parts,
            _ => m
                .parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text { text } if text.is_empty() => None,
                    Part::Text { text } => Some(json!({ "text": text })),
                    Part::ToolUse { id, name, input } => Some(with_id(json!({ "functionCall": { "name": name, "args": input } }), "functionCall", id)),
                    Part::ToolResult { id, name, output, is_error } => {
                        let response = if *is_error { json!({ "error": output }) } else { json!({ "result": output }) };
                        Some(with_id(json!({ "functionResponse": { "name": name, "response": response } }), "functionResponse", id))
                    }
                    Part::Image { media_type, data, .. } => Some(json!({ "inlineData": { "mimeType": media_type, "data": data } })),
                    Part::Opaque { .. } => None,
                })
                .collect(),
        };
        let role = if m.role == Role::User { "user" } else { "model" };
        match out.last_mut() {
            Some(last) if last["role"] == role => {
                if let Some(p) = last["parts"].as_array_mut() {
                    p.extend(parts);
                }
            }
            _ => out.push(json!({ "role": role, "parts": parts })),
        }
    }
    for c in &mut out {
        if c["parts"].as_array().is_some_and(|p| p.is_empty()) {
            c["parts"] = json!([{ "text": "…" }]);
        }
    }
    out
}

pub(super) async fn step(api: &Api, run: &Run, set: &ToolSet, messages: &[Message], round: usize) -> Result<Step, String> {
    let body = json!({
        "systemInstruction": { "parts": [{ "text": set.system_prompt() }] },
        "contents": wire(messages),
        "tools": tools(set),
        "toolConfig": { "functionCallingConfig": { "mode": "AUTO" } },
    });
    let model = api.model.trim_start_matches("models/");
    let url = format!("{}/models/{model}:streamGenerateContent?alt=sse", api.base);
    let label = api.label();
    let key = api.key.clone().unwrap_or_default();
    let response = http::post(&run.cancel, &label, || api.http.post(&url).header("x-goog-api-key", &key), &body).await?;

    let mut lines = Lines::new(response);
    // Every part of the reply as streamed: text parts with the same signature state are joined.
    let mut raw: Vec<Value> = vec![];
    let mut finish = String::new();
    let mut usage = (0, 0);
    while let Some(data) = lines.next_sse().await? {
        let Ok(chunk) = serde_json::from_str::<Value>(data.trim()) else { continue };
        if let Some(e) = chunk["error"]["message"].as_str() {
            return Err(format!("{label} error: {e}"));
        }
        if let Some(u) = chunk.get("usageMetadata") {
            usage = (u["promptTokenCount"].as_u64().unwrap_or(usage.0), u["candidatesTokenCount"].as_u64().unwrap_or(0) + u["thoughtsTokenCount"].as_u64().unwrap_or(0));
        }
        if let Some(reason) = chunk["promptFeedback"]["blockReason"].as_str() {
            return Err(format!("The model declined this request ({reason})."));
        }
        let Some(cand) = chunk["candidates"].get(0) else { continue };
        for part in cand["content"]["parts"].as_array().into_iter().flatten() {
            if let Some(t) = part["text"].as_str()
                && part["thought"].as_bool() != Some(true)
            {
                run.text(t);
            }
            if let Some(name) = part["functionCall"]["name"].as_str() {
                run.status(format!("Running {}…", crate::cli::tool_label(name)));
            }
            // A text delta continues the last text part unless it starts a new signed one.
            let joins = part.get("text").is_some()
                && part.get("thoughtSignature").is_none()
                && raw.last().is_some_and(|l: &Value| l.get("text").is_some() && l["thought"] == part["thought"] && l.get("functionCall").is_none());
            if joins && let Some(last) = raw.last_mut() {
                let joined = format!("{}{}", last["text"].as_str().unwrap_or(""), part["text"].as_str().unwrap_or(""));
                last["text"] = json!(joined);
            } else {
                raw.push(part.clone());
            }
        }
        if let Some(f) = cand["finishReason"].as_str() {
            finish = f.to_string();
        }
    }
    run.usage(usage.0, usage.1);
    match finish.as_str() {
        "" => return Err(format!("{label}: the connection closed before the reply finished. Try again.")),
        "MAX_TOKENS" => return Err("The reply reached its length limit; no unfinished command was run. Ask for less at a time.".into()),
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => return Err(format!("The model declined this request ({finish}).")),
        "MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL" => {
            return Err("The model sent a command Gemini couldn't read. Try again, or choose another model.".into());
        }
        _ => {}
    }
    let mut parts = vec![];
    let mut calls = vec![];
    let mut text = String::new();
    for (i, p) in raw.iter().enumerate() {
        if let Some(t) = p["text"].as_str()
            && p["thought"].as_bool() != Some(true)
        {
            text.push_str(t);
        }
        if let Some(f) = p.get("functionCall") {
            let name = f["name"].as_str().unwrap_or("").to_string();
            // Gemini 3 gives calls ids; for older models one is made up to pair the answer
            // with, and left out of what goes back to Gemini.
            let id = f["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("{MADE_UP}{round}_{i}"));
            let input = match &f["args"] {
                Value::Null => json!({}),
                v => v.clone(),
            };
            if !text.is_empty() {
                parts.push(Part::Text { text: std::mem::take(&mut text) });
            }
            parts.push(Part::ToolUse { id: id.clone(), name: name.clone(), input: input.clone() });
            calls.push(Call { id, name, input: Ok(input) });
        }
    }
    if !text.is_empty() {
        parts.push(Part::Text { text });
    }
    parts.push(Part::Opaque { provider: ProviderKind::Gemini, block: json!({ "parts": raw }) });
    Ok(Step { parts, calls })
}
