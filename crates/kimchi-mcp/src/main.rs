//! `kimchi-mcp`: kimchi as a Model Context Protocol server over stdio (newline-delimited
//! JSON-RPC 2.0). Every registry command is a tool (`clip.addText` becomes `clip_addText`), with
//! its description and JSON schema taken from the registry, so the tools can't drift from what
//! the window, the CLI and the built-in agent accept.
//!
//! In live mode each call runs in the open kimchi window through its loopback bridge, so an
//! agent and the person edit the same project with one undo history. With `--file` the server
//! hosts a session on a project file and saves after every change. MCP requests are always held
//! to Settings › Agent › Permissions. Only protocol goes to stdout; logs go to stderr.

use std::io::Write;
use std::path::PathBuf;

use kimchi_cli::Backend;
use kimchi_control::{Perm, Source, registry};
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;

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

    let mut server = Server { backend };
    let mut lines = tokio::io::BufReader::with_capacity(1 << 16, tokio::io::stdin()).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                eprintln!("kimchi-mcp: {e}");
                break;
            }
        };
        if line.len() > MAX_LINE {
            eprintln!("kimchi-mcp: a request exceeds 64 MiB");
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let Some(reply) = server.handle_line(&line).await else { continue };
        let mut out = std::io::stdout().lock();
        let written = serde_json::to_writer(&mut out, &reply).map_err(std::io::Error::other).and_then(|()| out.write_all(b"\n")).and_then(|()| out.flush());
        if written.is_err() {
            break;
        }
    }
}

fn exit_usage(message: &str) -> ! {
    eprintln!("{message}\n\n{USAGE}");
    std::process::exit(2);
}

struct Server {
    backend: Backend,
}

impl Server {
    async fn handle_line(&mut self, line: &str) -> Option<Value> {
        let frame: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return Some(error(Value::Null, -32700, &format!("Parse error: {e}"))),
        };
        let Some(obj) = frame.as_object() else {
            return Some(error(Value::Null, -32600, "Batch requests are not supported"));
        };
        let id = obj.get("id").cloned();
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || obj.get("method").and_then(Value::as_str).is_none_or(str::is_empty)
            || id.as_ref().is_some_and(|id| !id.is_null() && !id.is_string() && !id.is_number())
        {
            // A response from the client (we send no requests) or a malformed frame.
            if obj.contains_key("result") || obj.contains_key("error") {
                return None;
            }
            return Some(error(id.unwrap_or(Value::Null), -32600, "Invalid JSON-RPC 2.0 request"));
        }
        let method = obj.get("method").and_then(Value::as_str).unwrap_or("");
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        // Notifications (initialized, cancelled, progress) get no answer.
        let id = id?;
        Some(match self.dispatch(method, &params).await {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error(id, code, &message),
        })
    }

    async fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
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
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                Ok(match self.backend.call(spec.name, arguments).await {
                    Ok(result) => {
                        let mut text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
                        if spec.mutates
                            && let Some(path) = self.backend.path()
                        {
                            text.push_str(&format!("\n(saved {})", path.display()));
                        }
                        let mut reply = json!({ "content": [{ "type": "text", "text": text }], "isError": false });
                        if result.is_object() {
                            reply["structuredContent"] = result;
                        }
                        reply
                    }
                    Err(message) => json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
                })
            }
            "resources/list" => Ok(json!({ "resources": RESOURCES.iter().map(|(uri, name, description, _)| json!({
                "uri": uri, "name": name, "description": description, "mimeType": "application/json",
            })).collect::<Vec<_>>() })),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or((-32602, "resources/read needs `uri`".to_string()))?;
                let command = RESOURCES.iter().find(|(u, ..)| *u == uri).map(|(.., c)| *c).ok_or((-32002, format!("Unknown resource `{uri}`")))?;
                let value = self.backend.call(command, json!({})).await.map_err(|e| (-32000, e))?;
                Ok(json!({ "contents": [{
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string_pretty(&value).unwrap_or_default(),
                }] }))
            }
            "prompts/list" => Ok(json!({ "prompts": prompts::PROMPTS.iter().map(|p| json!({
                "name": p.name,
                "description": p.description,
                "arguments": p.arguments.iter().map(|(name, description, required)| json!({
                    "name": name, "description": description, "required": required,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>() })),
            "prompts/get" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((-32602, "prompts/get needs `name`".to_string()))?;
                let prompt = prompts::PROMPTS.iter().find(|p| p.name == name).ok_or((-32602, format!("Unknown prompt `{name}`")))?;
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                for (arg, _, required) in prompt.arguments {
                    if *required && prompts::arg(&arguments, arg, "").is_empty() {
                        return Err((-32602, format!("Prompt `{name}` needs `{arg}`")));
                    }
                }
                Ok(json!({
                    "description": prompt.description,
                    "messages": [{ "role": "user", "content": { "type": "text", "text": (prompt.render)(&arguments) } }],
                }))
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
        format!(
            "kimchi is a video editor with image and video generation in the cut. {mode}\n\
             Start with project_overview (also the resource kimchi://project/overview): one bounded answer with the canvas, every track and its clips, media with how they were generated, markers, running jobs, undo history, what the window shows and problems. Drill down with clip_get, media_get, track_list or generate_jobs.\n\
             Conventions: times are seconds on the timeline; parameters are camelCase; ids and unique names both work wherever an id is expected (clipId \"Title\", trackId \"Video 1\"), and a near miss answers with \"did you mean\". Every edit is one undo step; project_batch runs several commands as one step and rolls back on failure.\n\
             Generation: generate_providers and generate_models show what is ready; generate_submit makes an image or video and puts a placeholder on the timeline that becomes the result; generate_animateFrame, generate_extendClip, generate_bridge, generate_restyleFrame and generate_regenerate work from clips already in the cut. Pass wait=true to get the finished job back; otherwise follow it with generate_wait.\n\
             Motion graphics and 3D: clip_setKeyframes / clip_animate animate any clip (position, scale, rotation, opacity, blur, volume) with easings; motion_addTemplate adds a lower third, title card, counter, chart, 3D title…; motion_add takes a whole 2D (layers) or 3D (camera, lights, objects) scene as JSON; motion_setLayer and motion_setKeyframes edit one. Read motion_guide first; check results with project_renderFrame (it returns a PNG path; several times give one labelled sheet).\n\
             export_start renders the cut. Agent permissions (Settings › Agent › Permissions) decide whether you may import, export, switch projects, generate, change settings or control the app; API keys and the permissions themselves stay with the person."
        )
    }
}

/// Registry-backed resources: uri, name, description, command.
const RESOURCES: [(&str, &str, &str, &str); 5] = [
    ("kimchi://project/overview", "Project overview", "Read it first: the whole open project in one bounded answer, with problems to notice.", "project.overview"),
    ("kimchi://project", "Open project", "The complete open project as JSON (the project file format).", "project.get"),
    ("kimchi://commands", "Commands", "Every command with its parameters, permission and whether it needs the window.", "app.commands"),
    ("kimchi://settings", "Settings", "kimchi's settings: agent permissions, updates, appearance, default models.", "app.settings"),
    ("kimchi://app", "Application", "Version, ffmpeg, folders, and whether the window and the bridge are running.", "app.info"),
];

/// One tool per command an agent can run (person-only commands are left out: they are always refused).
fn tools() -> Vec<Value> {
    registry::commands()
        .iter()
        .filter(|s| s.perm != Perm::PersonOnly)
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
                    "openWorldHint": matches!(spec.family(), "generate" | "handoff") || spec.name == "app.checkUpdates",
                },
            })
        })
        .collect()
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
