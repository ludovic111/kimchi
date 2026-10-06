//! `kimchi-cli` end to end: the real binary on a project file, on a bridge to a session in this
//! test (standing in for the app), and the generated command reference.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use kimchi_control::{Session, SessionOptions, Source, registry};
use serde_json::{Value, json};

/// Runs kimchi-cli with every folder it might touch inside `dir`.
fn cli(dir: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kimchi-cli"))
        .args(args)
        .env("KIMCHI_DATA_DIR", dir.join("cli-data"))
        .env("KIMCHI_CONFIG_DIR", dir.join("cli-config"))
        .env("KIMCHI_CONTROL", dir.join("control.json"))
        .env("LSUITE_HOME", dir.join("lsuite"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    if let Some(text) = stdin {
        input.write_all(text.as_bytes()).unwrap();
    }
    drop(input);
    child.wait_with_output().unwrap()
}

fn json_out(o: &Output) -> Value {
    assert!(o.status.success(), "exit {:?}: {}", o.status.code(), String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&o.stdout)))
}

fn session(dir: &Path) -> Arc<Session> {
    Session::new(SessionOptions { data_dir: Some(dir.join("data")), config_dir: Some(dir.join("config")), secrets: None, headless: true }).unwrap()
}

/// A project file made by a headless session, as another tool would.
async fn project_file(dir: &Path) -> PathBuf {
    let s = session(dir);
    registry::call(&s, Source::Cli, "project.create", json!({ "name": "On disk" })).await.unwrap();
    let path = dir.join("cut.json");
    registry::call(&s, Source::Cli, "project.saveAs", json!({ "path": path })).await.unwrap();
    path
}

fn text_clips(path: &Path) -> Vec<String> {
    let p = kimchi_control::session::read_project_file(path).unwrap();
    p.clips()
        .filter_map(|(_, c)| match &c.content {
            kimchi_core::ClipContent::Text { style } => Some(style.content.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn file_mode_edits_and_saves_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = project_file(dir.path()).await;
    let file = path.to_str().unwrap();

    let added = json_out(&cli(dir.path(), &["--file", file, "clip.addText", "text=Hi", "--start", "1"], None));
    assert_eq!(added["clips"][0]["text"], "Hi");
    assert_eq!(text_clips(&path), ["Hi"]);

    // Names work like ids, and --compact prints one line.
    let o = cli(dir.path(), &["--file", file, "--compact", "clip.update", "clipId=Hi", "--opacity", "0.5"], None);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim().lines().count(), 1);

    let overview = json_out(&cli(dir.path(), &["--file", file, "project.overview"], None));
    assert_eq!(overview["project"]["name"], "On disk");
    assert_eq!(overview["tracks"][0]["clips"][0]["text"], "Hi");
}

#[tokio::test(flavor = "multi_thread")]
async fn project_create_makes_a_missing_file_without_touching_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new").join("trailer.json");
    let file = path.to_str().unwrap();
    let o = cli(dir.path(), &["--file", file, "clip.addText", "text=Hi"], None);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("project.create"));

    let created = json_out(&cli(dir.path(), &["--file", file, "project.create", "name=Trailer", "width=1080", "height=1920"], None));
    assert_eq!(created["name"], "Trailer");
    let p = kimchi_control::session::read_project_file(&path).unwrap();
    assert_eq!((p.name.as_str(), p.settings.width, p.settings.height), ("Trailer", 1080, 1920));
    let projects = std::fs::read_dir(dir.path().join("cli-data").join("projects")).map(|d| d.count()).unwrap_or(0);
    assert_eq!(projects, 0, "the library copy is removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_runs_lines_and_stops_at_the_first_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = project_file(dir.path()).await;
    let file = path.to_str().unwrap();
    let lines = r#"{"command":"clip.addText","params":{"text":"One","start":0}}
# comments and blank lines are skipped

{"command":"clip.addText","params":{"txt":"oops"}}
{"command":"clip.addText","params":{"text":"Three","start":8}}
"#;
    let o = cli(dir.path(), &["--file", file, "batch"], Some(lines));
    assert_eq!(o.status.code(), Some(1));
    let out: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(out.len(), 2);
    assert_eq!((out[0]["ok"].clone(), out[1]["ok"].clone(), out[1]["line"].clone()), (json!(true), json!(false), json!(4)));
    assert!(out[1]["error"].as_str().unwrap().contains("Did you mean `text`"));
    assert_eq!(text_clips(&path), ["One"]);

    let o = cli(dir.path(), &["--file", file, "batch", "--continue"], Some(lines));
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&o.stdout).lines().count(), 3);
    assert_eq!(text_clips(&path).len(), 3);
}

#[test]
fn exit_codes_tell_usage_from_command_errors() {
    let dir = tempfile::tempdir().unwrap();
    let usage = |args: &[&str]| cli(dir.path(), args, None).status.code();
    assert_eq!(usage(&[]), Some(2));
    assert_eq!(usage(&["clip.addTxt"]), Some(2));
    assert_eq!(usage(&["--live", "--headless", "project.list"]), Some(2));
    assert_eq!(usage(&["clip.addText"]), Some(2), "missing a required parameter");
    // No app is running at the test's control path.
    let o = cli(dir.path(), &["project.list"], None);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("isn't running"));
    assert_eq!(usage(&["--help"]), Some(0));
    let help = cli(dir.path(), &["help", "generate.submit"], None);
    assert!(String::from_utf8_lossy(&help.stdout).contains("\"generate\" permission"));
    let list = json_out(&cli(dir.path(), &["commands", "--json"], None));
    assert_eq!(list.as_array().unwrap().len(), registry::commands().len());
    let headless = json_out(&cli(dir.path(), &["--headless", "app.info"], None));
    assert_eq!(headless["app"], "kimchi");
}

#[tokio::test(flavor = "multi_thread")]
async fn live_mode_drives_the_app_and_file_mode_leaves_its_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let app = session(dir.path());
    let _bridge = kimchi_control::bridge::Server::start_at(app.clone(), dir.path().join("control.json")).await.unwrap();
    let d = dir.path().to_path_buf();
    let run = move |args: Vec<&str>| {
        let (d, args) = (d.clone(), args.into_iter().map(str::to_string).collect::<Vec<_>>());
        async move { tokio::task::spawn_blocking(move || cli(&d, &args.iter().map(String::as_str).collect::<Vec<_>>(), None)).await.unwrap() }
    };

    json_out(&run(vec!["project.create", "name=Live"]).await);
    json_out(&run(vec!["clip.addSolid", "color=#ff0000", "start=0"]).await);
    assert_eq!(app.project().unwrap().clips().count(), 1);
    // The CLI's edits are in the app's own undo history.
    let history = json_out(&run(vec!["history.list"]).await);
    assert!(history.to_string().contains("cli"), "{history}");

    // As an agent, permissions apply.
    let o = run(vec!["--agent", "app.setSetting", "key=updates.checkOnStart", "value=false"]).await;
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("permission"));

    // A file the app has open is refused for --file.
    let path = dir.path().join("open.json");
    registry::call(&app, Source::Window, "project.saveAs", json!({ "path": path })).await.unwrap();
    registry::call(&app, Source::Window, "project.open", json!({ "path": path })).await.unwrap();
    let o = run(vec!["--file", path.to_str().unwrap(), "clip.addText", "text=Nope"]).await;
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("is open in kimchi"), "{}", String::from_utf8_lossy(&o.stderr));
    // And --headless is refused while the app runs.
    let o = run(vec!["--headless", "project.list"]).await;
    assert_eq!(o.status.code(), Some(1));
}

/// `docs/COMMANDS.md` is generated from the registry and must not drift from it.
#[test]
fn commands_md_matches_the_registry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/COMMANDS.md");
    let current = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(current == registry::markdown(), "docs/COMMANDS.md is out of date: run `cargo run -p kimchi-cli -- docs`");
}

/// `docs/COMPATIBILITY.md` is generated from kimchi's tables of apps, formats and layouts.
#[test]
fn compatibility_md_matches_the_tables() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/COMPATIBILITY.md");
    let current = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(current == kimchi_control::compat::markdown(), "docs/COMPATIBILITY.md is out of date: run `cargo run -p kimchi-cli -- docs`");
}
