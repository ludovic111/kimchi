use std::sync::Arc;

use serde_json::{Value, json};

use crate::registry::{self, Args, Ctx};
use crate::session::{Session, SessionOptions, Source};

fn session(dir: &std::path::Path) -> Arc<Session> {
    Session::new(SessionOptions {
        data_dir: Some(dir.join("data")),
        config_dir: Some(dir.join("config")),
        secrets: None,
        headless: true,
    })
    .unwrap()
}

async fn ok(s: &Arc<Session>, source: Source, name: &str, params: Value) -> Value {
    registry::call(s, source, name, params).await.unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn every_command_has_a_handler() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    for spec in registry::commands() {
        if matches!(spec.name, "app.checkUpdates" | "app.installUpdate" | "generate.models" | "generate.check") {
            continue; // network
        }
        let cx = Ctx { source: Source::Window, spec };
        let r = crate::commands::dispatch(&s, &cx, Args::default()).await;
        if let Err(e) = r {
            assert!(!e.contains("not implemented"), "{}: {e}", spec.name);
        }
    }
}

#[test]
fn names_are_unique_and_well_formed() {
    let mut seen = std::collections::HashSet::new();
    for s in registry::commands() {
        assert!(seen.insert(s.name), "duplicate {}", s.name);
        let (family, verb) = s.name.split_once('.').expect("family.verb");
        assert!(family.chars().all(|c| c.is_ascii_lowercase()), "{}", s.name);
        assert!(verb.chars().next().unwrap().is_ascii_lowercase(), "{}", s.name);
        assert!(!s.doc.is_empty());
        for p in s.params {
            assert!(!p.doc.is_empty(), "{}.{}", s.name, p.name);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_share_one_undo_history() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Shared" })).await;
    ok(&s, Source::Agent, "clip.addText", json!({ "text": "Hello", "start": 0 })).await;
    ok(&s, Source::Cli, "clip.addSolid", json!({ "color": "#ff0000", "start": 4 })).await;
    let h = ok(&s, Source::Window, "history.list", json!({})).await;
    let sources: Vec<&str> = h["undo"].as_array().unwrap().iter().map(|s| s["source"].as_str().unwrap()).collect();
    assert_eq!(sources, ["cli", "agent"]);
    // The window undoes the CLI's edit, then the agent's.
    ok(&s, Source::Window, "history.undo", json!({})).await;
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(s.project().unwrap().clips().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn names_work_like_ids_and_mistakes_are_explained() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    ok(&s, Source::Cli, "clip.addText", json!({ "text": "Opening title", "start": 1 })).await;
    let c = ok(&s, Source::Cli, "clip.update", json!({ "clipId": "Opening title", "opacity": 0.5, "style": { "fontSize": 80 } })).await;
    assert_eq!(c["transform"]["opacity"], 0.5);
    let e = registry::call(&s, Source::Cli, "clip.update", json!({ "clipId": "Opening titel" })).await.unwrap_err();
    assert!(e.contains("Did you mean Opening title"), "{e}");
    let e = registry::call(&s, Source::Cli, "clip.updat", json!({})).await.unwrap_err();
    assert!(e.contains("clip.update"), "{e}");
    let e = registry::call(&s, Source::Cli, "clip.split", json!({ "tme": 2 })).await.unwrap_err();
    assert!(e.contains("Did you mean `time`"), "{e}");
    let e = registry::call(&s, Source::Cli, "clip.split", json!({ "time": "two" })).await.unwrap_err();
    assert!(e.contains("should be a number"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_is_one_step_and_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    ok(&s, Source::Agent, "project.batch", json!({ "commands": [
        { "command": "clip.addSolid", "params": { "color": "#000000", "start": 0 } },
        { "command": "timeline.addMarker", "params": { "time": 1, "label": "Drop" } },
    ]})).await;
    assert_eq!(s.read(|ed| ed.undo_steps().len()).unwrap(), 1);
    let e = registry::call(&s, Source::Agent, "project.batch", json!({ "commands": [
        { "command": "timeline.addMarker", "params": { "time": 2, "label": "B" } },
        { "command": "clip.delete", "params": { "clipIds": ["nope"] } },
    ]})).await.unwrap_err();
    assert!(e.contains("Nothing was changed"), "{e}");
    assert_eq!(s.project().unwrap().markers.len(), 1);
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert!(s.project().unwrap().markers.is_empty());
    assert_eq!(s.project().unwrap().clips().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn permissions_apply_to_agents_and_mcp_only() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    let e = registry::call(&s, Source::Mcp, "generate.setKey", json!({ "provider": "fal", "key": "x" })).await.unwrap_err();
    assert!(e.contains("stays with the person"), "{e}");
    let e = registry::call(&s, Source::Agent, "app.setSetting", json!({ "key": "updates.checkOnStart", "value": false })).await.unwrap_err();
    assert!(e.contains("\"settings\" permission"), "{e}");
    s.update_settings(|st| st.agent.permissions.enabled = false).unwrap();
    let e = registry::call(&s, Source::Mcp, "project.overview", json!({})).await.unwrap_err();
    assert!(e.contains("turned off"), "{e}");
    // The person's own tools are not agents.
    ok(&s, Source::Cli, "app.setSetting", json!({ "key": "updates.checkOnStart", "value": false })).await;
    assert!(!s.settings().updates.check_on_start);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_mode_saves_back_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({ "name": "On disk" })).await;
    let path = dir.path().join("cut.json");
    ok(&s, Source::Cli, "project.saveAs", json!({ "path": path })).await;
    let s2 = session(&dir.path().join("other"));
    ok(&s2, Source::Cli, "project.open", json!({ "path": path })).await;
    ok(&s2, Source::Cli, "timeline.addMarker", json!({ "time": 3, "label": "Here" })).await;
    let back = crate::session::read_project_file(&path).unwrap();
    assert_eq!(back.markers[0].label, "Here");
}

#[tokio::test(flavor = "multi_thread")]
async fn overview_reports_problems() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    ok(&s, Source::Cli, "clip.addText", json!({ "text": "Hi", "start": 0 })).await;
    let track = ok(&s, Source::Cli, "track.list", json!({})).await[0]["name"].as_str().unwrap().to_string();
    ok(&s, Source::Cli, "track.update", json!({ "trackId": track, "hidden": true })).await;
    let o = ok(&s, Source::Mcp, "project.overview", json!({})).await;
    assert!(o["problems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("hidden")), "{o}");
    assert_eq!(o["tracks"][0]["clips"][0]["text"], "Hi");
}

#[tokio::test(flavor = "multi_thread")]
async fn window_commands_need_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let e = registry::call(&s, Source::Cli, "timeline.play", json!({})).await.unwrap_err();
    assert!(e.contains("needs the kimchi window"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bridge_runs_commands_for_a_client_with_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let path = dir.path().join("control.json");
    let _server = crate::bridge::Server::start_at(s.clone(), path.clone()).await.unwrap();
    let mut c = crate::bridge::Client::connect(&path, "mcp").await.unwrap();
    c.call("project.create", json!({ "name": "Live" })).await.unwrap();
    let list = c.call("project.list", json!({})).await.unwrap();
    assert_eq!(list[0]["name"], "Live");
    // A wrong token is refused.
    let mut d = crate::bridge::read_discovery(&path).unwrap();
    d.token = "0".repeat(d.token.len());
    std::fs::write(&path, serde_json::to_vec(&d).unwrap()).unwrap();
    assert!(crate::bridge::Client::connect(&path, "cli").await.is_err());
}

#[test]
fn markdown_lists_every_command() {
    let md = registry::markdown();
    for s in registry::commands() {
        assert!(md.contains(&format!("### `{}`", s.name)));
    }
}
