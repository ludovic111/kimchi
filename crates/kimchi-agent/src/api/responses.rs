//! The OpenAI Responses API (`POST /v1/responses`), for OpenAI's own models.
//!
//! Chosen over Chat Completions for OpenAI: reasoning models (gpt-5 and later) keep their
//! reasoning between tool calls only here. Requests are stateless (`store: false`) and ask for
//! the reasoning items encrypted (`include: ["reasoning.encrypted_content"]`); those items come
//! back verbatim as [`Part::Opaque`] and are replayed to OpenAI only. Streaming events followed:
//! `response.output_text.delta`, `response.output_item.added` / `.done`,
//! `response.function_call_arguments.delta`, `response.completed` / `.incomplete` / `.failed`
//! and `error` (platform.openai.com/docs/api-reference/responses-streaming, 2026-10).

use serde_json::{Value, json};

use super::{Api, Call, Step, parse_args};
use crate::http::{self, Lines};
use crate::tools::ToolSet;
use crate::{Message, Part, Role, Run};

fn tools(set: &ToolSet) -> Vec<Value> {
    set.defs.iter().map(|t| json!({ "type": "function", "name": t.name, "description": t.description, "parameters": t.schema })).collect()
}

pub(super) fn wire(api: &Api, messages: &[Message]) -> Vec<Value> {
    let mut out = vec![];
    for m in messages {
        match m.role {
            Role::User => {
                for p in &m.parts {
                    match p {
                        Part::ToolResult { id, output, .. } => out.push(json!({ "type": "function_call_output", "call_id": id, "output": output })),
                        Part::Text { text } if !text.is_empty() => out.push(json!({ "role": "user", "content": text })),
                        _ => {}
                    }
                }
            }
            Role::Assistant => {
                for p in &m.parts {
                    match p {
                        Part::Text { text } if !text.is_empty() => {
                            out.push(json!({ "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": text }] }))
                        }
                        Part::ToolUse { id, name, input } => out.push(json!({ "type": "function_call", "call_id": id, "name": name, "arguments": input.to_string() })),
                        Part::Opaque { provider, block } if *provider == api.kind => out.push(block.clone()),
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

pub(super) async fn step(api: &Api, run: &Run, set: &ToolSet, messages: &[Message]) -> Result<Step, String> {
    let body = json!({
        "model": api.model,
        "instructions": set.system_prompt(),
        "input": wire(api, messages),
        "tools": tools(set),
        "tool_choice": "auto",
        "store": false,
        "include": ["reasoning.encrypted_content"],
        "stream": true,
    });
    let url = format!("{}/responses", api.base);
    let label = api.label();
    let response = http::post(&run.cancel, &label, || api.authorized(api.http.post(&url)), &body).await?;

    let mut lines = Lines::new(response);
    let mut items: Vec<(usize, Value)> = vec![];
    let mut streamed_text = false;
    let mut finished: Option<Value> = None;
    while let Some(data) = lines.next_sse().await? {
        let Ok(ev) = serde_json::from_str::<Value>(data.trim()) else { continue };
        match ev["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => {
                streamed_text = true;
                run.text(ev["delta"].as_str().unwrap_or(""));
            }
            "response.output_item.added" => {
                if ev["item"]["type"] == "function_call" {
                    run.status(format!("Running {}…", crate::cli::tool_label(ev["item"]["name"].as_str().unwrap_or("a command"))));
                }
            }
            "response.output_item.done" => {
                let index = ev["output_index"].as_u64().unwrap_or(items.len() as u64) as usize;
                items.push((index, ev["item"].clone()));
            }
            "response.completed" | "response.incomplete" => {
                finished = Some(ev["response"].clone());
                break;
            }
            "response.failed" => {
                let why = ev["response"]["error"]["message"].as_str().unwrap_or("the request failed");
                return Err(format!("{label} error: {why}"));
            }
            "error" => {
                let why = ev["message"].as_str().or_else(|| ev["error"]["message"].as_str()).unwrap_or("unknown");
                return Err(format!("{label} error: {why}"));
            }
            _ => {}
        }
    }
    let Some(response) = finished else {
        return Err(format!("{label}: the connection closed before the reply finished. Try again."));
    };
    let u = &response["usage"];
    run.usage(u["input_tokens"].as_u64().unwrap_or(0), u["output_tokens"].as_u64().unwrap_or(0));
    if response["status"] == "incomplete" {
        return Err(match response["incomplete_details"]["reason"].as_str() {
            Some("content_filter") => "The model declined this request.".into(),
            _ => "The reply reached its length limit; no unfinished command was run. Ask for less at a time.".into(),
        });
    }
    // The finished items, in output order (the completed response has them too, if the
    // `.done` events were missed).
    if items.is_empty() {
        items = response["output"].as_array().into_iter().flatten().cloned().enumerate().collect();
    }
    items.sort_by_key(|(i, _)| *i);
    let mut parts = vec![];
    let mut calls = vec![];
    for (_, item) in items {
        match item["type"].as_str().unwrap_or("") {
            "message" => {
                let text: String = item["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
                if !streamed_text {
                    run.text(&text);
                }
                if !text.is_empty() {
                    parts.push(Part::Text { text });
                }
            }
            "function_call" => {
                let id = item["call_id"].as_str().unwrap_or("").to_string();
                let name = item["name"].as_str().unwrap_or("").to_string();
                let input = parse_args(item["arguments"].as_str().unwrap_or(""));
                parts.push(Part::ToolUse { id: id.clone(), name: name.clone(), input: input.clone().unwrap_or_else(|_| json!({})) });
                calls.push(Call { id, name, input });
            }
            "reasoning" => {
                // Replayed as sent; the id means nothing to a stateless request.
                let mut block = item.clone();
                if let Some(o) = block.as_object_mut() {
                    o.remove("status");
                }
                parts.push(Part::Opaque { provider: api.kind, block });
            }
            _ => {}
        }
    }
    Ok(Step { parts, calls })
}
