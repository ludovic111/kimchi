//! Ollama's `/api/chat` with tools, streamed as NDJSON. Nothing leaves the computer.

use serde_json::{Value, json};

use super::{Api, Call, Step, chat};
use crate::http::{self, Lines};
use crate::tools::ToolSet;
use crate::{Message, Part, Role, Run};

fn wire(system: &str, messages: &[Message]) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": system })];
    for m in messages {
        let text = m.text();
        match m.role {
            Role::User => {
                for p in &m.parts {
                    if let Part::ToolResult { id, name, output, .. } = p {
                        out.push(json!({ "role": "tool", "tool_name": name, "tool_call_id": id, "content": output }));
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

pub(super) async fn step(api: &Api, run: &Run, set: &ToolSet, messages: &[Message], round: usize) -> Result<Step, String> {
    let body = json!({
        "model": api.model,
        "messages": wire(&set.system_prompt(), messages),
        "tools": chat::tools(&set.defs),
        "stream": true,
        // Ollama's default context is short for tools and a project overview.
        "options": { "num_ctx": 16_384 },
    });
    let url = format!("{}/api/chat", api.base);
    let response = http::post(&run.cancel, &format!("Ollama at {}", api.base), || api.http.post(&url), &body).await?;
    let mut lines = Lines::new(response);
    let mut text = String::new();
    let mut calls: Vec<(String, String, Result<Value, String>)> = vec![];
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
            calls.push((tc["id"].as_str().unwrap_or("").to_string(), f["name"].as_str().unwrap_or("").to_string(), args));
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
    for (i, (id, name, input)) in calls.into_iter().enumerate().filter(|(_, c)| !c.1.is_empty()) {
        // Older Ollama versions give no call ids; these only pair calls with results in the thread.
        let id = if id.is_empty() { format!("call_{}_{round}_{i}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()) } else { id };
        parts.push(Part::ToolUse { id: id.clone(), name: name.clone(), input: input.clone().unwrap_or_else(|_| json!({})) });
        out.push(Call { id, name, input });
    }
    Ok(Step { parts, calls: out })
}
