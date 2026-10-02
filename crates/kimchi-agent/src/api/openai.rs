//! OpenAI Chat Completions with streamed text and function tools. Also any
//! compatible server, through `settings.agent.baseUrl`.

use serde_json::{Value, json};

use super::{Api, Call, Step, parse_args};
use crate::http::{self, Lines};
use crate::tools::SYSTEM_PROMPT;
use crate::{Message, Part, ProviderKind, Role, Run, ToolDef};

pub(super) fn tools(defs: &[ToolDef]) -> Vec<Value> {
    defs.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.schema } })).collect()
}

pub(super) fn wire(messages: &[Message]) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
    for m in messages {
        let text = m.text();
        match m.role {
            Role::User => {
                // Tool answers come first: they must follow the assistant's calls.
                for p in &m.parts {
                    if let Part::ToolResult { id, output, .. } = p {
                        out.push(json!({ "role": "tool", "tool_call_id": id, "content": output }));
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
                        Part::ToolUse { id, name, input } => Some(json!({ "id": id, "type": "function", "function": { "name": name, "arguments": input.to_string() } })),
                        _ => None,
                    })
                    .collect();
                let mut msg = json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) } });
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                } else if text.is_empty() {
                    msg["content"] = json!("…");
                }
                out.push(msg);
            }
        }
    }
    out
}

pub(super) async fn step(api: &Api, run: &Run, defs: &[ToolDef], messages: &[Message]) -> Result<Step, String> {
    let mut body = json!({
        "model": api.model,
        "messages": wire(messages),
        "tools": tools(defs),
        "tool_choice": "auto",
        "stream": true,
    });
    let official = api.base == ProviderKind::OpenAi.default_base_url();
    if official {
        body["stream_options"] = json!({ "include_usage": true });
    }
    let url = format!("{}/chat/completions", api.base);
    let label = if official { "the OpenAI API".to_string() } else { api.base.clone() };
    let response = http::post(
        &run.cancel,
        &label,
        || {
            let r = api.http.post(&url);
            match &api.key {
                Some(k) => r.bearer_auth(k),
                None => r,
            }
        },
        &body,
    )
    .await?;

    let mut lines = Lines::new(response);
    let mut text = String::new();
    // index → (id, name, arguments)
    let mut calls: Vec<(String, String, String)> = vec![];
    let mut finish = String::new();
    while let Some(data) = lines.next_sse().await? {
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(e) = chunk["error"]["message"].as_str() {
            return Err(format!("{label} error: {e}"));
        }
        if let Some(u) = chunk.get("usage").filter(|u| u.is_object()) {
            run.usage(u["prompt_tokens"].as_u64().unwrap_or(0), u["completion_tokens"].as_u64().unwrap_or(0));
        }
        let Some(choice) = chunk["choices"].get(0) else { continue };
        let delta = &choice["delta"];
        if let Some(t) = delta["content"].as_str() {
            text.push_str(t);
            run.text(t);
        }
        for tc in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = tc["index"].as_u64().unwrap_or(calls.len() as u64) as usize;
            while calls.len() <= index {
                calls.push(Default::default());
            }
            let c = &mut calls[index];
            if let Some(id) = tc["id"].as_str() {
                c.0 = id.to_string();
            }
            if let Some(n) = tc["function"]["name"].as_str() {
                c.1.push_str(n);
            }
            if let Some(a) = tc["function"]["arguments"].as_str() {
                c.2.push_str(a);
            }
        }
        if let Some(f) = choice["finish_reason"].as_str() {
            finish = f.to_string();
        }
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
        let id = if id.is_empty() { format!("call_{i}") } else { id };
        let input = parse_args(&args);
        parts.push(Part::ToolUse { id: id.clone(), name: name.clone(), input: input.clone().unwrap_or_else(|_| json!({})) });
        out.push(Call { id, name, input });
    }
    Ok(Step { parts, calls: out })
}
