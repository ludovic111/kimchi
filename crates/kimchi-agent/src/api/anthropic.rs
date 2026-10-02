//! Anthropic Messages API: streamed text and tool use over server-sent events.
//!
//! Thinking blocks (on by default on current models) are kept verbatim as
//! [`Part::Opaque`] and replayed unchanged, as the API requires within a tool
//! loop; the system prompt and tool list never change during a conversation so
//! the replayed blocks stay valid and the prompt cache holds.

use serde_json::{Value, json};

use super::{Api, Call, Step, parse_args};
use crate::http::{self, Lines};
use crate::tools::SYSTEM_PROMPT;
use crate::{Message, Part, ProviderKind, Role, Run, ToolDef};

const VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 32_000;

fn tools(defs: &[ToolDef]) -> Vec<Value> {
    defs.iter().map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema })).collect()
}

pub(super) fn wire(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let mut content: Vec<Value> = m
                .parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text { text } if text.is_empty() => None,
                    Part::Text { text } => Some(json!({ "type": "text", "text": text })),
                    Part::ToolUse { id, name, input } => Some(json!({ "type": "tool_use", "id": id, "name": name, "input": input })),
                    Part::ToolResult { id, output, is_error, .. } => {
                        Some(json!({ "type": "tool_result", "tool_use_id": id, "content": output, "is_error": is_error }))
                    }
                    Part::Opaque { provider: ProviderKind::Anthropic, block } => Some(block.clone()),
                    Part::Opaque { .. } => None,
                })
                .collect();
            if content.is_empty() {
                content.push(json!({ "type": "text", "text": "…" }));
            }
            json!({ "role": if m.role == Role::User { "user" } else { "assistant" }, "content": content })
        })
        .collect()
}

/// A content block being streamed.
enum Block {
    Text(String),
    Tool { id: String, name: String, json: String },
    /// Thinking and anything else: the start skeleton plus its deltas.
    Other { block: Value, thinking: String, signature: String },
}

pub(super) async fn step(api: &Api, run: &Run, defs: &[ToolDef], messages: &[Message]) -> Result<Step, String> {
    let mut body = json!({
        "model": api.model,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "tools": tools(defs),
        "messages": wire(messages),
        "stream": true,
    });
    if api.base == ProviderKind::Anthropic.default_base_url() {
        // Caches the conversation so far for the next round of the loop.
        body["cache_control"] = json!({ "type": "ephemeral" });
    }
    let key = api.key.clone().unwrap_or_default();
    let url = format!("{}/v1/messages", api.base);
    let response = http::post(
        &run.cancel,
        "the Anthropic API",
        || api.http.post(&url).header("x-api-key", &key).header("anthropic-version", VERSION),
        &body,
    )
    .await?;

    let mut lines = Lines::new(response);
    let mut blocks: Vec<Block> = vec![];
    let mut stop_reason = String::new();
    let mut stop_details = Value::Null;
    let (mut input_tokens, mut output_tokens) = (0, 0);
    let mut completed = false;
    while let Some(data) = lines.next_sse().await? {
        let event: Value = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match event["type"].as_str().unwrap_or("") {
            "message_start" => {
                let u = &event["message"]["usage"];
                input_tokens = u["input_tokens"].as_u64().unwrap_or(0) + u["cache_read_input_tokens"].as_u64().unwrap_or(0) + u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            }
            "content_block_start" => {
                let index = event["index"].as_u64().unwrap_or(blocks.len() as u64) as usize;
                let b = &event["content_block"];
                let block = match b["type"].as_str().unwrap_or("") {
                    "text" => {
                        let t = b["text"].as_str().unwrap_or("").to_string();
                        run.text(&t);
                        Block::Text(t)
                    }
                    "tool_use" => {
                        Block::Tool { id: b["id"].as_str().unwrap_or("").into(), name: b["name"].as_str().unwrap_or("").into(), json: String::new() }
                    }
                    _ => Block::Other { block: b.clone(), thinking: String::new(), signature: String::new() },
                };
                if index >= blocks.len() {
                    blocks.push(block);
                } else {
                    blocks[index] = block;
                }
            }
            "content_block_delta" => {
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                let d = &event["delta"];
                match (blocks.get_mut(index), d["type"].as_str().unwrap_or("")) {
                    (Some(Block::Text(t)), "text_delta") => {
                        let delta = d["text"].as_str().unwrap_or("");
                        t.push_str(delta);
                        run.text(delta);
                    }
                    (Some(Block::Tool { json, .. }), "input_json_delta") => json.push_str(d["partial_json"].as_str().unwrap_or("")),
                    (Some(Block::Other { thinking, .. }), "thinking_delta") => thinking.push_str(d["thinking"].as_str().unwrap_or("")),
                    (Some(Block::Other { signature, .. }), "signature_delta") => signature.push_str(d["signature"].as_str().unwrap_or("")),
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(r) = event["delta"]["stop_reason"].as_str() {
                    stop_reason = r.to_string();
                }
                stop_details = event["delta"]["stop_details"].clone();
                output_tokens = event["usage"]["output_tokens"].as_u64().unwrap_or(output_tokens);
            }
            "message_stop" => {
                completed = true;
                break;
            }
            "error" => {
                return Err(format!("Anthropic API error: {}", event["error"]["message"].as_str().unwrap_or("unknown")));
            }
            _ => {}
        }
    }
    run.usage(input_tokens, output_tokens);
    if !completed {
        return Err("The Anthropic response stopped before it was complete. Try again.".into());
    }
    match stop_reason.as_str() {
        "max_tokens" => return Err("The reply reached its length limit; no unfinished command was run. Ask for less at a time.".into()),
        "refusal" => {
            let why = stop_details["explanation"].as_str().map(|e| format!(" ({e})")).unwrap_or_default();
            return Err(format!("The model declined this request{why}."));
        }
        _ => {}
    }

    let mut parts = vec![];
    let mut calls = vec![];
    for block in blocks {
        match block {
            Block::Text(text) => parts.push(Part::Text { text }),
            Block::Tool { id, name, json } => {
                let input = parse_args(&json);
                parts.push(Part::ToolUse { id: id.clone(), name: name.clone(), input: input.clone().unwrap_or_else(|_| json!({})) });
                calls.push(Call { id, name, input });
            }
            Block::Other { mut block, thinking, signature } => {
                if block["type"] == "thinking" {
                    block["thinking"] = json!(format!("{}{thinking}", block["thinking"].as_str().unwrap_or("")));
                    if !signature.is_empty() {
                        block["signature"] = json!(signature);
                    }
                }
                parts.push(Part::Opaque { provider: ProviderKind::Anthropic, block });
            }
        }
    }
    if stop_reason != "tool_use" {
        calls.clear();
    }
    Ok(Step { parts, calls })
}
