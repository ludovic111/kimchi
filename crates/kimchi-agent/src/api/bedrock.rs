//! Amazon Bedrock's ConverseStream API, with tools, written without the AWS SDK.
//!
//! `POST https://bedrock-runtime.{region}.amazonaws.com/model/{modelId}/converse-stream`
//! (docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html, 2026-10).
//! The answer is an AWS event stream ([`crate::eventstream`]) of `messageStart`,
//! `contentBlockStart`, `contentBlockDelta` (text, tool input as partial JSON, reasoning),
//! `contentBlockStop`, `messageStop` and `metadata` events, or an exception event.
//!
//! Credentials, the first found: the key saved in kimchi (a Bedrock API key, or access keys as
//! JSON), `AWS_BEARER_TOKEN_BEDROCK`, `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, then the
//! AWS CLI's `~/.aws/credentials` profile (`AWS_PROFILE`, else `default`). API keys go as a
//! bearer token; access keys sign each request with SigV4 ([`crate::sigv4`]). The region is
//! `settings.agent.baseUrl` when it holds one (`eu-west-1`, or a whole endpoint URL), else
//! `AWS_REGION`, `AWS_DEFAULT_REGION`, the profile's region in `~/.aws/config`, `us-east-1`.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Api, Call, Step, parse_args};
use crate::eventstream::Decoder;
use crate::sigv4::{self, Credentials};
use crate::tools::ToolSet;
use crate::{Message, Part, ProviderKind, Role, Run};

/// How requests are authorised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    Bearer(String),
    Keys(Credentials),
}

/// Where Bedrock is reached and with what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub auth: Auth,
    pub region: String,
    /// `https://bedrock-runtime.{region}.amazonaws.com`, or the endpoint given.
    pub endpoint: String,
    /// Where the credentials came from, for the status line.
    pub source: String,
}

/// The saved key: a Bedrock API key as it is, or access keys as JSON
/// (`{"accessKeyId": …, "secretAccessKey": …, "sessionToken": …}`).
pub fn parse_saved(saved: &str) -> Option<Auth> {
    let saved = saved.trim();
    if saved.starts_with('{') {
        let v: Value = serde_json::from_str(saved).ok()?;
        if let Some(k) = v["apiKey"].as_str().filter(|k| !k.trim().is_empty()) {
            return Some(Auth::Bearer(k.trim().to_string()));
        }
        let id = v["accessKeyId"].as_str()?.trim();
        let secret = v["secretAccessKey"].as_str()?.trim();
        if id.is_empty() || secret.is_empty() {
            return None;
        }
        let token = v["sessionToken"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        return Some(Auth::Keys(Credentials { access_key_id: id.into(), secret_access_key: secret.into(), session_token: token }));
    }
    (!saved.is_empty()).then(|| Auth::Bearer(saved.to_string()))
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn aws_dir() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default().join(".aws")
}

/// `key = value` lines of one `[section]` of an AWS ini file.
pub(crate) fn ini_section(text: &str, section: &str) -> Vec<(String, String)> {
    let mut inside = false;
    let mut out = vec![];
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(h) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = h.trim() == section;
        } else if inside && let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    out
}

fn profile() -> String {
    env("AWS_PROFILE").unwrap_or_else(|| "default".into())
}

/// Access keys of the AWS CLI's profile, if it has plain keys (not SSO or a role).
fn profile_credentials() -> Option<Credentials> {
    let path = env("AWS_SHARED_CREDENTIALS_FILE").map(PathBuf::from).unwrap_or_else(|| aws_dir().join("credentials"));
    let text = std::fs::read_to_string(path).ok()?;
    let s = ini_section(&text, &profile());
    let get = |k: &str| s.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
    Some(Credentials { access_key_id: get("aws_access_key_id")?, secret_access_key: get("aws_secret_access_key")?, session_token: get("aws_session_token") })
}

fn profile_region() -> Option<String> {
    let path = env("AWS_CONFIG_FILE").map(PathBuf::from).unwrap_or_else(|| aws_dir().join("config"));
    let text = std::fs::read_to_string(path).ok()?;
    let p = profile();
    let section = if p == "default" { p } else { format!("profile {p}") };
    ini_section(&text, &section).into_iter().find(|(k, _)| k == "region").map(|(_, v)| v)
}

/// A region code like `us-east-1`.
fn is_region(s: &str) -> bool {
    !s.is_empty() && s.len() < 32 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && s.contains('-')
}

/// Region and endpoint from `settings.agent.baseUrl` (a region, or an endpoint URL).
pub fn place(base_setting: &str) -> (String, String) {
    let b = base_setting.trim().trim_end_matches('/');
    if b.contains("://") {
        // `https://bedrock-runtime.eu-west-1.amazonaws.com` (or a VPC endpoint naming it).
        let region = b.split(['.', '/']).find(|p| is_region(p) && p.matches('-').count() >= 2).map(str::to_string).or_else(default_region).unwrap_or_else(|| "us-east-1".into());
        return (region, b.to_string());
    }
    let region = if is_region(b) { b.to_string() } else { default_region().unwrap_or_else(|| "us-east-1".into()) };
    let endpoint = format!("https://bedrock-runtime.{region}.amazonaws.com");
    (region, endpoint)
}

fn default_region() -> Option<String> {
    env("AWS_REGION").or_else(|| env("AWS_DEFAULT_REGION")).or_else(profile_region)
}

/// The credentials to use, or what to do to get some.
pub fn resolve(saved: Option<&str>, base_setting: &str) -> Result<Target, String> {
    let (region, endpoint) = place(base_setting);
    let found = saved
        .and_then(parse_saved)
        .map(|a| (a, "the key saved in kimchi".to_string()))
        .or_else(|| env("AWS_BEARER_TOKEN_BEDROCK").map(|k| (Auth::Bearer(k), "AWS_BEARER_TOKEN_BEDROCK".into())))
        .or_else(|| {
            Some((
                Auth::Keys(Credentials { access_key_id: env("AWS_ACCESS_KEY_ID")?, secret_access_key: env("AWS_SECRET_ACCESS_KEY")?, session_token: env("AWS_SESSION_TOKEN") }),
                "AWS_ACCESS_KEY_ID from the environment".into(),
            ))
        })
        .or_else(|| profile_credentials().map(|c| (Auth::Keys(c), format!("the AWS CLI's \"{}\" profile", profile()))));
    let Some((auth, source)) = found else {
        return Err("No AWS credentials. Paste a Bedrock API key (or access keys) in Settings › Agent, set AWS_BEARER_TOKEN_BEDROCK, or sign in with the AWS CLI (`aws configure`).".into());
    };
    Ok(Target { auth, region, endpoint, source })
}

/// The default model for a region: Claude Sonnet through the region's inference profile.
pub fn default_model(region: &str) -> String {
    let geo = match region.split('-').next().unwrap_or("") {
        "us" => "us",
        "eu" => "eu",
        _ => "global",
    };
    format!("{geo}.anthropic.claude-sonnet-5-5")
}

impl Target {
    /// Headers for one request (`path` as sent, `body` the exact bytes).
    pub fn headers(&self, method: &str, path: &str, query: &str, body: &[u8]) -> Vec<(String, String)> {
        let content: &[(&str, &str)] = if method == "POST" { &[("content-type", "application/json")] } else { &[] };
        match &self.auth {
            Auth::Bearer(k) => {
                let mut h: Vec<(String, String)> = content.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                h.push(("authorization".into(), format!("Bearer {k}")));
                h
            }
            Auth::Keys(c) => {
                let host = self.endpoint.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").to_string();
                let req = sigv4::Request { method, host: &host, path, query, headers: content, body };
                sigv4::sign(&req, c, &self.region, "bedrock", chrono::Utc::now())
            }
        }
    }
}

fn tools(set: &ToolSet) -> Value {
    let tools: Vec<Value> = set.defs.iter().map(|t| json!({ "toolSpec": { "name": t.name, "description": t.description, "inputSchema": { "json": t.schema } } })).collect();
    json!({ "tools": tools, "toolChoice": { "auto": {} } })
}

/// The conversation as Converse messages: roles alternate (neighbours with the same role are
/// merged), reasoning blocks go back to Bedrock as they came.
pub(super) fn wire(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    for m in messages {
        let content: Vec<Value> = m
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } if text.is_empty() => None,
                Part::Text { text } => Some(json!({ "text": text })),
                Part::ToolUse { id, name, input } => Some(json!({ "toolUse": { "toolUseId": id, "name": name, "input": input } })),
                Part::ToolResult { id, output, is_error, .. } => {
                    Some(json!({ "toolResult": { "toolUseId": id, "content": [{ "text": output }], "status": if *is_error { "error" } else { "success" } } }))
                }
                Part::Opaque { provider: ProviderKind::Bedrock, block } => Some(block.clone()),
                Part::Opaque { .. } => None,
            })
            .collect();
        let role = if m.role == Role::User { "user" } else { "assistant" };
        match out.last_mut() {
            Some(last) if last["role"] == role => {
                if let Some(c) = last["content"].as_array_mut() {
                    c.extend(content);
                }
            }
            _ => out.push(json!({ "role": role, "content": content })),
        }
    }
    for m in &mut out {
        if m["content"].as_array().is_some_and(|c| c.is_empty()) {
            m["content"] = json!([{ "text": "…" }]);
        }
    }
    out
}

enum Block {
    Text(String),
    Tool { id: String, name: String, json: String },
    Reasoning { text: String, signature: String, redacted: String },
}

pub(super) async fn step(api: &Api, run: &Run, set: &ToolSet, messages: &[Message]) -> Result<Step, String> {
    let target = api.bedrock.as_ref().ok_or("Bedrock isn't set up.")?;
    let mut body = json!({
        "system": [{ "text": set.system_prompt() }],
        "messages": wire(messages),
        "toolConfig": tools(set),
    });
    // Claude's default output length on Bedrock is short for a tool loop.
    if api.model.contains("anthropic.") {
        body["inferenceConfig"] = json!({ "maxTokens": 32_000 });
    }
    let path = format!("/model/{}/converse-stream", sigv4::encode(&api.model, true));
    let url = format!("{}{path}", target.endpoint);
    let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let label = format!("Amazon Bedrock ({})", target.region);
    let mut response = crate::http::post_bytes(
        &run.cancel,
        &label,
        || {
            let mut r = api.http.post(&url).header("accept", "application/vnd.amazon.eventstream");
            for (k, v) in target.headers("POST", &path, "", &bytes) {
                r = r.header(k, v);
            }
            r
        },
        &bytes,
    )
    .await?;

    let mut decoder = Decoder::default();
    let mut blocks: Vec<(usize, Block)> = vec![];
    let mut stop = String::new();
    let mut done = false;
    'read: loop {
        let chunk = match response.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(format!("The response stream broke off: {e}")),
        };
        decoder.push(&chunk);
        while let Some(msg) = decoder.next()? {
            let payload: Value = serde_json::from_slice(&msg.payload).unwrap_or(Value::Null);
            if msg.header(":message-type") != Some("event") {
                let kind = msg.header(":exception-type").or(msg.header(":error-code")).unwrap_or("error");
                let why = payload["message"].as_str().or(payload["Message"].as_str()).or(msg.header(":error-message")).unwrap_or("unknown");
                return Err(format!("{label} error ({kind}): {why}"));
            }
            let index = payload["contentBlockIndex"].as_u64().unwrap_or(0) as usize;
            let slot = |blocks: &mut Vec<(usize, Block)>, make: fn() -> Block| -> usize {
                match blocks.iter().position(|(i, _)| *i == index) {
                    Some(p) => p,
                    None => {
                        blocks.push((index, make()));
                        blocks.len() - 1
                    }
                }
            };
            match msg.header(":event-type").unwrap_or("") {
                "messageStart" => {}
                "contentBlockStart" => {
                    if let Some(t) = payload["start"].get("toolUse") {
                        let name = t["name"].as_str().unwrap_or("").to_string();
                        run.status(format!("Running {}…", crate::cli::tool_label(&name)));
                        let p = slot(&mut blocks, || Block::Text(String::new()));
                        blocks[p].1 = Block::Tool { id: t["toolUseId"].as_str().unwrap_or("").into(), name, json: String::new() };
                    }
                }
                "contentBlockDelta" => {
                    let d = &payload["delta"];
                    if let Some(t) = d["text"].as_str() {
                        let p = slot(&mut blocks, || Block::Text(String::new()));
                        if let Block::Text(s) = &mut blocks[p].1 {
                            s.push_str(t);
                            run.text(t);
                        }
                    } else if let Some(i) = d["toolUse"]["input"].as_str() {
                        let p = slot(&mut blocks, || Block::Tool { id: String::new(), name: String::new(), json: String::new() });
                        if let Block::Tool { json, .. } = &mut blocks[p].1 {
                            json.push_str(i);
                        }
                    } else if let Some(r) = d.get("reasoningContent") {
                        let p = slot(&mut blocks, || Block::Reasoning { text: String::new(), signature: String::new(), redacted: String::new() });
                        if let Block::Reasoning { text, signature, redacted } = &mut blocks[p].1 {
                            text.push_str(r["text"].as_str().unwrap_or(""));
                            signature.push_str(r["signature"].as_str().unwrap_or(""));
                            redacted.push_str(r["redactedContent"].as_str().unwrap_or(""));
                        }
                    }
                }
                "messageStop" => stop = payload["stopReason"].as_str().unwrap_or("").to_string(),
                "metadata" => {
                    let u = &payload["usage"];
                    run.usage(u["inputTokens"].as_u64().unwrap_or(0) + u["cacheReadInputTokens"].as_u64().unwrap_or(0), u["outputTokens"].as_u64().unwrap_or(0));
                    done = true;
                    break 'read;
                }
                _ => {}
            }
        }
    }
    if !done && stop.is_empty() {
        return Err(format!("{label}: the connection closed before the reply finished. Try again."));
    }
    match stop.as_str() {
        "max_tokens" | "model_context_window_exceeded" => return Err("The reply reached its length limit; no unfinished command was run. Ask for less at a time.".into()),
        "guardrail_intervened" | "content_filtered" => return Err("The model declined this request (a Bedrock guardrail or content filter stopped it).".into()),
        "malformed_model_output" | "malformed_tool_use" => return Err("The model sent a command Bedrock couldn't read. Try again, or choose another model.".into()),
        _ => {}
    }
    blocks.sort_by_key(|(i, _)| *i);
    let mut parts = vec![];
    let mut calls = vec![];
    for (_, b) in blocks {
        match b {
            Block::Text(text) if text.is_empty() => {}
            Block::Text(text) => parts.push(Part::Text { text }),
            Block::Tool { id, name, json } => {
                let input = parse_args(&json);
                parts.push(Part::ToolUse { id: id.clone(), name: name.clone(), input: input.clone().unwrap_or_else(|_| json!({})) });
                calls.push(Call { id, name, input });
            }
            Block::Reasoning { text, signature, redacted } => {
                let block = if !redacted.is_empty() {
                    json!({ "reasoningContent": { "redactedContent": redacted } })
                } else {
                    let mut r = json!({ "text": text });
                    if !signature.is_empty() {
                        r["signature"] = json!(signature);
                    }
                    json!({ "reasoningContent": { "reasoningText": r } })
                };
                parts.push(Part::Opaque { provider: ProviderKind::Bedrock, block });
            }
        }
    }
    if stop != "tool_use" {
        calls.clear();
    }
    Ok(Step { parts, calls })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_keys_take_both_shapes() {
        assert_eq!(parse_saved("ABSK123"), Some(Auth::Bearer("ABSK123".into())));
        assert_eq!(parse_saved(r#"{"apiKey":"ABSK9"}"#), Some(Auth::Bearer("ABSK9".into())));
        let k = parse_saved(r#"{"accessKeyId":"AKIA1","secretAccessKey":"s3cr3t"}"#).unwrap();
        assert_eq!(k, Auth::Keys(Credentials { access_key_id: "AKIA1".into(), secret_access_key: "s3cr3t".into(), session_token: None }));
        assert_eq!(parse_saved(r#"{"accessKeyId":"AKIA1"}"#), None);
    }

    #[test]
    fn the_address_field_takes_a_region_or_an_endpoint() {
        assert_eq!(place("eu-west-3"), ("eu-west-3".into(), "https://bedrock-runtime.eu-west-3.amazonaws.com".into()));
        assert_eq!(place("https://bedrock-runtime.ap-southeast-2.amazonaws.com/").0, "ap-southeast-2");
        assert_eq!(default_model("eu-central-1"), "eu.anthropic.claude-sonnet-5-5");
        assert_eq!(default_model("ap-northeast-1"), "global.anthropic.claude-sonnet-5-5");
    }

    #[test]
    fn aws_ini_sections_are_read() {
        let t = "[default]\naws_access_key_id = A\n[profile work]\nregion=eu-west-1\n# x\n";
        assert_eq!(ini_section(t, "default"), [("aws_access_key_id".to_string(), "A".to_string())]);
        assert_eq!(ini_section(t, "profile work"), [("region".to_string(), "eu-west-1".to_string())]);
    }
}
