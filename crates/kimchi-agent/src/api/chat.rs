//! Chat Completions (`POST {base}/chat/completions`) with streamed text and function tools:
//! OpenRouter, Groq, Mistral, DeepSeek, xAI, Together, Fireworks, Cerebras, Azure OpenAI, LM
//! Studio, any OpenAI-compatible server, and OpenAI itself at another address. One
//! implementation; [`Quirks`] says where a server differs (the reply cap's name, usage in the
//! stream, tool caps, Mistral's tool call ids, reasoning that has to go back, Azure's
//! `api-key` header and its filter-only chunks).

use serde_json::{Value, json};

use super::{Api, Call, Step, parse_args};
use crate::http::{self, Lines};
use crate::providers::Quirks;
use crate::tools::ToolSet;
use crate::{Message, Part, Role, Run, ToolDef};

pub(super) fn tools(defs: &[ToolDef]) -> Vec<Value> {
    defs.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.schema } })).collect()
}

/// A tool call id as Mistral takes them: nine letters and digits, the same for the same id.
pub(crate) fn short_id(id: &str) -> String {
    if id.len() == 9 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return id.to_string();
    }
    // FNV-1a, spelled out in base 62.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    (0..9)
        .map(|_| {
            let c = DIGITS[(h % 62) as usize] as char;
            h /= 62;
            c
        })
        .collect()
}

pub(super) fn wire(api: &Api, q: Quirks, system: &str, messages: &[Message]) -> Vec<Value> {
    let id = |s: &str| if q.short_ids { short_id(s) } else { s.to_string() };
    let mut out = vec![json!({ "role": "system", "content": system })];
    for m in messages {
        let text = m.text();
        match m.role {
            Role::User => {
                // Tool answers come first: they must follow the assistant's calls.
                for p in &m.parts {
                    if let Part::ToolResult { id: call, output, .. } = p {
                        out.push(json!({ "role": "tool", "tool_call_id": id(call), "content": output }));
                    }
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
                        Part::ToolUse { id: call, name, input } => Some(json!({ "id": id(call), "type": "function", "function": { "name": name, "arguments": input.to_string() } })),
                        _ => None,
                    })
                    .collect();
                let mut msg = json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) } });
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                } else if text.is_empty() {
                    msg["content"] = json!("…");
                }
                // What this provider asked to get back with the message (reasoning).
                for p in &m.parts {
                    if let Part::Opaque { provider, block } = p
                        && *provider == api.kind
                        && let Some(o) = block.as_object()
                    {
                        for (k, v) in o {
                            msg[k] = v.clone();
                        }
                    }
                }
                out.push(msg);
            }
        }
    }
    out
}

/// A text delta: a string, or (Mistral's thinking models) a list of typed chunks.
fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect(),
        _ => String::new(),
    }
}

pub(super) async fn step(api: &Api, q: Quirks, run: &Run, set: &ToolSet, messages: &[Message], round: usize) -> Result<Step, String> {
    let mut body = json!({
        "model": api.model,
        "messages": wire(api, q, &set.system_prompt(), messages),
        "tools": tools(&set.defs),
        "tool_choice": "auto",
        "stream": true,
    });
    if q.usage {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if let Some((name, n)) = q.max_tokens {
        body[name] = json!(n);
    }
    // Azure's whole URL is the base already.
    let url = if api.base.contains("/chat/completions") { api.base.clone() } else { format!("{}/chat/completions", api.base) };
    let label = api.label();
    let response = http::post(&run.cancel, &label, || api.authorized(api.http.post(&url)), &body).await?;

    let mut lines = Lines::new(response);
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut details: Vec<Value> = vec![];
    // index → (id, name, arguments)
    let mut calls: Vec<(String, String, String)> = vec![];
    let mut finish = String::new();
    let mut done = false;
    while let Some(data) = lines.next_sse().await? {
        let data = data.trim();
        if data == "[DONE]" {
            done = true;
            break;
        }
        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(e) = chunk["error"]["message"].as_str().or_else(|| chunk["error"].as_str()) {
            return Err(format!("{label} error: {e}"));
        }
        if let Some(u) = chunk.get("usage").filter(|u| u.is_object()) {
            run.usage(u["prompt_tokens"].as_u64().unwrap_or(0), u["completion_tokens"].as_u64().unwrap_or(0));
        }
        // Azure's content filter notes come as chunks without choices or deltas.
        let Some(choice) = chunk["choices"].get(0) else { continue };
        let delta = &choice["delta"];
        let t = content_text(&delta["content"]);
        if !t.is_empty() {
            text.push_str(&t);
            run.text(&t);
        }
        if let Some(r) = delta["reasoning_content"].as_str().or_else(|| delta["reasoning"].as_str()) {
            reasoning.push_str(r);
        }
        if let Some(d) = delta["reasoning_details"].as_array() {
            details.extend(d.iter().cloned());
        }
        for tc in delta["tool_calls"].as_array().into_iter().flatten() {
            // Mistral sends whole calls without an index: a new call, unless its id was seen.
            let id = tc["id"].as_str().unwrap_or("");
            let index = match tc["index"].as_u64() {
                Some(i) => i as usize,
                None => calls.iter().position(|c| !id.is_empty() && c.0 == id).unwrap_or(calls.len()),
            };
            while calls.len() <= index {
                calls.push(Default::default());
            }
            let c = &mut calls[index];
            if let Some(id) = tc["id"].as_str().filter(|i| !i.is_empty()) {
                c.0 = id.to_string();
            }
            if let Some(n) = tc["function"]["name"].as_str() {
                if c.1.is_empty() {
                    run.status(format!("Running {}…", crate::cli::tool_label(n)));
                }
                c.1.push_str(n);
            }
            match &tc["function"]["arguments"] {
                Value::String(a) => c.2.push_str(a),
                // Some servers send the arguments as an object.
                v @ Value::Object(_) => c.2 = v.to_string(),
                _ => {}
            }
        }
        if let Some(f) = choice["finish_reason"].as_str() {
            finish = f.to_string();
        }
    }
    // Some compatible servers close after the finish reason without `[DONE]`; with neither, the
    // reply (and any half-streamed command) is cut off.
    if !done && finish.is_empty() {
        return Err(format!("{label}: the connection closed before the reply finished. Try again."));
    }
    match finish.as_str() {
        "length" => return Err("The reply reached its length limit; no unfinished command was run. Ask for less at a time.".into()),
        "content_filter" => return Err("The model declined this request.".into()),
        _ => {}
    }
    let mut parts = vec![];
    if !text.is_empty() {
        parts.push(Part::Text { text });
    }
    let mut out = vec![];
    for (i, (id, name, args)) in calls.into_iter().enumerate().filter(|(_, c)| !c.1.is_empty()) {
        let id = if id.is_empty() { format!("call_{round}_{i}") } else { id };
        let input = parse_args(&args);
        parts.push(Part::ToolUse { id: id.clone(), name: name.clone(), input: input.clone().unwrap_or_else(|_| json!({})) });
        out.push(Call { id, name, input });
    }
    // Reasoning that must go back with this message, for this provider only.
    let mut back = serde_json::Map::new();
    if q.reasoning_back && !reasoning.is_empty() && !out.is_empty() {
        back.insert("reasoning_content".into(), json!(reasoning));
    }
    if q.reasoning_details && !details.is_empty() {
        back.insert("reasoning_details".into(), json!(details));
    }
    if !back.is_empty() {
        parts.push(Part::Opaque { provider: api.kind, block: Value::Object(back) });
    }
    Ok(Step { parts, calls: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistral_ids_are_nine_letters_and_digits_and_stable() {
        let a = short_id("toolu_01ABCdef");
        assert_eq!(a.len(), 9);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(a, short_id("toolu_01ABCdef"));
        assert_ne!(a, short_id("toolu_01ABCdeg"));
        assert_eq!(short_id("Ab3dE6gH9"), "Ab3dE6gH9");
    }

    #[test]
    fn typed_content_chunks_give_their_text() {
        assert_eq!(content_text(&json!([{ "type": "thinking", "thinking": [] }, { "type": "text", "text": "Hi" }])), "Hi");
        assert_eq!(content_text(&json!("Hey")), "Hey");
    }
}
