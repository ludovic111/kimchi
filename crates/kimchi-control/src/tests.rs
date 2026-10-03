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

#[tokio::test(flavor = "multi_thread")]
async fn effects_transitions_and_freeze_frames() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Cut", "width": 640, "height": 360 })).await;
    let a = ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#ff0000", "start": 0, "duration": 3, "trackId": "Video 1" })).await["clips"][0]["id"].as_str().unwrap().to_string();
    let b = ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#0000ff", "start": 3, "duration": 3, "trackId": "Video 1" })).await["clips"][0]["id"].as_str().unwrap().to_string();

    // Effects: a look, a field on top, a key, a LUT; mistakes explained.
    let r = ok(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "look": "vintage", "contrast": 0.4, "chromaKey": "#00ff00" })).await;
    let fx = &r["clips"][0]["effects"];
    assert_eq!((fx["contrast"].as_f64(), fx["vignette"].as_f64(), fx["chroma_key"]["color"].as_str()), (Some(0.4), Some(0.45), Some("#00ff00")));
    let e = registry::call(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "look": "noire" })).await.unwrap_err();
    assert!(e.contains("noir"), "{e}");
    let cube = dir.path().join("id.cube");
    let mut text = String::from("LUT_3D_SIZE 2\n");
    for i in 0..8 {
        text.push_str(&format!("{} {} {}\n", i & 1, (i >> 1) & 1, (i >> 2) & 1));
    }
    std::fs::write(&cube, text).unwrap();
    ok(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "lut": cube, "lutStrength": 0.5 })).await;
    let e = registry::call(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "lut": dir.path().join("nope.cube") })).await.unwrap_err();
    assert!(e.contains("Can't use"), "{e}");
    // Effects animate like other properties, and removing the animation keeps the value.
    ok(&s, Source::Agent, "clip.setKeyframes", json!({ "clipId": a, "property": "saturation", "keyframes": [[0, -1], [2, 1]] })).await;
    ok(&s, Source::Agent, "clip.removeKeyframe", json!({ "clipId": a, "property": "saturation" })).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": a })).await;
    assert!(c["keyframes"]["saturation"].is_null());
    assert_eq!(c["effects"]["saturation"].as_f64(), Some(-1.0), "the value at the playhead (0 s) stays");
    let reset = ok(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "reset": true })).await;
    assert_eq!(reset["clips"][0]["effects"], json!(null), "no effects left: {reset}");
    assert_eq!(ok(&s, Source::Agent, "clip.looks", json!({})).await.as_array().unwrap().len(), kimchi_core::effects::LOOKS.len());

    // Transitions: on every cut of a track, listed with where they play, then changed and removed.
    let r = ok(&s, Source::Agent, "transition.set", json!({ "trackId": "Video 1", "kind": "crossfade", "duration": 1 })).await;
    assert_eq!(r.as_array().unwrap().len(), 1);
    assert_eq!((r[0]["kind"].as_str(), r[0]["start"].as_f64(), r[0]["end"].as_f64(), r[0]["from"].as_str()), (Some("dissolve"), Some(2.5), Some(3.5), Some("Solid")));
    ok(&s, Source::Agent, "transition.set", json!({ "clipIds": [b], "kind": "pushLeft" })).await;
    let l = ok(&s, Source::Agent, "transition.list", json!({})).await;
    assert_eq!((l[0]["kind"].as_str(), l[0]["duration"].as_f64()), (Some("pushLeft"), Some(1.0)), "the length stayed");
    let e = registry::call(&s, Source::Agent, "transition.set", json!({ "clipIds": [b], "kind": "swirl" })).await.unwrap_err();
    assert!(e.contains("Transitions:"), "{e}");
    if s.tools().is_ok() {
        // Halfway through a push the red clip is on the left, the blue one on the right.
        let f = ok(&s, Source::Agent, "project.renderFrame", json!({ "time": 3.0, "width": 320 })).await;
        let png = kimchi_media::tiny_skia::Pixmap::load_png(f["path"].as_str().unwrap()).unwrap();
        let (l, r) = (png.pixel(40, 90).unwrap(), png.pixel(280, 90).unwrap());
        assert!(l.red() > 200 && l.blue() < 50 && r.blue() > 200 && r.red() < 50, "{l:?} {r:?}");
    }
    ok(&s, Source::Agent, "transition.remove", json!({ "clipIds": [b] })).await;
    assert!(ok(&s, Source::Agent, "transition.list", json!({})).await.as_array().unwrap().is_empty());

    // Solids and titles can't play backwards.
    let e = registry::call(&s, Source::Agent, "clip.update", json!({ "clipId": a, "reverse": true })).await.unwrap_err();
    assert!(e.contains("backwards"), "{e}");

    // A freeze frame splits the clip, holds for 2 s and pushes the rest later, as one step.
    if s.tools().is_ok() {
        let r = ok(&s, Source::Agent, "clip.freezeFrame", json!({ "clipId": a, "time": 1.0 })).await;
        assert_eq!((r["clips"][0]["start"].as_f64(), r["clips"][0]["duration"].as_f64()), (Some(1.0), Some(2.0)));
        let clips = ok(&s, Source::Agent, "clip.list", json!({ "trackId": "Video 1" })).await;
        let starts: Vec<f64> = clips.as_array().unwrap().iter().map(|c| c["start"].as_f64().unwrap()).collect();
        assert_eq!(starts, vec![0.0, 1.0, 3.0, 5.0]);
        ok(&s, Source::Agent, "history.undo", json!({})).await;
        assert_eq!(ok(&s, Source::Agent, "clip.list", json!({ "trackId": "Video 1" })).await.as_array().unwrap().len(), 2);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn captions_import_style_and_export() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Subs", "width": 640, "height": 360 })).await;
    let srt = dir.path().join("in.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,500\nHello\n\n2\n00:00:03,000 --> 00:00:04,000\nSecond line\nof two\n").unwrap();
    let r = ok(&s, Source::Agent, "captions.import", json!({ "path": srt })).await;
    assert_eq!(r["captions"], 2);
    let tracks = ok(&s, Source::Agent, "track.list", json!({})).await;
    assert_eq!(tracks[0]["name"], "Captions", "a captions track on top: {tracks}");
    let list = ok(&s, Source::Agent, "captions.list", json!({})).await;
    assert_eq!((list[1]["text"].as_str(), list[1]["start"].as_f64()), (Some("Second line\nof two"), Some(3.0)));
    // One more, styled like the others; then all restyled at once.
    ok(&s, Source::Agent, "captions.add", json!({ "text": "Third", "start": 5 })).await;
    ok(&s, Source::Agent, "captions.setStyle", json!({ "style": { "fontSize": 30, "color": "#ffcc00" }, "y": 100 })).await;
    let p = s.project().unwrap();
    assert!(p.captions().iter().all(|(_, c, _)| c.transform.y == 100.0 && matches!(&c.content, kimchi_core::ClipContent::Text { style } if style.font_size == 30.0 && style.color == "#ffcc00")));
    // Out as WebVTT, and next to an export instead of in the picture.
    let vtt = dir.path().join("out.vtt");
    ok(&s, Source::Agent, "captions.export", json!({ "path": vtt })).await;
    let text = std::fs::read_to_string(&vtt).unwrap();
    assert!(text.starts_with("WEBVTT") && text.contains("00:00:05.000 --> 00:00:07.500\nThird"), "{text}");
    if s.tools().is_ok() {
        let mp4 = dir.path().join("cut.mp4");
        ok(&s, Source::Agent, "export.start", json!({ "path": mp4, "quality": "draft", "captions": "file", "wait": true })).await;
        assert!(dir.path().join("cut.srt").is_file());
    }
    // Clearing is one step.
    ok(&s, Source::Agent, "captions.clear", json!({})).await;
    assert!(ok(&s, Source::Agent, "captions.list", json!({})).await.as_array().unwrap().is_empty());
    ok(&s, Source::Agent, "history.undo", json!({})).await;
    assert_eq!(ok(&s, Source::Agent, "captions.list", json!({})).await.as_array().unwrap().len(), 3);
    let models = ok(&s, Source::Agent, "captions.models", json!({})).await;
    assert_eq!(models.as_array().unwrap().len(), 3);
    assert_eq!(ok(&s, Source::Agent, "captions.status", json!({})).await["running"], false);
}

/// The real model on real speech: `KIMCHI_SPEECH_WAV=jfk.wav KIMCHI_WHISPER_DIR=/models cargo test
/// -p kimchi-control captions_from_speech -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn captions_from_speech() {
    let (Ok(wav), Ok(models)) = (std::env::var("KIMCHI_SPEECH_WAV"), std::env::var("KIMCHI_WHISPER_DIR")) else { return };
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    // Reuse the downloaded models.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&models, dir.path().join("data/models")).unwrap();
    ok(&s, Source::Window, "project.create", json!({ "name": "Speech" })).await;
    ok(&s, Source::Agent, "media.import", json!({ "paths": [wav], "place": true, "start": 2 })).await;
    let r = ok(&s, Source::Agent, "captions.transcribe", json!({})).await;
    eprintln!("{r:#}");
    assert_eq!(r["language"], "en");
    let list = ok(&s, Source::Agent, "captions.list", json!({})).await;
    let text = list.as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap().replace('\n', " ")).collect::<Vec<_>>().join(" ").to_lowercase();
    assert!(text.contains("ask not what your country can do for you"), "{text}");
    assert!(list[0]["start"].as_f64().unwrap() >= 2.0, "timed on the timeline: {list}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_move_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({})).await;
    let a = registry::created_clips(&ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#111111", "start": 0 })).await)[0];
    let b = registry::created_clips(&ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#222222", "start": 10 })).await)[0];
    let tracks = ok(&s, Source::Agent, "track.list", json!({})).await;
    let audio = tracks.as_array().unwrap().iter().find(|t| t["kind"] == "audio").unwrap()["id"].as_str().unwrap().to_string();
    let moves = json!({ "moves": [{ "clipId": a.to_string(), "start": 30 }, { "clipId": b.to_string(), "trackId": audio, "start": 0 }] });
    assert!(registry::call(&s, Source::Agent, "clip.moveMany", moves).await.is_err());
    let p = s.project().unwrap();
    assert_eq!(p.clip(a).unwrap().start, 0.0, "the first move is put back");
    assert_eq!(s.read(|ed| ed.undo_steps().len()).unwrap(), 2);
    // Absurd times are refused rather than written as null.
    assert!(registry::call(&s, Source::Agent, "timeline.addMarker", json!({ "time": 1e300 })).await.is_err());
    let e = registry::call(&s, Source::Agent, "generate.wait", json!({ "jobId": "nope", "timeout": 1e20 })).await.unwrap_err();
    assert!(e.contains("No job"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn batches_hold_others_back_and_end_when_dropped() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({})).await;
    // `timeline.play` waits for the window, which answers when the test says.
    let mut ui = s.attach_ui();
    let batch = |label: &'static str| {
        let s = s.clone();
        async move {
            registry::call(&s, Source::Agent, "project.batch", json!({ "commands": [
                { "command": "timeline.addMarker", "params": { "time": 1, "label": label } },
                { "command": "timeline.play" },
            ]}))
            .await
        }
    };
    let markers = |s: &Arc<Session>| s.project().unwrap().markers.iter().map(|m| m.label.clone()).collect::<Vec<_>>();
    let running = tokio::spawn(batch("agent"));
    let call = ui.next().await.unwrap();
    assert_eq!(markers(&s), ["agent"]);
    // The window's change waits for the batch instead of folding into it.
    let s2 = s.clone();
    let window = tokio::spawn(async move { registry::call(&s2, Source::Window, "timeline.addMarker", json!({ "time": 2, "label": "window" })).await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!window.is_finished());
    call.reply.send(Err("no".into())).unwrap();
    assert!(running.await.unwrap().is_err());
    window.await.unwrap().unwrap();
    assert_eq!(markers(&s), ["window"], "the batch rolled back only its own change");
    // A batch whose caller stops waiting (an agent's Stop) is rolled back, not left open.
    assert!(tokio::time::timeout(std::time::Duration::from_millis(300), batch("dropped")).await.is_err());
    assert!(!s.read(|ed| ed.in_batch()).unwrap());
    assert_eq!(markers(&s), ["window"]);
    ok(&s, Source::Window, "timeline.addMarker", json!({ "time": 3, "label": "after" })).await;
    assert_eq!(s.read(|ed| ed.undo_steps().len()).unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn projects_keep_to_themselves() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    // A checkpoint names a state of its own project only.
    ok(&s, Source::Window, "project.create", json!({ "name": "A" })).await;
    let cp = ok(&s, Source::Window, "history.checkpoint", json!({})).await["checkpoint"].clone();
    ok(&s, Source::Window, "project.create", json!({ "name": "B" })).await;
    let e = registry::call(&s, Source::Window, "history.revertTo", json!({ "checkpoint": cp })).await.unwrap_err();
    assert!(e.contains("isn't in the open project's history"), "{e}");
    // Renaming a project opened from a file answers with its new name.
    let file = dir.path().join("b.json");
    ok(&s, Source::Window, "project.saveAs", json!({ "path": file })).await;
    ok(&s, Source::Window, "project.open", json!({ "path": file })).await;
    assert_eq!(ok(&s, Source::Window, "project.rename", json!({ "name": "Renamed" })).await["name"], "Renamed");
    // A duplicate has its own copy of the generated media.
    ok(&s, Source::Window, "project.create", json!({ "name": "Orig" })).await;
    let orig = s.current_id().unwrap();
    let gen_dir = s.library.generated_dir(orig);
    std::fs::create_dir_all(&gen_dir).unwrap();
    std::fs::write(gen_dir.join("pic.png"), b"png").unwrap();
    let asset = kimchi_core::Asset {
        id: kimchi_core::new_id(),
        name: "pic".into(),
        kind: kimchi_core::MediaKind::Image,
        path: gen_dir.join("pic.png").to_string_lossy().into_owned(),
        meta: Default::default(),
        origin: kimchi_core::AssetOrigin::Imported,
        created_at: chrono::Utc::now(),
        thumbnail: None,
        filmstrip: None,
        waveform: None,
        proxy: None,
    };
    s.apply("test", Source::Window, &kimchi_core::Edit::AddAsset { asset }, None).unwrap();
    let copy = ok(&s, Source::Window, "project.duplicate", json!({})).await;
    let copy_id: kimchi_core::Id = copy["id"].as_str().unwrap().parse().unwrap();
    ok(&s, Source::Window, "project.delete", json!({ "projectId": orig.to_string() })).await;
    let p = s.library.load(copy_id).unwrap();
    let path = std::path::Path::new(&p.assets[0].path);
    assert!(path.starts_with(s.library.project_dir(copy_id)) && path.is_file(), "{path:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn captions_files_in_other_encodings() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({})).await;
    let srt = dir.path().join("utf16.srt");
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend("1\r\n00:00:01,000 --> 00:00:02,000\r\nÇa va\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nOui\r\n".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&srt, bytes).unwrap();
    assert_eq!(ok(&s, Source::Agent, "captions.import", json!({ "path": srt })).await["captions"], 2);
    let list = ok(&s, Source::Agent, "captions.list", json!({})).await;
    assert_eq!(list[0]["text"], "Ça va");
}

#[tokio::test]
async fn paths_and_lines_are_bounded() {
    let p = crate::commands::media::absolute("-take2.mov").unwrap();
    assert!(p.is_absolute() && p.ends_with("-take2.mov"));
    let mut r: &[u8] = b"short\r\nwaaaaaaaay too long\n";
    assert_eq!(crate::bridge::read_line(&mut r, 8).await.unwrap().as_deref(), Some("short"));
    assert!(crate::bridge::read_line(&mut r, 8).await.is_err());
}
