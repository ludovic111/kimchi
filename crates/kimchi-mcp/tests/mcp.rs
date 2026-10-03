//! The real `kimchi-mcp` binary over stdio, on a project file.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Mcp {
    child: Child,
    reader: BufReader<ChildStdout>,
}

impl Mcp {
    fn start(dir: &Path, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kimchi-mcp"))
            .args(args)
            .env("KIMCHI_DATA_DIR", dir.join("data"))
            .env("KIMCHI_CONFIG_DIR", dir.join("config"))
            .env("KIMCHI_CONTROL", dir.join("absent-control.json"))
            .env("LSUITE_HOME", dir.join("lsuite"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        Self { child, reader }
    }

    fn send(&mut self, frame: Value) {
        writeln!(self.child.stdin.as_mut().unwrap(), "{frame}").unwrap();
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(reply["id"], id);
        reply
    }

    fn tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(id, "tools/call", json!({ "name": name, "arguments": arguments }))["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

#[test]
fn speaks_mcp_over_a_project_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cut.json");
    let mut mcp = Mcp::start(dir.path(), &["--file", file.to_str().unwrap()]);

    let init = mcp.request(1, "initialize", json!({ "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }));
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "kimchi");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert!(init["result"]["instructions"].as_str().unwrap().contains("project_overview"));
    // Notifications get no answer: the next line read is the ping's.
    mcp.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    assert_eq!(mcp.request(2, "ping", json!({}))["result"], json!({}));
    let old = mcp.request(3, "initialize", json!({ "protocolVersion": "1999-01-01" }));
    assert_eq!(old["result"]["protocolVersion"], "2025-11-25");

    let tools = mcp.request(4, "tools/list", json!({}))["result"]["tools"].clone();
    let tools = tools.as_array().unwrap();
    let find = |name: &str| tools.iter().find(|t| t["name"] == name).cloned();
    let add_text = find("clip_addText").expect("clip_addText");
    assert_eq!(add_text["inputSchema"]["required"], json!(["text"]));
    assert_eq!(add_text["annotations"]["readOnlyHint"], false);
    assert_eq!(find("project_overview").unwrap()["annotations"]["readOnlyHint"], true);
    assert!(find("generate_submit").unwrap()["description"].as_str().unwrap().contains("\"generate\" agent permission"));
    assert!(find("timeline_play").unwrap()["description"].as_str().unwrap().contains("window"));
    assert!(find("generate_setKey").is_none(), "person-only commands are not offered");

    // The file doesn't exist yet: project_create makes it.
    let overview = mcp.tool(5, "project_overview", json!({}));
    assert_eq!(overview["isError"], true);
    assert!(overview["content"][0]["text"].as_str().unwrap().contains("project.create"));
    let created = mcp.tool(6, "project_create", json!({ "name": "From MCP" }));
    assert_eq!(created["isError"], false, "{created}");
    assert!(file.exists());

    let added = mcp.tool(7, "clip_addText", json!({ "text": "Hi", "start": 0 }));
    assert_eq!(added["isError"], false, "{added}");
    assert_eq!(added["structuredContent"]["clips"][0]["text"], "Hi");
    assert!(added["content"][0]["text"].as_str().unwrap().contains("saved"));

    let overview = mcp.tool(8, "project_overview", json!({}));
    assert_eq!(overview["isError"], false);
    assert_eq!(overview["structuredContent"]["project"]["name"], "From MCP");
    assert_eq!(overview["structuredContent"]["tracks"][0]["clips"][0]["text"], "Hi");
    assert!(overview["structuredContent"]["project"]["location"]["file"].as_str().unwrap().ends_with("cut.json"));
    let on_disk: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(on_disk["name"], "From MCP");

    // Tool errors are results with isError, not protocol errors.
    let bad = mcp.tool(9, "clip_addText", json!({ "txt": "x" }));
    assert_eq!(bad["isError"], true);
    assert!(bad["content"][0]["text"].as_str().unwrap().contains("Did you mean `text`"));
    let window = mcp.tool(10, "timeline_play", json!({}));
    assert_eq!(window["isError"], true);
    assert_eq!(mcp.request(11, "tools/call", json!({ "name": "nope_nope" }))["error"]["code"], -32602);

    // The overview is also a resource; undo is shared with every edit above.
    let resources = mcp.request(12, "resources/list", json!({}))["result"]["resources"].clone();
    assert!(resources.as_array().unwrap().iter().any(|r| r["uri"] == "kimchi://project/overview"));
    let read = mcp.request(13, "resources/read", json!({ "uri": "kimchi://project/overview" }));
    let text = read["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(serde_json::from_str::<Value>(text).unwrap()["history"]["canUndo"] == true);
    assert_eq!(mcp.tool(14, "history_undo", json!({}))["isError"], false);
    let p: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(p["tracks"].as_array().unwrap().iter().all(|t| t["clips"].as_array().unwrap().is_empty()), "{p}");

    let prompts = mcp.request(15, "prompts/list", json!({}))["result"]["prompts"].clone();
    let names: Vec<&str> = prompts.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["rough-cut", "title-and-captions", "generate-b-roll", "review-the-cut", "motion-design", "3d-scene"]);
    let got = mcp.request(19, "prompts/get", json!({ "name": "3d-scene", "arguments": { "idea": "a chrome teapot", "seconds": "8" } }));
    let text = got["result"]["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("a chrome teapot") && text.contains("times=[0.5, 4, 7.5]"), "{text}");
    let got = mcp.request(16, "prompts/get", json!({ "name": "generate-b-roll", "arguments": { "subject": "a misty harbour" } }));
    assert!(got["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("a misty harbour"));
    assert_eq!(mcp.request(17, "prompts/get", json!({ "name": "title-and-captions" }))["error"]["code"], -32602);
    assert_eq!(mcp.request(18, "no/such", json!({}))["error"]["code"], -32601);
}

#[test]
fn requests_overlap_and_answers_stay_whole() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cut.json");
    let mut mcp = Mcp::start(dir.path(), &["--file", file.to_str().unwrap()]);
    mcp.request(1, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    // Sent back to back: every request is answered once, in whatever order they finish.
    for id in 2..12u64 {
        let frame = if id % 2 == 0 {
            json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": "app_commands", "arguments": {} } })
        } else {
            json!({ "jsonrpc": "2.0", "id": id, "method": "ping" })
        };
        mcp.send(frame);
    }
    // Cancelling something unknown (or already answered) is a no-op.
    mcp.send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 99 } }));
    let mut ids = vec![];
    for _ in 2..12 {
        let mut line = String::new();
        mcp.reader.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        ids.push(reply["id"].as_u64().unwrap());
    }
    ids.sort();
    assert_eq!(ids, (2..12).collect::<Vec<_>>());
    assert_eq!(mcp.request(12, "ping", json!({}))["result"], json!({}));
}

#[test]
fn usage_errors_exit_with_2() {
    let out = Command::new(env!("CARGO_BIN_EXE_kimchi-mcp")).arg("--bogus").stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
    assert_eq!(out.code(), Some(2));
    let out = Command::new(env!("CARGO_BIN_EXE_kimchi-mcp")).args(["--live", "--file", "x.json"]).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
    assert_eq!(out.code(), Some(2));
}
