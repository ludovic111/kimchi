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

/// kimchi → ryolune → kimchi through a fake ryolune bridge that records what it is asked.
#[tokio::test(flavor = "multi_thread")]
async fn hands_the_cut_to_ryolune_and_takes_audio_back() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    if kimchi_media::Tools::locate().is_err() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    // A fake ryolune: answers every call and remembers it.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let control = dir.path().join("ryolune-control.json");
    std::fs::write(&control, serde_json::to_vec(&json!({ "version": 1, "port": port, "token": "t", "pid": 1 })).unwrap()).unwrap();
    // SAFETY: only this test reads these variables.
    unsafe {
        std::env::set_var("RYOLUNE_CONTROL", &control);
        std::env::set_var("LSUITE_HOME", dir.path().join("lsuite"));
    }
    let calls = Arc::new(parking_lot::Mutex::new(Vec::<Value>::new()));
    let seen = calls.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let seen = seen.clone();
            tokio::spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut lines = BufReader::new(r).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let method = req["method"].as_str().unwrap().to_string();
                    let result = match method.as_str() {
                        "session.info" => json!({ "transport": { "tempo": 120.0, "timeSignature": { "numerator": 4, "denominator": 4 } } }),
                        "session.bounce" => {
                            let out = req["params"]["path"].as_str().unwrap().to_string();
                            let tools = kimchi_media::Tools::locate().unwrap();
                            std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=330:duration=2", &out]).status().unwrap();
                            json!({ "path": out })
                        }
                        _ => json!({ "ok": true }),
                    };
                    let answer = json!({ "jsonrpc": "2.0", "id": req["id"], "result": result });
                    seen.lock().push(req);
                    w.write_all(format!("{answer}\n").as_bytes()).await.unwrap();
                }
            });
        }
    });

    ok(&s, Source::Cli, "project.create", json!({ "name": "Score me" })).await;
    let wav = dir.path().join("tone.wav");
    let tools = kimchi_media::Tools::locate().unwrap();
    std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=4"]).arg(&wav).status().unwrap();
    ok(&s, Source::Cli, "media.import", json!({ "paths": [wav], "place": true, "start": 0 })).await;
    ok(&s, Source::Cli, "timeline.addMarker", json!({ "time": 2, "label": "Drop" })).await;

    let r = ok(&s, Source::Cli, "handoff.toRyolune", json!({})).await;
    assert_eq!(r["sentToRyolune"], true, "{r}");
    assert!(std::path::Path::new(r["audio"].as_str().unwrap()).is_file());
    let cut: Value = serde_json::from_slice(&std::fs::read(r["cut"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(cut["markers"][0]["label"], "Drop");
    {
        let calls = calls.lock();
        let methods: Vec<&str> = calls.iter().map(|c| c["method"].as_str().unwrap()).collect();
        assert_eq!(methods, ["auth", "session.info", "session.importAudio", "marker.add"]);
        // 2 s at 120 bpm in 4/4 is bar 1 (zero-based).
        assert!((calls[3]["params"]["bar"].as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(calls[3]["params"]["name"], "Drop");
    }

    let back = ok(&s, Source::Cli, "handoff.fromRyolune", json!({ "start": 1 })).await;
    let clip = back["clips"][0].as_str().unwrap().parse().unwrap();
    let p = s.project().unwrap();
    assert_eq!(p.clip(clip).unwrap().start, 1.0);
    assert_eq!(p.tracks[p.locate_clip(clip).unwrap().0].kind, kimchi_core::TrackKind::Audio);
}

/// A terminal agent (MCP over the bridge): its records name the clips it created and carry one
/// checkpoint, taken before its first change, that reverts the whole session.
#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_session_can_be_reverted_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Mine" })).await;
    ok(&s, Source::Window, "clip.addSolid", json!({ "color": "#111111", "start": 0 })).await;
    let path = dir.path().join("control.json");
    let _server = crate::bridge::Server::start_at(s.clone(), path.clone()).await.unwrap();
    let mut rx = s.subscribe();
    let mut c = crate::bridge::Client::connect(&path, "mcp").await.unwrap();
    c.call("project.overview", json!({})).await.unwrap();
    c.call("clip.addText", json!({ "text": "One", "start": 1 })).await.unwrap();
    c.call("clip.addText", json!({ "text": "Two", "start": 6 })).await.unwrap();
    let mut records = vec![];
    while let Ok(Ok(e)) = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
        if let crate::Event::Command { record } = e {
            records.push(record);
        }
    }
    let adds: Vec<_> = records.iter().filter(|r| r.command == "clip.addText").collect();
    assert_eq!(adds.len(), 2);
    assert_eq!(adds[0].created.len(), 1, "{:?}", adds[0]);
    let cp = adds[0].checkpoint.expect("a checkpoint for the session");
    assert_eq!(adds[1].checkpoint, Some(cp), "one checkpoint per connection");
    ok(&s, Source::Window, "history.revertTo", json!({ "checkpoint": cp })).await;
    let p = s.project().unwrap();
    assert_eq!(p.clips().count(), 1, "only the person's solid is left");
}

/// export.encoders says what each format is encoded with here; export.start takes the choice and
/// its status names the encoder that ran.
#[tokio::test(flavor = "multi_thread")]
async fn exports_name_their_encoder() {
    if kimchi_media::Tools::locate().is_err() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let enc = ok(&s, Source::Cli, "export.encoders", json!({})).await;
    let mp4 = enc["formats"].as_array().unwrap().iter().find(|f| f["format"] == "mp4").unwrap().clone();
    assert_eq!(mp4["software"]["id"], "libx264", "{enc}");
    assert_eq!(mp4["software"]["label"], "x264 (CPU)");
    assert_eq!(mp4["auto"], if mp4["hardware"].is_null() { mp4["software"].clone() } else { mp4["hardware"].clone() });

    ok(&s, Source::Cli, "project.create", json!({ "name": "Encoders" })).await;
    ok(&s, Source::Cli, "clip.addSolid", json!({ "color": "#336699", "start": 0, "duration": 1 })).await;
    let out = dir.path().join("cpu.mp4");
    let st = ok(&s, Source::Cli, "export.start", json!({ "path": out, "encoder": "software", "width": 320, "height": 180 })).await;
    assert_eq!((st["done"].as_bool(), st["encoder"].as_str()), (Some(true), Some("libx264")), "{st}");
    assert!(out.is_file());
    let err = registry::call(&s, Source::Cli, "export.start", json!({ "path": out, "encoder": "quantum" })).await.unwrap_err();
    assert!(err.contains("auto, hardware or software"), "{err}");
}
