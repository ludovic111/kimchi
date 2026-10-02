use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use kimchi_control::{Session, SessionOptions, Source};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::*;

fn session(dir: &std::path::Path) -> Arc<Session> {
    Session::new(SessionOptions { data_dir: Some(dir.join("data")), config_dir: Some(dir.join("config")), secrets: None, headless: true }).unwrap()
}

async fn with_project(dir: &std::path::Path) -> Arc<Session> {
    let s = session(dir);
    kimchi_control::call(&s, Source::Window, "project.create", json!({ "name": "Test" })).await.unwrap();
    s
}

/// Answers each request with the next body, in order (the last one repeats).
struct Script {
    bodies: Vec<String>,
    content_type: &'static str,
    next: AtomicUsize,
}

impl Respond for Script {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let i = self.next.fetch_add(1, Ordering::SeqCst).min(self.bodies.len() - 1);
        ResponseTemplate::new(200).insert_header("content-type", self.content_type).set_body_string(self.bodies[i].clone())
    }
}

async fn mock(route: &str, content_type: &'static str, bodies: Vec<String>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path(route)).respond_with(Script { bodies, content_type, next: AtomicUsize::new(0) }).mount(&server).await;
    server
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap_or("message"))).collect()
}

fn openai_sse(chunks: &[Value]) -> String {
    let mut s: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    s.push_str("data: [DONE]\n\n");
    s
}

/// Every event of a run, up to and including the terminal one.
async fn collect(run: &mut AgentRun) -> Vec<AgentEvent> {
    let mut out = vec![];
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(e) = run.next_event().await {
            out.push(e);
        }
    })
    .await
    .expect("the run finished");
    out
}

/// The events a panel draws, without status lines and token counts.
fn visible(events: &[AgentEvent]) -> Vec<&AgentEvent> {
    events.iter().filter(|e| !matches!(e, AgentEvent::Status { .. } | AgentEvent::Usage { .. })).collect()
}

fn anthropic_text(text: &str) -> String {
    sse(&[
        json!({ "type": "message_start", "message": { "id": "msg_2", "role": "assistant", "content": [], "usage": { "input_tokens": 200, "output_tokens": 1 } } }),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 8 } }),
        json!({ "type": "message_stop" }),
    ])
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_tool_call_lands_in_the_project_and_the_run_reverts() {
    let dir = tempfile::tempdir().unwrap();
    let s = with_project(dir.path()).await;
    s.set_secret("anthropic", Some("sk-ant-test")).unwrap();
    let first = sse(&[
        json!({ "type": "message_start", "message": { "id": "msg_1", "role": "assistant", "content": [], "usage": { "input_tokens": 120, "output_tokens": 1 } } }),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig-abc" } }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
        json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Adding a title." } }),
        json!({ "type": "content_block_stop", "index": 1 }),
        json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "clip_addText", "input": {} } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"text\": \"Hel" } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "lo\", \"start\": 0}" } }),
        json!({ "type": "content_block_stop", "index": 2 }),
        json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 40 } }),
        json!({ "type": "message_stop" }),
    ]);
    let server = mock("/v1/messages", "text/event-stream", vec![first, anthropic_text("Added the title."), anthropic_text("Sure.")]).await;
    let config = AgentConfig { base_url: server.uri(), ..AgentConfig::new(ProviderKind::Anthropic) };

    let mut run = Agent::start(&s, config.clone(), "Put Hello at the start", Conversation::new());
    let events = collect(&mut run).await;
    let shown = visible(&events);
    assert_eq!(shown.len(), 4, "{shown:#?}");
    assert!(matches!(shown[0], AgentEvent::Text { delta } if delta == "Adding a title."));
    match shown[1] {
        AgentEvent::Command { record, result } => {
            assert_eq!(record.command, "clip.addText");
            assert_eq!(record.source, Source::Agent);
            assert!(record.ok && record.mutates);
            assert!(record.seq > 0);
            assert!(result.is_some());
        }
        e => panic!("expected a command card, got {e:?}"),
    }
    assert!(matches!(shown[2], AgentEvent::Text { delta } if delta == "\n\nAdded the title."));
    let (checkpoint, conversation) = match shown[3] {
        AgentEvent::Done { summary, checkpoint, changes, conversation } => {
            assert_eq!(summary, "Added the title.");
            assert_eq!(*changes, 1);
            (checkpoint.expect("a checkpoint before the change"), conversation.clone())
        }
        e => panic!("expected Done, got {e:?}"),
    };
    let texts: Vec<String> = s.project().unwrap().clips().filter_map(|c| serde_json::to_value(c).ok()).map(|c| c.to_string()).collect();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("Hello"), "{texts:?}");
    assert_eq!(run.checkpoint(), Some(checkpoint));

    // What the model was sent back: the thinking block verbatim, then the tool result.
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers.get("x-api-key").unwrap(), "sk-ant-test");
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(second["messages"][1]["content"][0], json!({ "type": "thinking", "thinking": "", "signature": "sig-abc" }));
    assert_eq!(second["messages"][1]["content"][2]["input"], json!({ "text": "Hello", "start": 0 }));
    let result = &second["messages"][2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "toolu_1");
    assert_eq!(result["is_error"], false);
    assert_eq!(second["tools"].as_array().unwrap().len(), tool_defs().len());

    // A follow-up continues the thread.
    let mut next = Agent::start(&s, config, "Thanks", conversation);
    let events = collect(&mut next).await;
    assert!(matches!(events.last(), Some(AgentEvent::Done { summary, checkpoint: None, changes: 0, .. }) if summary == "Sure."));
    let third: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[2].body).unwrap();
    assert_eq!(third["messages"].as_array().unwrap().len(), 5);
    assert_eq!(third["messages"][4]["content"][0]["text"], "Thanks");

    // Revert this run.
    revert(&s, checkpoint).await.unwrap();
    assert_eq!(s.project().unwrap().clips().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_permission_goes_back_to_the_model_as_a_tool_error() {
    let dir = tempfile::tempdir().unwrap();
    let s = with_project(dir.path()).await;
    s.set_secret("openai", Some("sk-test")).unwrap();
    let first = openai_sse(&[
        json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "content": "Turning off update checks." } }] }),
        json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{ "index": 0, "id": "call_1", "type": "function", "function": { "name": "app_setSetting", "arguments": "" } }] } }] }),
        json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "{\"key\":\"updates.checkOnStart\",\"value\":false}" } }] } }] }),
        json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }] }),
    ]);
    let second = openai_sse(&[
        json!({ "choices": [{ "index": 0, "delta": { "content": "I can't: the settings permission is off." } }] }),
        json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] }),
    ]);
    let server = mock("/chat/completions", "text/event-stream", vec![first, second]).await;
    let config = AgentConfig { base_url: server.uri(), ..AgentConfig::new(ProviderKind::OpenAi) };
    let mut run = Agent::start(&s, config, "Stop checking for updates", Conversation::new());
    let events = collect(&mut run).await;
    let shown = visible(&events);
    assert_eq!(shown.len(), 4, "{shown:#?}");
    match shown[1] {
        AgentEvent::Command { record, result } => {
            assert_eq!(record.command, "app.setSetting");
            assert!(!record.ok);
            assert!(record.error.as_deref().unwrap().contains("\"settings\" permission"), "{record:?}");
            assert!(result.is_none());
        }
        e => panic!("expected a command card, got {e:?}"),
    }
    assert!(matches!(shown[3], AgentEvent::Done { checkpoint: None, changes: 0, .. }), "{:?}", shown[3]);
    assert!(s.settings().updates.check_on_start);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers.get("authorization").unwrap(), "Bearer sk-test");
    let body: Value = serde_json::from_slice(&requests[1].body).unwrap();
    let tool = body["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").expect("a tool message");
    assert_eq!(tool["tool_call_id"], "call_1");
    assert!(tool["content"].as_str().unwrap().contains("permission"), "{tool}");
    assert_eq!(body["messages"][0]["role"], "system");
}

#[tokio::test(flavor = "multi_thread")]
async fn ollama_runs_tools_locally() {
    let dir = tempfile::tempdir().unwrap();
    let s = with_project(dir.path()).await;
    let first = [
        json!({ "message": { "role": "assistant", "content": "", "tool_calls": [{ "function": { "name": "timeline_addMarker", "arguments": { "time": 2, "label": "Drop" } } }] }, "done": false }),
        json!({ "message": { "role": "assistant", "content": "" }, "done": true, "done_reason": "stop", "prompt_eval_count": 10, "eval_count": 5 }),
    ]
    .map(|v| v.to_string())
    .join("\n");
    let second = json!({ "message": { "role": "assistant", "content": "Marker added." }, "done": true }).to_string();
    let server = mock("/api/chat", "application/x-ndjson", vec![first, second]).await;
    let config = AgentConfig { base_url: server.uri(), model: "qwen3".into(), ..AgentConfig::new(ProviderKind::Ollama) };
    let mut run = Agent::start(&s, config, "Mark the drop at 2 s", Conversation::new());
    let events = collect(&mut run).await;
    assert!(matches!(events.last(), Some(AgentEvent::Done { changes: 1, summary, .. }) if summary == "Marker added."), "{events:#?}");
    assert_eq!(s.project().unwrap().markers[0].label, "Drop");
    let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[1].body).unwrap();
    let tool = body["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(tool["tool_name"], "timeline_addMarker");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_a_run_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let s = with_project(dir.path()).await;
    s.set_secret("anthropic", Some("k")).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60))).mount(&server).await;
    let config = AgentConfig { base_url: server.uri(), ..AgentConfig::new(ProviderKind::Anthropic) };
    let mut run = Agent::start(&s, config, "Do something slow", Conversation::new());
    assert!(matches!(run.next_event().await, Some(AgentEvent::Status { .. })));
    run.cancel();
    let events = tokio::time::timeout(Duration::from_secs(5), collect(&mut run)).await.expect("cancelled promptly");
    assert!(matches!(events.last(), Some(AgentEvent::Cancelled { checkpoint: None, changes: 0 })), "{events:?}");
    assert!(run.is_finished());
    // The thread keeps the request, so a follow-up has the context.
    assert_eq!(run.conversation().messages.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_keys_and_disabled_agents_are_explained() {
    let dir = tempfile::tempdir().unwrap();
    let s = with_project(dir.path()).await;
    // Point at a dead address so a key from the environment can't reach anything.
    let config = AgentConfig { base_url: "http://127.0.0.1:9".into(), ..AgentConfig::new(ProviderKind::Anthropic) };
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        let mut run = Agent::start(&s, config.clone(), "Hi", Conversation::new());
        let events = collect(&mut run).await;
        assert!(matches!(events.last(), Some(AgentEvent::Error { message, .. }) if message.contains("No Anthropic API key")), "{events:?}");
    }
    s.update_settings(|st| st.agent.permissions.enabled = false).unwrap();
    let mut run = Agent::start(&s, config, "Hi", Conversation::new());
    let events = collect(&mut run).await;
    assert!(matches!(events.last(), Some(AgentEvent::Error { message, .. }) if message.contains("turned off")), "{events:?}");
}

#[test]
fn tools_cover_the_registry_except_person_only_commands() {
    let defs = tool_defs();
    assert!(defs.iter().any(|t| t.name == "project_overview"));
    assert!(defs.iter().all(|t| t.name != "generate_setKey"));
    let mut names: Vec<&str> = defs.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), defs.len());
    for t in &defs {
        assert!(t.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && t.name.len() <= 64, "{}", t.name);
        assert_eq!(t.schema["type"], "object");
        assert_eq!(tools::spec_for_tool(&t.name).unwrap().name, t.command);
    }
    assert_eq!(tools::spec_for_tool("mcp__kimchi__clip_addText").unwrap().name, "clip.addText");
    let long = "é".repeat(TOOL_OUTPUT_LIMIT);
    let (out, err) = tools::tool_output(&Ok(json!(long)));
    assert!(!err && out.contains("truncated") && out.len() < TOOL_OUTPUT_LIMIT + 200);
}

#[test]
fn settings_choose_the_provider() {
    let mut a = kimchi_control::settings::AgentSettings::default();
    assert_eq!(AgentConfig::from_settings(&a).provider, ProviderKind::ClaudeCode);
    a.provider = "anthropic".into();
    let c = AgentConfig::from_settings(&a);
    assert_eq!((c.provider, c.model(), c.base_url()), (ProviderKind::Anthropic, "claude-sonnet-5-5".to_string(), "https://api.anthropic.com".to_string()));
    a.provider = "ollama".into();
    a.base_url = "http://box:11434/".into();
    assert_eq!(AgentConfig::from_settings(&a).base_url(), "http://box:11434");
    assert_eq!(serde_json::to_value(ProviderKind::OpenAi).unwrap(), "openai");
}

#[test]
fn a_stopped_turn_is_closed_before_the_next() {
    let mut c = Conversation::new();
    c.messages.push(Message::user("go"));
    c.messages.push(Message {
        role: Role::Assistant,
        parts: vec![
            Part::ToolUse { id: "a".into(), name: "clip_list".into(), input: json!({}) },
            Part::ToolUse { id: "b".into(), name: "clip_list".into(), input: json!({}) },
        ],
    });
    c.messages.push(Message { role: Role::User, parts: vec![Part::ToolResult { id: "a".into(), name: "clip_list".into(), output: "[]".into(), is_error: false }] });
    c.prepare_turn();
    assert_eq!(c.messages.len(), 3);
    assert!(matches!(&c.messages[2].parts[1], Part::ToolResult { id, is_error: true, .. } if id == "b"));
}

#[test]
fn cli_invocations_attach_kimchi_only() {
    let args = cli::claude_args(std::path::Path::new("/tmp/mcp.json"), "", Some("sess-1"));
    let joined = args.join(" ");
    for flag in ["-p", "--output-format stream-json", "--strict-mcp-config", "--allowedTools mcp__kimchi__*", "--mcp-config /tmp/mcp.json", "--resume sess-1"] {
        assert!(joined.contains(flag), "{flag} in {joined}");
    }
    assert!(!joined.contains("--model"));
    let live = cli::Live { mcp: "/Apps/kimchi \"x\"/kimchi-mcp".into(), control: "/data/control.json".into() };
    let args = cli::codex_args(&live, "gpt-5", Some("thread-9"));
    assert_eq!(&args[..2], ["exec", "--json"]);
    assert!(args.contains(&"mcp_servers.kimchi.command=\"/Apps/kimchi \\\"x\\\"/kimchi-mcp\"".to_string()), "{args:?}");
    assert!(args.windows(2).any(|w| w == ["resume", "thread-9"]));
    assert_eq!(args.last().unwrap(), "-");
}

#[cfg(unix)]
fn script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("fake-cli");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn claude_code_stream_json_becomes_events() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let lines = [
        json!({ "type": "system", "subtype": "init", "session_id": "sess-1", "mcp_servers": [{ "name": "kimchi", "status": "connected" }] }),
        json!({ "type": "stream_event", "event": { "type": "message_start" } }),
        json!({ "type": "stream_event", "event": { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hel" } } }),
        json!({ "type": "stream_event", "event": { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "lo" } } }),
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Hello" }] } }),
        json!({ "type": "result", "subtype": "success", "is_error": false, "result": "Hello", "session_id": "sess-1", "usage": { "input_tokens": 3, "output_tokens": 2 } }),
    ];
    let echo: String = lines.iter().map(|l| format!("echo '{l}'\n")).collect();
    let got = dir.path().join("prompt.txt");
    let exe = script(dir.path(), &format!("cat > '{}'\n{echo}", got.display()));
    let (mut run, mut handle) = Run::new(&s, AgentConfig::new(ProviderKind::ClaudeCode), &Conversation::new());
    let (out, result) = cli::run_child(&mut run, tokio::process::Command::new(exe), "the prompt".into(), "Claude Code", cli::parse_claude).await;
    result.unwrap();
    assert_eq!((out.reply.as_str(), out.session_id.as_deref()), ("Hello", Some("sess-1")));
    assert_eq!(std::fs::read_to_string(got).unwrap(), "the prompt");
    drop(run);
    let mut deltas = vec![];
    while let Some(e) = handle.next_event().await {
        if let AgentEvent::Text { delta } = e {
            deltas.push(delta);
        }
    }
    assert_eq!(deltas, ["Hel", "lo"]);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_kills_the_cli_and_its_children() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let pid_file = dir.path().join("pid");
    let exe = script(dir.path(), &format!("sleep 60 &\necho $! > '{}'\nwait", pid_file.display()));
    let (mut run, _handle) = Run::new(&s, AgentConfig::new(ProviderKind::ClaudeCode), &Conversation::new());
    let child = cli::run_child(&mut run, tokio::process::Command::new(exe), String::new(), "Claude Code", cli::parse_claude);
    assert!(tokio::time::timeout(Duration::from_millis(800), child).await.is_err(), "the fake CLI waits");
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    let alive = || std::process::Command::new("kill").args(["-0", &pid]).stderr(std::process::Stdio::null()).status().unwrap().success();
    for _ in 0..40 {
        if !alive() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the CLI's child {pid} survived the cancel");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_cli_without_kimchi_tools_stops_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let init = json!({ "type": "system", "subtype": "init", "session_id": "s", "mcp_servers": [{ "name": "kimchi", "status": "failed" }] });
    let exe = script(dir.path(), &format!("echo '{init}'\nsleep 60"));
    let (mut run, _handle) = Run::new(&s, AgentConfig::new(ProviderKind::ClaudeCode), &Conversation::new());
    let child = cli::run_child(&mut run, tokio::process::Command::new(exe), String::new(), "Claude Code", cli::parse_claude);
    let (_, result) = tokio::time::timeout(Duration::from_secs(5), child).await.expect("stopped at once");
    assert!(result.unwrap_err().contains("couldn't start kimchi-mcp"));
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_status_says_what_is_usable() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "models": [{ "name": "qwen3:8b" }] })))
        .mount(&server)
        .await;
    s.update_settings(|st| {
        st.agent.provider = "ollama".into();
        st.agent.base_url = server.uri();
    })
    .unwrap();
    s.set_secret("openai", Some("sk-test")).unwrap();
    let all = provider_status(&s).await;
    assert_eq!(all.len(), ProviderKind::ALL.len());
    let get = |k| all.iter().find(|p| p.provider == k).unwrap();
    let ollama = get(ProviderKind::Ollama);
    assert!(ollama.ready && ollama.active, "{ollama:?}");
    assert_eq!(ollama.models, ["qwen3:8b"]);
    assert!(get(ProviderKind::OpenAi).ready);
    // No bridge in a headless session: the CLIs can't reach kimchi, whatever is installed.
    let claude = get(ProviderKind::ClaudeCode);
    assert!(!claude.ready && !claude.message.is_empty(), "{claude:?}");
}

/// The whole chain with the real Claude Code (spends a little of the person's
/// plan): `cargo build -p kimchi-mcp && cargo test -p kimchi-agent -- --ignored live_claude_code`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the installed Claude Code against a real model"]
async fn live_claude_code_edits_through_the_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let s = with_project(dir.path()).await;
    let _bridge = kimchi_control::bridge::Server::start(s.clone()).await.unwrap();
    let config = AgentConfig { model: "claude-haiku-4-5".into(), ..AgentConfig::new(ProviderKind::ClaudeCode) };
    let mut run = Agent::start(&s, config.clone(), "Add a title that says Hello at 0 seconds, then reply in one short sentence.", Conversation::new());
    let events = tokio::time::timeout(Duration::from_secs(240), collect(&mut run)).await.unwrap();
    for e in &events {
        eprintln!("{}", serde_json::to_string(e).unwrap());
    }
    let Some(AgentEvent::Done { checkpoint: Some(checkpoint), changes, conversation, .. }) = events.last() else { panic!("{:?}", events.last()) };
    assert!(*changes >= 1);
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Command { record, .. } if record.command == "clip.addText" && record.source == Source::Mcp && record.ok)));
    assert!(s.project().unwrap().clips().count() >= 1);
    let session_id = conversation.cli_session.clone().expect("a Claude Code session to resume").id;
    // The follow-up resumes the same Claude Code session.
    let mut next = Agent::start(&s, config, "What did you just add? Answer in five words.", conversation.clone());
    let events = tokio::time::timeout(Duration::from_secs(240), collect(&mut next)).await.unwrap();
    assert!(matches!(events.last(), Some(AgentEvent::Done { .. })), "{:?}", events.last());
    assert_eq!(next.conversation().cli_session.unwrap().id, session_id);
    revert(&s, *checkpoint).await.unwrap();
    assert_eq!(s.project().unwrap().clips().count(), 0);
}
