//! `kimchi-mcp`: kimchi as a Model Context Protocol server over stdio (newline-delimited
//! JSON-RPC 2.0). Every registry command is a tool (`clip.addText` becomes `clip_addText`), with
//! its description and JSON schema taken from the registry, so the tools can't drift from what
//! the window, the CLI and the built-in agent accept.
//!
//! In live mode each call runs in the open kimchi window through its loopback bridge, so an
//! agent and the person edit the same project with one undo history. With `--file` the server
//! hosts a session on a project file and saves after every change. MCP requests are always held
//! to Settings › Agent › Permissions. Only protocol goes to stdout; logs go to stderr.
//!
//! Requests are served concurrently: each one is a task, so `ping` is answered while a
//! generation runs, and `notifications/cancelled` aborts the request it names. One writer
//! thread owns stdout, one JSON line at a time.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kimchi_cli::Backend;
use kimchi_control::{Perm, Source, registry};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

mod prompts;

/// Protocol revisions we speak, oldest first; an unknown request gets the newest.
const PROTOCOLS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const MAX_LINE: usize = 64 * 1024 * 1024;

const USAGE: &str = "kimchi-mcp — Model Context Protocol server for kimchi (stdio)

USAGE
  kimchi-mcp                  drive the running kimchi app; if it isn't running, host a session
                              on the library in this process
  kimchi-mcp --live           drive the running app only (calls fail with a hint while it is closed)
  kimchi-mcp --file <path>    host that project file in this process and save after every change;
                              project_create makes it if it doesn't exist
  kimchi-mcp --headless       host a session on the library in this process

Register it with an MCP client, for example Claude Code:
  claude mcp add kimchi -- /path/to/kimchi-mcp --live
`kimchi-cli mcp-config` prints this line and the others with this computer's paths.";

#[tokio::main]
async fn main() {
    // The plugin scanner runs this program to probe one bundle in a child process.
    if let Some(code) = kimchi_audio::plugins::scan_child() {
        std::process::exit(code);
    }
    if let Some(code) = kimchi_media::render::plugins::scan_child() {
        std::process::exit(code);
    }
    let mut file: Option<PathBuf> = None;
    let (mut live, mut headless) = (false, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            "--version" | "-V" => {
                println!("kimchi-mcp {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--file" | "-f" => match args.next().filter(|p| !p.starts_with("--")) {
                Some(p) => file = Some(PathBuf::from(p)),
                None => exit_usage("--file needs a path"),
            },
            "--live" => live = true,
            "--headless" => headless = true,
            _ => exit_usage(&format!("Unknown option `{arg}`")),
        }
    }
    if usize::from(file.is_some()) + usize::from(live) + usize::from(headless) > 1 {
        exit_usage("--file, --live and --headless are exclusive");
    }
    let backend = match (file, live, headless) {
        (Some(path), _, _) => Backend::file(&path, Source::Mcp).await,
        (None, true, _) => {
            if let Err(e) = Backend::live("mcp").await {
                eprintln!("kimchi-mcp: {e}\nkimchi-mcp: waiting for the app; each call tries again");
            }
            Ok(Backend::live_lazy("mcp"))
        }
        (None, false, true) => Backend::headless(Source::Mcp).await,
        (None, false, false) => match Backend::live("mcp").await {
            Ok(b) => Ok(b),
            Err(e) => {
                eprintln!("kimchi-mcp: {e}\nkimchi-mcp: hosting a session on the library in this process instead");
                Backend::headless(Source::Mcp).await
            }
        },
    };
    let backend = match backend {
        Ok(b) => b,
        Err(e) => {
            eprintln!("kimchi-mcp: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("kimchi-mcp {}: {} mode{}", env!("CARGO_PKG_VERSION"), backend.mode(), backend.path().map(|p| format!(" on {}", p.display())).unwrap_or_default());

    let (out, writer) = protocol_out();
    let server = Arc::new(Server { backend, seen: Default::default() });
    // Requests in flight by id (as JSON text), to cancel them.
    let running: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> = Arc::default();
    let mut stdin = tokio::io::BufReader::with_capacity(1 << 16, tokio::io::stdin());
    loop {
        let line = match read_line(&mut stdin, MAX_LINE).await {
            Ok(Line::Text(l)) => l,
            Ok(Line::TooLong) => {
                eprintln!("kimchi-mcp: skipped a request over 64 MiB");
                out.send(error(Value::Null, -32600, "The request exceeds 64 MiB"));
                continue;
            }
            Ok(Line::Eof) => break,
            Err(e) => {
                eprintln!("kimchi-mcp: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let frame = match server.frame(&line) {
            Ok(f) => f,
            Err(reply) => {
                out.send(reply);
                continue;
            }
        };
        let Some(Frame { id, method, params }) = frame else { continue };
        let Some(id) = id else {
            // Notifications get no answer; a cancelled request gets none either.
            if method == "notifications/cancelled"
                && let Some(task) = params.get("requestId").and_then(|r| running.lock().unwrap_or_else(|e| e.into_inner()).remove(&r.to_string()))
            {
                task.abort();
            }
            continue;
        };
        let key = id.to_string();
        let (server, out, done) = (server.clone(), out.clone(), running.clone());
        // Held while spawning, so a quick task can't remove its entry before it is there.
        let mut tasks = running.lock().unwrap_or_else(|e| e.into_inner());
        let task_key = key.clone();
        let task = tokio::spawn(async move {
            let reply = match server.dispatch(&method, &params).await {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err((code, message)) => error(id, code, &message),
            };
            done.lock().unwrap_or_else(|e| e.into_inner()).remove(&task_key);
            out.send(reply);
        });
        tasks.insert(key, task);
    }
    // The client is gone; let calls already running finish (a file-mode edit saves its file),
    // up to a few seconds.
    let left: Vec<_> = running.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, t)| t).collect();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for t in left {
            let _ = t.await;
        }
    })
    .await;
    // Out with the answers already given, then stop (a call still running keeps a sender: bounded).
    drop(out);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), tokio::task::spawn_blocking(move || writer.join())).await;
}

/// One line of input, read without ever holding more than `max` bytes of it.
#[derive(Debug, PartialEq)]
enum Line {
    Text(String),
    /// Over `max`: skipped up to its end.
    TooLong,
    Eof,
}

async fn read_line(r: &mut (impl AsyncBufRead + Unpin), max: usize) -> std::io::Result<Line> {
    let mut buf: Vec<u8> = vec![];
    let mut over = false;
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(match (over, buf.is_empty()) {
                (true, _) => Line::TooLong,
                (false, true) => Line::Eof,
                (false, false) => Line::Text(String::from_utf8_lossy(&buf).into_owned()),
            });
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let take = newline.map_or(chunk.len(), |i| i + 1);
        if !over && buf.len() + take > max + 1 {
            over = true;
            buf = vec![];
        }
        if !over {
            buf.extend_from_slice(&chunk[..take]);
        }
        r.consume(take);
        if newline.is_some() {
            if over {
                return Ok(Line::TooLong);
            }
            while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                buf.pop();
            }
            return Ok(Line::Text(String::from_utf8_lossy(&buf).into_owned()));
        }
    }
}

/// The protocol's way out: a thread that writes one JSON line per message. When stdout is gone
/// the client is too, and the server stops.
#[derive(Clone)]
struct Out(std::sync::mpsc::Sender<Value>);

impl Out {
    fn send(&self, v: Value) {
        let _ = self.0.send(v);
    }
}

fn protocol_out() -> (Out, std::thread::JoinHandle<()>) {
    let mut stdout = protocol_stdout();
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    let writer = std::thread::spawn(move || {
        for v in rx {
            let written = serde_json::to_writer(&mut stdout, &v).map_err(std::io::Error::other).and_then(|()| stdout.write_all(b"\n")).and_then(|()| stdout.flush());
            if written.is_err() {
                std::process::exit(0);
            }
        }
    });
    (Out(tx), writer)
}

/// stdout for the protocol only. On Unix the real stdout is kept aside and fd 1 points at
/// stderr from here on, so a stray print (ours, a library's or a child process's, like ffmpeg)
/// can never corrupt the JSON-RPC stream.
#[cfg(unix)]
fn protocol_stdout() -> Box<dyn Write + Send> {
    use std::os::fd::FromRawFd;
    unsafe extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(old: i32, new: i32) -> i32;
    }
    // SAFETY: plain descriptor calls on 0–2, which exist for the life of the process; the
    // duplicate is owned by the File alone.
    unsafe {
        let fd = dup(1);
        if fd >= 0 && dup2(2, 1) >= 0 {
            return Box::new(std::io::BufWriter::new(std::fs::File::from_raw_fd(fd)));
        }
    }
    Box::new(std::io::stdout())
}

#[cfg(not(unix))]
fn protocol_stdout() -> Box<dyn Write + Send> {
    Box::new(std::io::stdout())
}

fn exit_usage(message: &str) -> ! {
    eprintln!("{message}\n\n{USAGE}");
    std::process::exit(2);
}

struct Server {
    backend: Backend,
    /// The live context the client last saw: the summary and the change it goes up to.
    seen: tokio::sync::Mutex<Option<(String, u64)>>,
}

/// A request (with an id) or a notification.
struct Frame {
    id: Option<Value>,
    method: String,
    params: Value,
}

impl Server {
    /// Reads one line: `Ok(None)` for something to ignore (a client's response), `Err` for the
    /// error answer to a bad frame.
    fn frame(&self, line: &str) -> Result<Option<Frame>, Value> {
        let frame: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return Err(error(Value::Null, -32700, &format!("Parse error: {e}"))),
        };
        let Some(obj) = frame.as_object() else {
            return Err(error(Value::Null, -32600, "Batch requests are not supported"));
        };
        let id = obj.get("id").cloned();
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || obj.get("method").and_then(Value::as_str).is_none_or(str::is_empty)
            || id.as_ref().is_some_and(|id| !id.is_null() && !id.is_string() && !id.is_number())
        {
            // A response from the client (we send no requests) or a malformed frame.
            if obj.contains_key("result") || obj.contains_key("error") {
                return Ok(None);
            }
            return Err(error(id.unwrap_or(Value::Null), -32600, "Invalid JSON-RPC 2.0 request"));
        }
        let method = obj.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        Ok(Some(Frame { id, method, params }))
    }

    async fn dispatch(&self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = if PROTOCOLS.contains(&requested) { requested } else { PROTOCOLS[PROTOCOLS.len() - 1] };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {
                        "tools": { "listChanged": false },
                        "resources": { "subscribe": false, "listChanged": false },
                        "prompts": { "listChanged": false },
                    },
                    "serverInfo": { "name": "kimchi", "title": "kimchi video editor", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": self.instructions(),
                }))
            }
            "ping" => Ok(json!({})),
            "logging/setLevel" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((-32602, "tools/call needs `name`".to_string()))?;
                let spec = registry::commands()
                    .iter()
                    .find(|s| s.tool_name() == name || s.name == name)
                    .ok_or_else(|| (-32602, format!("Unknown tool `{name}`")))?;
                if for_builtin_agent() && spec.family() == "agent" {
                    return Err((-32602, format!("Unknown tool `{name}`: the built-in agent doesn't drive itself")));
                }
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                Ok(match self.backend.call(spec.name, arguments).await {
                    Ok(result) => {
                        let mut text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
                        if spec.mutates
                            && let Some(path) = self.backend.path()
                        {
                            text.push_str(&format!("\n(saved {})", path.display()));
                        }
                        // A frame, a media look or a screenshot: the client's model gets the picture itself.
                        let mut pictures = vec![];
                        for path in kimchi_control::vision::pictures_in(spec.name, &result) {
                            match kimchi_control::vision::picture(&path).await {
                                Ok(p) => pictures.push(json!({ "type": "image", "data": p.data, "mimeType": p.media_type })),
                                Err(e) => text.push_str(&format!("\n(The picture couldn't be attached: {e})")),
                            }
                        }
                        let context = self.context_update(spec.name).await;
                        let mut reply = json!({ "content": std::iter::once(json!({ "type": "text", "text": text })).chain(pictures).chain(context).collect::<Vec<_>>(), "isError": false });
                        if result.is_object() {
                            reply["structuredContent"] = result;
                        }
                        reply
                    }
                    Err(message) => {
                        let context = self.context_update(spec.name).await;
                        json!({ "content": std::iter::once(json!({ "type": "text", "text": message })).chain(context).collect::<Vec<_>>(), "isError": true })
                    }
                })
            }
            "resources/list" => {
                let mut list: Vec<Value> = RESOURCES.iter().map(|(uri, name, description, _)| json!({
                    "uri": uri, "name": name, "description": description, "mimeType": "application/json",
                })).collect();
                list.push(json!({ "uri": "kimchi://brief", "name": "Brief", "description": "How to work in kimchi: the project model, the quality bar, the finish routine (the same as these instructions).", "mimeType": "text/markdown" }));
                list.extend(kimchi_control::harness::skills().iter().map(|s| json!({
                    "uri": format!("kimchi://skills/{}", s.name), "name": format!("Skill: {}", s.title), "description": s.when, "mimeType": "text/markdown",
                })));
                Ok(json!({ "resources": list }))
            }
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [{
                "uriTemplate": "kimchi://skills/{name}", "name": "Skill", "description": "A playbook for a video job (harness.skills lists them).", "mimeType": "text/markdown",
            }] })),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or((-32602, "resources/read needs `uri`".to_string()))?;
                let markdown = |text: String| json!({ "contents": [{ "uri": uri, "mimeType": "text/markdown", "text": text }] });
                if uri == "kimchi://brief" {
                    return Ok(markdown(kimchi_control::harness::brief()));
                }
                if let Some(name) = uri.strip_prefix("kimchi://skills/") {
                    return kimchi_control::harness::skill(name).map(|t| markdown(t.to_string())).map_err(|e| (-32002, e));
                }
                let command = RESOURCES.iter().find(|(u, ..)| *u == uri).map(|(.., c)| *c).ok_or((-32002, format!("Unknown resource `{uri}`")))?;
                let value = self.backend.call(command, json!({})).await.map_err(|e| (-32000, e))?;
                Ok(json!({ "contents": [{
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string_pretty(&value).unwrap_or_default(),
                }] }))
            }
            "prompts/list" => Ok(json!({ "prompts": prompts::list() })),
            "prompts/get" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((-32602, "prompts/get needs `name`".to_string()))?;
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                prompts::get(name, &arguments).ok_or((-32602, format!("Unknown prompt `{name}`")))
            }
            "completion/complete" => Ok(json!({ "completion": { "values": [] } })),
            _ => Err((-32601, format!("Method not found: {method}"))),
        }
    }

    fn instructions(&self) -> String {
        let mode = match &self.backend {
            Backend::Live { .. } => "Live mode: every tool runs in the open kimchi window. The person may be editing at the same time; you share one undo history (history_undo undoes the last step, whoever made it).".to_string(),
            Backend::Local { file: Some(p), .. } => format!(
                "File mode on {}: the project file is saved after every change. Commands that need the window (playback, selection, panels, screenshots) are unavailable; exports and generations wait until they finish.",
                p.display()
            ),
            Backend::Local { .. } => "Headless mode on the library (the app isn't running): project_list, project_open or project_create first. Commands that need the window are unavailable; exports and generations wait until they finish.".into(),
        };
        if for_builtin_agent() {
            // The built-in agent's system prompt already holds the brief.
            return format!("kimchi's commands for the Agent panel's model. {mode} Your system prompt has kimchi's brief; harness_skill loads a playbook.");
        }
        format!(
            "{}\n## Over MCP\n\n{mode}\n\nkimchi's commands are this server's tools, with an underscore for the dot (the command clip.addText is the tool clip_addText), as they are written above. The overview is also the resource kimchi://project/overview; each skill is a prompt and the resource kimchi://skills/<name>. When the project or the window changed since your last call, a tool result ends with an updated <context> block that says what the person changed meanwhile. Agent permissions (Settings › Agent › Permissions) decide whether you may import, export, switch projects, generate, change settings or control the app; API keys and the permissions themselves stay with the person.",
            kimchi_control::harness::as_tools(&kimchi_control::harness::brief())
        )
    }
}

impl Server {
    /// An updated `<context>` block for a tool result, when the project or the window changed
    /// since the client last saw it (`KIMCHI_MCP_CONTEXT=0` turns this off).
    async fn context_update(&self, command: &str) -> Option<Value> {
        if !live_context() || command.starts_with("harness.") {
            return None;
        }
        let mut seen = self.seen.lock().await;
        let since = seen.as_ref().map(|(_, seq)| *seq);
        let v = self.backend.call("harness.context", since.map_or(json!({}), |s| json!({ "since": s }))).await.ok()?;
        let summary = v["summary"].as_str().unwrap_or("").to_string();
        let seq = v["seq"].as_u64().unwrap_or(0);
        let changes = v["changes"].as_str().filter(|c| !c.is_empty());
        // A closed app or no open project: nothing to summarise.
        if summary.starts_with("No project is open") {
            return None;
        }
        // The first block is news unless the built-in agent framed the request with it already.
        let first = seen.is_none();
        let changed = seen.as_ref().is_none_or(|(s, _)| *s != summary) || changes.is_some();
        *seen = Some((summary, seq));
        if !changed || (first && for_builtin_agent()) {
            return None;
        }
        Some(json!({ "type": "text", "text": format!("<context>\nUpdated after this call.\n{}\n</context>", v["context"].as_str().unwrap_or("")) }))
    }
}

fn live_context() -> bool {
    std::env::var("KIMCHI_MCP_CONTEXT").map_or(true, |v| v != "0")
}

/// Registry-backed resources: uri, name, description, command.
const RESOURCES: [(&str, &str, &str, &str); 5] = [
    ("kimchi://project/overview", "Project overview", "Read it first: the whole open project in one bounded answer, with problems to notice.", "project.overview"),
    ("kimchi://project", "Open project", "The complete open project as JSON (the project file format).", "project.get"),
    ("kimchi://commands", "Commands", "Every command with its parameters, permission and whether it needs the window.", "app.commands"),
    ("kimchi://settings", "Settings", "kimchi's settings: agent permissions, updates, appearance, default models.", "app.settings"),
    ("kimchi://app", "Application", "Version, ffmpeg, folders, and whether the window and the bridge are running.", "app.info"),
];

/// Started by the app's built-in agent (Claude Code or Codex as the panel's model): it gets no
/// `agent_*` tools, which would drive the agent itself.
fn for_builtin_agent() -> bool {
    std::env::var_os("KIMCHI_MCP_BUILTIN_AGENT").is_some_and(|v| !v.is_empty() && v != "0")
}

/// One tool per command an agent can run (person-only commands are left out: they are always refused).
fn tools() -> Vec<Value> {
    let builtin = for_builtin_agent();
    registry::commands()
        .iter()
        .filter(|s| s.perm != Perm::PersonOnly && !(builtin && s.family() == "agent"))
        .map(|spec| {
            let mut description = spec.doc.to_string();
            if spec.perm != Perm::Edit {
                description.push_str(&format!(
                    " Needs the \"{}\" agent permission (Settings › Agent › Permissions).",
                    spec.perm.label()
                ));
            }
            if spec.needs_window {
                description.push_str(" Needs the running kimchi window (kimchi-mcp --live).");
            }
            let destructive = spec.mutates && ["delete", "remove", "close", "cancel", "quit", "revertTo", "open", "create"].iter().any(|w| spec.name.contains(w));
            json!({
                "name": spec.tool_name(),
                "title": spec.name,
                "description": description,
                "inputSchema": registry::input_schema(spec),
                "annotations": {
                    "readOnlyHint": !spec.mutates,
                    "destructiveHint": destructive,
                    "idempotentHint": !spec.mutates,
                    "openWorldHint": matches!(spec.family(), "generate" | "handoff") || matches!(spec.name, "app.checkUpdates" | "agent.send"),
                },
            })
        })
        .collect()
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lines_are_read_within_the_limit() {
        let input: &[u8] = b"{\"a\":1}\r\n0123456789abcdef\nshort\nlast";
        let mut r = tokio::io::BufReader::with_capacity(4, input);
        assert_eq!(read_line(&mut r, 10).await.unwrap(), Line::Text("{\"a\":1}".into()));
        // Skipped to its end, and the next line is whole.
        assert_eq!(read_line(&mut r, 10).await.unwrap(), Line::TooLong);
        assert_eq!(read_line(&mut r, 10).await.unwrap(), Line::Text("short".into()));
        assert_eq!(read_line(&mut r, 10).await.unwrap(), Line::Text("last".into()));
        assert_eq!(read_line(&mut r, 10).await.unwrap(), Line::Eof);
    }
}
