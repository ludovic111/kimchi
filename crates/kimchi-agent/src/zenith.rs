//! lsuite agent integration over Zenith's public command interface.
//! Credentials and provider sessions stay in Zenith, not in the Kimchi document.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use serde_json::{Value, json};
use crate::{CliSession, Conversation, Message, ProviderKind, Run};

pub fn executable() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KIMCHI_ZENITH_CLI").map(PathBuf::from).filter(|p| p.is_file()) { return Some(p); }
    kimchi_control::discovery::installed_apps().into_iter().find(|app| app.app == "zenith")
        .and_then(|app| app.cli).filter(|p| p.is_absolute() && p.is_file())
        .or_else(|| std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths)
            .map(|p| p.join(format!("zenith-cli{}", std::env::consts::EXE_SUFFIX))).find(|p| p.is_file())))
}

pub(crate) async fn call(exe: &Path, command: &str, args: Value) -> Result<Value, String> {
    let mut child = tokio::process::Command::new(exe);
    child.arg(command).arg("--json").arg(args.to_string()).stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(30), child.output()).await
        .map_err(|_| format!("zenith timed out on {command}. Check that its server is running."))?
        .map_err(|e| format!("Could not start zenith: {e}"))?;
    if !out.status.success() {
        return Err(format!("zenith {command}: {}", crate::tools::bounded(&String::from_utf8_lossy(&out.stderr), 2000)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("Invalid response from zenith for {command}: {e}"))
}

pub(crate) async fn models() -> Result<Vec<String>, String> {
    let exe = executable().ok_or("Install zenith to use its lsuite agents.")?;
    models_at(&exe).await
}

async fn models_at(exe: &Path) -> Result<Vec<String>, String> {
    let providers = call(exe, "provider.list", json!({})).await?;
    Ok(providers.as_array().into_iter().flatten().filter(|p| p["enabled"] != false)
        .flat_map(|p| p["models"].as_array().into_iter().flatten().filter_map(|m| {
            Some(format!("{}/{}", p["provider"].as_str()?, m["model"].as_str()?))
        })).collect())
}

fn selection(model: &str, args: &mut Value) {
    if let Some((provider, model)) = model.split_once('/') {
        args["provider"] = json!(provider);
        args["model"] = json!(model);
    } else if !model.is_empty() { args["model"] = json!(model); }
}

pub(crate) async fn interrupt(run: &Run) -> Result<(), String> {
    let remote = run.shared.zenith_thread.lock().clone();
    if let Some((exe, id)) = remote {
        call(&exe, "thread.interrupt", json!({"threadId": id})).await?;
        let wait = async {
            loop {
                let state = call(&exe, "thread.get", json!({"threadId": id, "messages": 0})).await?;
                if matches!(state["status"].as_str(), Some("ready" | "failed" | "plan-ready" | "monitoring")) { return Ok(()); }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        };
        return tokio::time::timeout(Duration::from_secs(15), wait).await
            .map_err(|_| "zenith has not stopped yet. Open zenith to check the conversation before sending again.".to_string())?;
        }
    Ok(())
}

pub(crate) async fn run(run: &mut Run, prompt: String, conv: Conversation) -> Result<String, String> {
    let exe = executable().ok_or("zenith was not found. Install it and start its server, then choose zenith again.")?;
    run_at(run, prompt, conv, exe).await
}

fn context_prompt(name: &str, id: kimchi_core::Id, prompt: &str) -> String {
    format!("You are working in kimchi, part of lsuite. Use the installed kimchi MCP tools to edit project '{name}' (id {id}). Check project.overview before editing; stop if another project is open. Use the suite's ryolune tools when audio work needs them. kimchi's command permissions and undo history apply.\n\n{}\n\nRequest:\n{prompt}", crate::SYSTEM_PROMPT)
}

async fn run_at(run: &mut Run, prompt: String, mut conv: Conversation, exe: PathBuf) -> Result<String, String> {
    if run.session.bridge_port().is_none() { return Err("zenith needs kimchi's live bridge. Restart kimchi.".into()); }
    let project = run.session.project()?;
    let workspace = run.session.data_dir.join("agent-workspaces").join(project.id.to_string());
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    let resume = conv.cli_session.as_ref().filter(|s| s.provider == ProviderKind::Zenith).map(|s| s.id.clone());
    let prompt = context_prompt(&project.name, project.id, &crate::context::glance(&run.session).frame(&prompt));
    run.ensure_checkpoint().await;
    conv.prepare_turn();
    conv.messages.push(Message::user(prompt.clone()));
    let id = if let Some(id) = resume {
        *run.shared.zenith_thread.lock() = Some((exe.clone(), id.clone()));
        let mut args = json!({"threadId": id, "prompt": prompt});
        selection(&run.config.model, &mut args);
        call(&exe, "thread.send", args).await?;
        id
    } else {
        let projects = call(&exe, "project.list", json!({})).await?;
        let path = workspace.to_string_lossy();
        let existing = projects.as_array().into_iter().flatten().find(|p| p["path"].as_str() == Some(path.as_ref()));
        let project_id = match existing.and_then(|p| p["projectId"].as_str()) {
            Some(id) => id.to_string(),
            None => call(&exe, "project.add", json!({"path": workspace, "title": format!("kimchi · {}", project.name)})).await?
                ["projectId"].as_str().ok_or("zenith did not return a project id.")?.to_string(),
        };
        // Choose the id before dispatch so cancellation can interrupt even when
        // the command's acknowledgement was lost.
        let id = kimchi_core::Id::new_v4().to_string();
        *run.shared.zenith_thread.lock() = Some((exe.clone(), id.clone()));
        conv.cli_session = Some(CliSession { provider: ProviderKind::Zenith, id: id.clone() });
        run.set_conversation(&conv);
        let mut args = json!({"threadId": id, "projectId": project_id, "prompt": prompt});
        selection(&run.config.model, &mut args);
        call(&exe, "thread.new", args).await?;
        id
    };
    conv.cli_session = Some(CliSession { provider: ProviderKind::Zenith, id: id.clone() });
    run.set_conversation(&conv);
    run.status("Working in zenith…");
    let started = Instant::now();
    let mut reply = String::new();
    // The prompts of this run, including steering, that the newest user message may be.
    let mut sent = vec![prompt.clone()];
    loop {
        steer_queued(run, &exe, &id, &mut sent, &mut conv).await?;
        let state = call(&exe, "thread.get", json!({"threadId": id, "messages": 100})).await?;
        let status = state["status"].as_str().unwrap_or("working");
        let timeline = state["timeline"].as_array().cloned().unwrap_or_default();
        let last_user = timeline.iter().rposition(|m| m["kind"] == "message" && m["role"] == "user");
        // A command acknowledgement can precede the new turn appearing in the
        // snapshot. Never stream or finish with the previous turn's answer.
        if !last_user.is_some_and(|i| timeline[i]["text"].as_str().is_some_and(|t| sent.iter().any(|s| s == t))) {
            if started.elapsed() > Duration::from_secs(30) { return Err("zenith did not show the submitted message. Open zenith to check this conversation.".into()); }
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let text = timeline.iter().skip(last_user.map_or(0, |i| i + 1)).filter(|m| m["kind"] == "message" && m["role"] == "assistant")
            .filter_map(|m| m["text"].as_str()).collect::<Vec<_>>().join("\n\n");
        match text.strip_prefix(&reply) {
            Some(delta) => if !delta.is_empty() { run.text(delta); },
            // A steering message started a new answer.
            None => { run.break_text(); if !text.is_empty() { run.text(&text); } }
        }
        reply = text;
        run.drain_commands(kimchi_control::Source::Mcp);
        match status {
            "approval" => run.status("zenith is waiting for your approval. Open the conversation in zenith."),
            "input" => run.status("zenith has a question for you. Open the conversation in zenith."),
            "failed" => return Err(state["session"]["error"].as_str().unwrap_or("The zenith agent failed. Open zenith for details.").into()),
            "ready" | "plan-ready" | "monitoring" if started.elapsed() > Duration::from_secs(1) => break,
            _ => {}
        }
        let shared = run.shared.clone();
        tokio::select! {
            _ = shared.steered.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(750)) => {}
        }
    }
    conv.messages.push(Message::assistant(reply.clone()));
    run.set_conversation(&conv);
    Ok(reply)
}

/// Hands queued steering messages to the running Zenith turn with `thread.steer`. A Zenith
/// without it (older than 0.3) gets `thread.send`, which joins a running turn the same way.
async fn steer_queued(run: &mut Run, exe: &Path, id: &str, sent: &mut Vec<String>, conv: &mut Conversation) -> Result<(), String> {
    let messages: Vec<String> = run.shared.steering.lock().drain(..).collect();
    for message in messages {
        let args = json!({"threadId": id, "prompt": message});
        match call(exe, "thread.steer", args.clone()).await {
            Ok(_) => {}
            Err(e) if e.contains("unknown command") => { call(exe, "thread.send", args).await?; }
            // The turn ended between the click and the call: start the next one.
            Err(e) if e.contains("thread.send") => { call(exe, "thread.send", args).await?; }
            Err(e) if e.contains("thread.approve") || e.contains("thread.answer") => {
                run.status("zenith is waiting for your approval or answer. Reply in zenith, then steer again.");
                continue;
            }
            Err(e) => return Err(e),
        }
        run.status("zenith is applying your steering message…");
        conv.messages.push(Message::user(message.clone()));
        run.set_conversation(conv);
        sent.push(message);
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mock_cli(dir: &Path) -> PathBuf {
        let exe = dir.join("zenith-cli");
        std::fs::write(&exe, r##"#!/bin/sh
cd "$(dirname "$0")" || exit 1
printf '%s\n%s\n' "$1" "$3" >> calls
case "$1" in
  thread.new|thread.send) printf '%s' "$3" > submitted.json; printf '{}' ;;
  thread.interrupt) touch interrupted; printf '{}' ;;
  thread.steer)
    if [ -f old-zenith ]; then printf 'unknown command "thread.steer" (see `zenith-cli list`)' >&2; exit 1
    elif [ -f pending ]; then printf 'an approval is pending: answer it with thread.approve / thread.answer first' >&2; exit 1
    else printf '%s' "$3" > steered.json; printf '{"steered":true}'
    fi ;;
  thread.get)
    if [ -f interrupted ]; then
      if [ -f stopping ]; then cat ready.json; else touch stopping; printf '{"status":"working"}'; fi
    elif [ -f polled ]; then cat turn.json
    else touch polled; cat old.json
    fi ;;
  bad) printf 'not json' ;;
  denied) printf 'Permission denied' >&2; exit 1 ;;
  *) cat "$1.json" ;;
esac
"##).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        exe
    }

    fn response(dir: &Path, name: &str, body: Value) {
        std::fs::write(dir.join(format!("{name}.json")), body.to_string()).unwrap();
    }

    #[tokio::test]
    async fn discovers_qualified_models_and_reports_cli_failures() {
        let dir = tempfile::tempdir().unwrap();
        let exe = mock_cli(dir.path());
        response(dir.path(), "provider.list", json!([
            {"provider":"my-codex","enabled":true,"models":[{"model":"coding-model"}]},
            {"provider":"off","enabled":false,"models":[{"model":"hidden"}]}
        ]));
        assert_eq!(models_at(&exe).await.unwrap(), ["my-codex/coding-model"]);
        assert!(call(&exe, "bad", json!({})).await.unwrap_err().contains("Invalid response"));
        assert!(call(&exe, "denied", json!({})).await.unwrap_err().contains("Permission denied"));
    }

    #[tokio::test]
    async fn steering_uses_thread_steer_falls_back_to_send_and_waits_out_approvals() {
        let dir = tempfile::tempdir().unwrap();
        let exe = mock_cli(dir.path());
        let session = crate::tests::with_project(dir.path()).await;
        let (mut run, agent_run) = Run::new(&session, crate::AgentConfig::new(ProviderKind::Zenith), &Conversation::new());
        let handle = agent_run.handle();
        let (mut sent, mut conv) = (vec![], Conversation::new());
        let read = |name: &str| serde_json::from_slice::<Value>(&std::fs::read(dir.path().join(name)).unwrap()).unwrap();
        handle.steer("Use blue".into()).unwrap();
        steer_queued(&mut run, &exe, "t1", &mut sent, &mut conv).await.unwrap();
        assert_eq!(read("steered.json"), json!({"threadId":"t1","prompt":"Use blue"}));
        std::fs::write(dir.path().join("old-zenith"), "").unwrap();
        handle.steer("Make it bigger".into()).unwrap();
        steer_queued(&mut run, &exe, "t1", &mut sent, &mut conv).await.unwrap();
        assert_eq!(read("submitted.json"), json!({"threadId":"t1","prompt":"Make it bigger"}), "older zenith gets thread.send");
        std::fs::remove_file(dir.path().join("old-zenith")).unwrap();
        std::fs::write(dir.path().join("pending"), "").unwrap();
        handle.steer("Never mind".into()).unwrap();
        steer_queued(&mut run, &exe, "t1", &mut sent, &mut conv).await.unwrap();
        assert_eq!(sent, ["Use blue", "Make it bigger"], "a pending approval keeps the run going without the message");
        assert_eq!(conv.messages.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn creates_and_resumes_project_threads_ignoring_stale_replies_and_waits_for_interrupt() {
        let dir = tempfile::tempdir().unwrap();
        let exe = mock_cli(dir.path());
        let session = crate::tests::with_project(dir.path()).await;
        let _bridge = kimchi_control::bridge::Server::start(session.clone()).await.unwrap();
        let project = session.project().unwrap();
        response(dir.path(), "project.list", json!([]));
        response(dir.path(), "project.add", json!({"projectId":"zenith-project"}));
        response(dir.path(), "old", json!({"status":"ready","timeline":[
            {"kind":"message","role":"user","text":"an old request"},
            {"kind":"message","role":"assistant","text":"stale answer"}
        ]}));
        let turn = |prompt: &str, answer: &str| json!({"status":"ready","timeline":[
            {"kind":"message","role":"user","text":context_prompt(&project.name, project.id, &crate::context::glance(&session).frame(prompt))},
            {"kind":"message","role":"assistant","text":answer}
        ]});
        response(dir.path(), "turn", turn("Add a cube", "Cube added."));
        let mut config = crate::AgentConfig::new(ProviderKind::Zenith);
        config.model = "my-codex/coding-model".into();
        let (mut run, _handle) = Run::new(&session, config, &Conversation::new());
        let reply = run_at(&mut run, "Add a cube".into(), Conversation::new(), exe.clone()).await.unwrap();
        assert_eq!(reply, "Cube added.");
        let submitted: Value = serde_json::from_slice(&std::fs::read(dir.path().join("submitted.json")).unwrap()).unwrap();
        assert_eq!(submitted["projectId"], "zenith-project");
        assert_eq!(submitted["provider"], "my-codex");
        assert_eq!(submitted["model"], "coding-model");
        assert!(submitted.get("runtimeMode").is_none(), "zenith retains its approval policy");
        let conv = run.shared.conversation.lock().clone();
        let thread_id = conv.cli_session.as_ref().unwrap().id.clone();
        std::fs::remove_file(dir.path().join("polled")).unwrap();
        response(dir.path(), "turn", turn("Make it blue", "Cube is blue."));
        assert_eq!(run_at(&mut run, "Make it blue".into(), conv, exe).await.unwrap(), "Cube is blue.");
        let submitted: Value = serde_json::from_slice(&std::fs::read(dir.path().join("submitted.json")).unwrap()).unwrap();
        assert_eq!(submitted["threadId"], thread_id);
        assert!(submitted.get("projectId").is_none(), "follow-ups use thread.send");
        response(dir.path(), "ready", json!({"status":"ready"}));
        interrupt(&run).await.unwrap();
        let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(calls.lines().filter(|l| *l == "project.add").count(), 1);
        assert_eq!(calls.lines().filter(|l| *l == "thread.new").count(), 1);
        let stop = calls.split("thread.interrupt").nth(1).unwrap();
        assert_eq!(stop.lines().filter(|l| *l == "thread.get").count(), 2, "interrupt waits for the remote turn to finish");
    }
}
