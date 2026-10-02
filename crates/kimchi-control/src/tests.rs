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

#[tokio::test(flavor = "multi_thread")]
async fn motion_clips_templates_and_keyframes() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Motion", "width": 640, "height": 360 })).await;

    // A template clip, its scene, a layer changed by hand, then new template values.
    let added = ok(&s, Source::Agent, "motion.addTemplate", json!({ "template": "lowerThird", "values": { "title": "Grace Hopper" }, "start": 0 })).await;
    let clip = added["clips"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(added["clips"][0]["type"], "motion");
    assert_eq!(added["clips"][0]["template"], "lowerThird");
    let got = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "title" })).await;
    assert_eq!(got["text"], "Grace Hopper");
    ok(&s, Source::Agent, "motion.setLayer", json!({ "clipId": clip, "layer": { "id": "dot", "type": "ellipse", "width": 20, "height": 20, "fill": "#ffffff" }, "parent": "lowerThird" })).await;
    ok(&s, Source::Agent, "motion.setKeyframes", json!({ "clipId": clip, "id": "dot", "property": "opacity", "keyframes": [[0, 0], [0.5, 1, "easeOut"]] })).await;
    let scene = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip })).await;
    assert!(scene["scene"].to_string().contains("\"dot\""));
    ok(&s, Source::Agent, "motion.setTemplate", json!({ "clipId": clip, "values": { "subtitle": "Rear admiral" } })).await;
    let got = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "subtitle" })).await;
    assert_eq!(got["text"], "Rear admiral");
    assert_eq!(ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "title" })).await["text"], "Grace Hopper", "values are kept");

    // The template was re-made (the dot went with the old scene) and the clip's name followed it.
    assert!(ok(&s, Source::Agent, "clip.get", json!({ "clipId": clip })).await["name"].as_str().unwrap().ends_with("Grace Hopper"));
    ok(&s, Source::Agent, "motion.setLayer", json!({ "clipId": clip, "layer": { "id": "dot", "type": "ellipse", "width": 20, "height": 20, "keyframes": { "opacity": [[0, 0], [0.5, 1]] } } })).await;
    // One property at a time; animated ones get a keyframe at that time.
    ok(&s, Source::Window, "motion.updateLayer", json!({ "clipId": clip, "id": "dot", "props": { "x": 40, "fill": "#ff0000", "stroke": { "width": 3 } } })).await;
    let dot = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "dot" })).await;
    assert_eq!((dot["x"].as_f64(), dot["fill"].as_str(), dot["stroke"]["width"].as_f64()), (Some(40.0), Some("#ff0000"), Some(3.0)));
    ok(&s, Source::Window, "motion.updateLayer", json!({ "clipId": clip, "id": "dot", "props": { "opacity": 0.5 }, "time": 2.0 })).await;
    let dot = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "dot" })).await;
    assert_eq!(dot["keyframes"]["opacity"].as_array().unwrap().len(), 3, "a third keyframe at 2 s: {dot}");
    ok(&s, Source::Window, "motion.addKeyframe", json!({ "clipId": clip, "id": "dot", "property": "x", "time": 1.0 })).await;
    ok(&s, Source::Window, "motion.addKeyframe", json!({ "clipId": clip, "id": "dot", "property": "x", "time": 2.0, "value": 90, "easing": "easeOut" })).await;
    ok(&s, Source::Window, "motion.removeKeyframe", json!({ "clipId": clip, "id": "dot", "property": "x", "time": 1.0 })).await;
    let dot = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "dot" })).await;
    assert_eq!(dot["keyframes"]["x"].as_array().unwrap().len(), 1);
    let e = registry::call(&s, Source::Window, "motion.addKeyframe", json!({ "clipId": clip, "id": "dot", "property": "wobble" })).await.unwrap_err();
    assert!(e.contains("no property `wobble`"), "{e}");

    // Mistakes come back with the fix.
    let e = registry::call(&s, Source::Agent, "motion.add", json!({ "scene": { "layers": [{ "id": "a", "type": "rectangle" }] } })).await.unwrap_err();
    assert!(e.contains("Did you mean `rect`"), "{e}");
    let e = registry::call(&s, Source::Agent, "motion.addTemplate", json!({ "template": "lowerthird3" })).await.unwrap_err();
    assert!(e.contains("lowerThird"), "{e}");

    // A 3D scene as a clip; its length follows the keyframes.
    let three = ok(&s, Source::Agent, "motion.add", json!({ "start": 6, "scene": {
        "objects": [{ "id": "cube", "type": "box", "keyframes": { "rotation.y": [[0, 0], [4, 360]] } }]
    } })).await;
    assert_eq!(three["clips"][0]["scene"], "3d");
    assert_eq!(three["clips"][0]["duration"], 5.0);
    let three_id = three["clips"][0]["id"].as_str().unwrap().to_string();
    ok(&s, Source::Window, "motion.updateLayer", json!({ "clipId": three_id, "id": "camera", "props": { "fov": 30, "position": [0, 2, 9] } })).await;
    ok(&s, Source::Window, "motion.updateLayer", json!({ "clipId": three_id, "id": "cube", "props": { "material": { "metallic": 1 }, "color": "#00ff00" } })).await;
    let cube = ok(&s, Source::Agent, "motion.get", json!({ "clipId": three_id, "id": "cube" })).await;
    assert_eq!((cube["material"]["metallic"].as_f64(), cube["material"]["color"].as_str()), (Some(1.0), Some("#00ff00")));
    let cam = ok(&s, Source::Agent, "motion.get", json!({ "clipId": three_id, "id": "camera" })).await;
    assert_eq!((cam["fov"].as_f64(), cam["position"][1].as_f64()), (Some(30.0), Some(2.0)));

    // Clip keyframes and presets, one undo step each.
    let text = ok(&s, Source::Agent, "clip.addText", json!({ "text": "Hi", "start": 0, "duration": 3 })).await;
    let text_id = text["clips"][0]["id"].as_str().unwrap().to_string();
    let r = ok(&s, Source::Agent, "clip.setKeyframes", json!({ "clipId": text_id, "property": "x", "keyframes": [{ "time": 0, "value": -200 }, { "time": 1, "value": 0, "easing": "easeOutBack" }] })).await;
    assert_eq!(r["keyframes"]["x"][1]["easing"], "easeOutBack");
    let e = registry::call(&s, Source::Agent, "clip.setKeyframes", json!({ "clipId": text_id, "property": "wobble", "keyframes": [[0, 1]] })).await.unwrap_err();
    assert!(e.contains("can't animate `wobble`"), "{e}");
    ok(&s, Source::Agent, "clip.animate", json!({ "clipIds": [text_id], "preset": "fadeOut" })).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": text_id })).await;
    assert!(c["keyframes"]["opacity"].as_array().unwrap().len() == 2 && c["keyframes"]["x"].is_array());
    ok(&s, Source::Agent, "history.undo", json!({})).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": text_id })).await;
    assert!(c["keyframes"]["opacity"].is_null(), "the preset was one step");
    ok(&s, Source::Agent, "clip.addKeyframe", json!({ "clipId": text_id, "property": "opacity", "time": 2 })).await;
    ok(&s, Source::Agent, "clip.removeKeyframe", json!({ "clipId": text_id, "property": "x" })).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": text_id })).await;
    assert!(c["keyframes"]["x"].is_null() && c["keyframes"]["opacity"].is_array());

    // The guide and listings.
    let g = ok(&s, Source::Agent, "motion.guide", json!({ "topic": "3d" })).await;
    assert!(g["guide"].as_str().unwrap().contains("## 3D scenes") && !g["guide"].as_str().unwrap().contains("## 2D scenes"));
    assert!(ok(&s, Source::Agent, "motion.templates", json!({})).await.as_array().unwrap().len() >= 15);
    assert!(ok(&s, Source::Agent, "motion.presets", json!({})).await.as_array().unwrap().len() >= 30);

    // Frames to look at (needs ffmpeg only for media; these are all drawn).
    if s.tools().is_ok() {
        let one = ok(&s, Source::Agent, "project.renderFrame", json!({ "time": 1.0, "width": 320 })).await;
        assert!(std::path::Path::new(one["path"].as_str().unwrap()).is_file());
        let sheet = ok(&s, Source::Agent, "project.renderFrame", json!({ "times": [0.2, 1.0, 2.0, 7.0] })).await;
        let png = kimchi_media::tiny_skia::Pixmap::load_png(sheet["path"].as_str().unwrap()).unwrap();
        assert!(png.width() > 900 && png.height() > 500, "2×2 sheet: {}x{}", png.width(), png.height());
        let frame = ok(&s, Source::Agent, "media.frame", json!({ "clipId": clip, "time": 2.0 })).await;
        assert!(std::path::Path::new(frame["path"].as_str().unwrap()).is_file());
    }
}
