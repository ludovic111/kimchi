//! The person's installed Claude Code or Codex CLI as the agent.
//!
//! The CLI runs one turn non-interactively, in its own process group, with
//! kimchi's MCP server (`kimchi-mcp --live`) as its only tools. Its commands
//! come back into this app through the bridge as `Source::Mcp`, where the
//! registry checks the same permissions, and the run shows them from the
//! session's event stream. Credentials stay with the CLI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use kimchi_control::Source;
use kimchi_control::session::Event;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::tools::{SYSTEM_PROMPT, bounded};
use crate::{CliSession, Conversation, Message, ProviderKind, Role, Run};

const CLAUDE_PLACES: &[&str] = &["~/.local/bin/claude", "~/.claude/local/claude", "~/.claude/local/bin/claude", "/opt/homebrew/bin/claude", "/usr/local/bin/claude", "~/.npm-global/bin/claude"];
const CODEX_PLACES: &[&str] = &[
    "/opt/homebrew/bin/codex",
    "/usr/local/bin/codex",
    "~/.local/bin/codex",
    "~/.npm-global/bin/codex",
    "/Applications/Codex.app/Contents/Resources/codex",
    "/Applications/ChatGPT.app/Contents/Resources/codex",
];

/// Built-in tools Claude Code must never use in a kimchi session.
const CLAUDE_DENIED: &str = "Bash,Edit,Write,MultiEdit,NotebookEdit,Read,Glob,Grep,WebFetch,WebSearch,Task,Agent";

/// Codex features that would give it tools besides kimchi's. Set through `-c features.<name>=false`
/// rather than `--disable <name>`: `--disable` refuses names a given Codex doesn't know, `-c` doesn't.
const CODEX_DISABLED: &[&str] = &["apps", "browser_use", "browser_use_external", "in_app_browser", "computer_use", "image_generation", "multi_agent", "plugins"];

/// The file names a command `name` can have here: `claude`, or on Windows `claude.exe`,
/// `claude.cmd` (npm's shims)… in `PATHEXT` order.
fn exe_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let mut names: Vec<String> = exts.split(';').map(str::trim).filter(|e| e.starts_with('.')).map(|e| format!("{name}{}", e.to_ascii_lowercase())).collect();
        if !names.iter().any(|n| n.ends_with(".exe")) {
            names.insert(0, format!("{name}.exe"));
        }
        names
    } else {
        vec![name.to_string()]
    }
}

fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn find_in<'a>(dirs: impl IntoIterator<Item = &'a PathBuf>, name: &str) -> Option<PathBuf> {
    let names = exe_names(name);
    dirs.into_iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

fn on_path(name: &str) -> Option<PathBuf> {
    let file = exe_name(name);
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(&file)).find(|p| p.is_file()))
}

fn home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    }
}

/// The newest `~/.nvm/versions/node/v*/bin` (by version, not by name: v9 < v10).
fn newest_nvm() -> Option<PathBuf> {
    let root = std::env::var_os("NVM_DIR").map(PathBuf::from).unwrap_or_else(|| home("~/.nvm")).join("versions/node");
    let version = |p: &Path| -> Vec<u64> { p.file_name().and_then(|n| n.to_str()).unwrap_or("").trim_start_matches('v').split('.').map(|n| n.parse().unwrap_or(0)).collect() };
    std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).filter(|p| p.join("bin").is_dir()).max_by_key(|p| version(p)).map(|p| p.join("bin"))
}

/// Where Node, npm's global scripts and their version managers put executables. A CLI
/// installed with npm is a `#!/usr/bin/env node` script, so `node` has to be findable too.
fn tool_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/local/bin", "~/.local/bin", "~/.npm-global/bin", "~/.bun/bin", "~/.volta/bin"].iter().map(|p| home(p)).collect();
    dirs.extend(newest_nvm());
    dirs.extend(["~/Library/pnpm", "~/.local/share/pnpm", "~/.local/share/mise/shims", "~/.asdf/shims"].iter().map(|p| home(p)));
    if let Some(appdata) = std::env::var_os("APPDATA").filter(|_| cfg!(windows)) {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

/// The PATH of the person's login shell (what a terminal has), for an app started from the
/// Dock or Finder, which only gets `/usr/bin:/bin:/usr/sbin:/sbin`. Two seconds at most, also
/// when the profile starts something that keeps the pipe open.
#[cfg(unix)]
fn login_shell_path() -> Option<String> {
    let shell = std::env::var_os("SHELL").filter(|s| !s.is_empty()).or_else(|| cfg!(target_os = "macos").then(|| "/bin/zsh".into()))?;
    // fish keeps PATH as a list, which "$PATH" would join with spaces.
    let script = if Path::new(&shell).file_name().is_some_and(|n| n == "fish") { "string join : $PATH" } else { "printf %s \"$PATH\"" };
    let mut child = std::process::Command::new(&shell).args(["-lc", script]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(std::time::Duration::from_secs(2));
    let _ = child.kill();
    let _ = child.wait();
    // Profiles that print something put it before our line.
    let path = out.ok()?.rsplit('\n').next().unwrap_or("").trim().to_string();
    (!path.is_empty()).then_some(path)
}

#[cfg(not(unix))]
fn login_shell_path() -> Option<String> {
    None
}

/// Where children look for programs: the inherited PATH first (a terminal's choice wins), then the
/// login shell's, then the usual tool folders. Computed once.
fn search_dirs() -> &'static [PathBuf] {
    static DIRS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    DIRS.get_or_init(|| {
        let inherited = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
        let shell = login_shell_path().map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
        let mut dirs: Vec<PathBuf> = vec![];
        for d in inherited.into_iter().chain(shell).chain(tool_dirs()) {
            if !d.as_os_str().is_empty() && !dirs.contains(&d) {
                dirs.push(d);
            }
        }
        dirs
    })
}

/// The PATH for a child running `exe`: its own folder first (an npm install keeps the `node` it
/// was installed with there), then [`search_dirs`].
pub(crate) fn child_path(exe: &Path) -> std::ffi::OsString {
    let own = exe.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf);
    let dirs: Vec<&PathBuf> = own.iter().chain(search_dirs().iter().filter(|d| Some(*d) != own.as_ref())).collect();
    std::env::join_paths(dirs).unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
}

/// The installed CLI for `kind` (the search PATH, then the usual install places: an app
/// started from the Finder has a short PATH). `None` for the API providers.
pub fn cli_executable(kind: ProviderKind) -> Option<PathBuf> {
    let (name, places) = match kind {
        ProviderKind::ClaudeCode => ("claude", CLAUDE_PLACES),
        ProviderKind::Codex => ("codex", CODEX_PLACES),
        _ => return None,
    };
    find_in(search_dirs(), name).or_else(|| {
        places.iter().map(|p| home(p)).find_map(|p| {
            let (dir, file) = (p.parent()?.to_path_buf(), p.file_name()?.to_str()?.to_string());
            find_in([&dir], &file)
        })
    })
}

/// A Windows batch file (npm's `.cmd` shims): Rust refuses to pass it arguments with line breaks.
fn is_batch(exe: &Path) -> bool {
    cfg!(windows) && exe.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// `kimchi-mcp`: `$KIMCHI_MCP`, next to this executable, on PATH, else a
/// development build in the workspace's `target/`.
pub fn mcp_executable() -> Option<PathBuf> {
    let file = exe_name("kimchi-mcp");
    if let Some(p) = std::env::var_os("KIMCHI_MCP").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    // `target/debug/deps/<test>` has the binaries one folder up.
    let beside = exe_dir.iter().flat_map(|d| [d.join(&file), d.parent().map(|p| p.join(&file)).unwrap_or_default()]);
    let dev = ["debug", "release"].into_iter().map(|p| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").join(p).join(&file));
    beside.chain(on_path("kimchi-mcp")).chain(dev).find(|p| p.is_file())
}

/// What a CLI needs to reach the running app.
pub(crate) struct Live {
    pub mcp: PathBuf,
    pub control: PathBuf,
}

fn live(run: &Run) -> Result<Live, String> {
    let label = run.config.provider.label();
    if run.session.bridge_port().is_none() {
        return Err(format!(
            "{label} works through kimchi's live bridge, which isn't running. Restart kimchi, or choose an API provider in Settings › Agent."
        ));
    }
    let mcp = mcp_executable().ok_or_else(|| format!("{label} needs kimchi-mcp, which wasn't found next to kimchi. Reinstall kimchi, or choose an API provider in Settings › Agent."))?;
    Ok(Live { mcp, control: kimchi_control::bridge::control_path(&run.session.data_dir) })
}

pub(crate) async fn run(run: &mut Run, prompt: String, mut conv: Conversation) -> Result<String, String> {
    let kind = run.config.provider;
    let exe = cli_executable(kind).ok_or_else(|| match kind {
        ProviderKind::Codex => "Codex isn't installed. Install it (npm install -g @openai/codex), sign in with `codex login`, then try again.".to_string(),
        _ => "Claude Code isn't installed. Install it from claude.com/claude-code, sign in by running `claude` once, then try again.".to_string(),
    })?;
    let live = live(run)?;
    let workspace = run.session.data_dir.join("agent-workspace");
    std::fs::create_dir_all(&workspace).map_err(|e| format!("Couldn't create the agent's folder: {e}"))?;
    // CLI commands can't be intercepted, so the checkpoint is taken up front.
    run.ensure_checkpoint().await;
    conv.prepare_turn();
    // A stopped run still leaves the request in the thread.
    let mut asked = conv.clone();
    asked.messages.push(Message::user(prompt.clone()));
    run.set_conversation(&asked);
    let resume = conv.cli_session.as_ref().filter(|s| s.provider == kind).map(|s| s.id.clone());

    let mut out = turn(run, kind, &exe, &live, &workspace, resume.as_deref(), &prompt, &conv).await;
    if resume.is_some() && out.1.is_err() && !out.0.started {
        // The CLI lost its session (cleared, or another machine): start afresh with the thread as context.
        run.status(format!("Starting a new {} session…", kind.label()));
        out = turn(run, kind, &exe, &live, &workspace, None, &prompt, &conv).await;
    }
    let (outcome, result) = out;
    run.drain_commands(Source::Mcp);
    conv.messages.push(Message::user(prompt));
    if !outcome.reply.is_empty() {
        conv.messages.push(Message::assistant(outcome.reply.clone()));
    }
    conv.cli_session = outcome.session_id.clone().map(|id| CliSession { provider: kind, id }).or(conv.cli_session);
    run.set_conversation(&conv);
    result.map(|_| outcome.reply)
}

/// The thread so far as plain text, for a CLI that can't resume it.
fn with_context(conv: &Conversation, prompt: &str) -> String {
    let lines: Vec<String> = conv
        .messages
        .iter()
        .rev()
        .filter(|m| !m.text().is_empty())
        .take(20)
        .map(|m| format!("{}: {}", if m.role == Role::User { "Person" } else { "Assistant" }, bounded(&m.text(), 1500)))
        .collect();
    if lines.is_empty() {
        return prompt.to_string();
    }
    let lines: Vec<String> = lines.into_iter().rev().collect();
    format!("Conversation so far, for context only:\n{}\n\nNew request:\n{prompt}", lines.join("\n"))
}

#[allow(clippy::too_many_arguments)]
async fn turn(
    run: &mut Run,
    kind: ProviderKind,
    exe: &Path,
    live: &Live,
    workspace: &Path,
    resume: Option<&str>,
    prompt: &str,
    conv: &Conversation,
) -> (Outcome, Result<(), String>) {
    match kind {
        ProviderKind::Codex => codex(run, exe, live, workspace, resume, prompt, conv).await,
        _ => claude(run, exe, live, workspace, resume, prompt, conv).await,
    }
}

// ---- Claude Code --------------------------------------------------------------

/// The `--mcp-config` file: kimchi's server only, pointed at this app's bridge.
fn mcp_config(live: &Live) -> Value {
    json!({ "mcpServers": { "kimchi": {
        "command": live.mcp,
        "args": ["--live"],
        "env": { "KIMCHI_CONTROL": live.control, "KIMCHI_MCP_BUILTIN_AGENT": "1" },
    }}})
}

/// Removes a file when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The arguments for one Claude Code turn (the prompt goes on stdin). `batch`: the executable is
/// a Windows `.cmd` shim, which can't take line breaks, so the system prompt is on one line.
pub(crate) fn claude_args(config: &Path, model: &str, resume: Option<&str>, batch: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        // No user or project settings, hooks or other MCP servers: kimchi's tools only.
        "--setting-sources",
        "",
        "--tools",
        "",
        "--strict-mcp-config",
        "--allowedTools",
        "mcp__kimchi__*",
        "--disallowedTools",
        CLAUDE_DENIED,
    ]
    .into_iter()
    .map(String::from)
    .collect();
    args.push("--mcp-config".into());
    args.push(config.to_string_lossy().into_owned());
    args.push("--append-system-prompt".into());
    let system = format!("{SYSTEM_PROMPT}\nkimchi's commands are the MCP tools mcp__kimchi__family_verb; you have no other tools.");
    args.push(if batch { system.split(['\r', '\n']).map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ") } else { system });
    if !model.is_empty() {
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(id) = resume {
        args.extend(["--resume".into(), id.into()]);
    }
    args
}

async fn claude(run: &mut Run, exe: &Path, live: &Live, workspace: &Path, resume: Option<&str>, prompt: &str, conv: &Conversation) -> (Outcome, Result<(), String>) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let config = TempFile(workspace.join(format!("mcp-{}-{nanos}.json", std::process::id())));
    if let Err(e) = std::fs::write(&config.0, mcp_config(live).to_string()) {
        return (Outcome::default(), Err(format!("Couldn't write the MCP configuration: {e}")));
    }
    let mut cmd = Command::new(exe);
    cmd.args(claude_args(&config.0, &run.config.model(), resume, is_batch(exe))).current_dir(workspace).env("PATH", child_path(exe));
    let input = if resume.is_some() { prompt.to_string() } else { with_context(conv, prompt) };
    run.status("Starting Claude Code…");
    run_child(run, cmd, input, "Claude Code", parse_claude).await
}

fn tool_label(name: &str) -> String {
    name.trim_start_matches("mcp__kimchi__").replacen('_', ".", 1)
}

pub(crate) fn parse_claude(run: &Run, ev: &Value, out: &mut Outcome) {
    match ev["type"].as_str().unwrap_or("") {
        "system" if ev["subtype"] == "init" => {
            out.started = true;
            if let Some(id) = ev["session_id"].as_str() {
                out.session_id = Some(id.to_string());
            }
            let kimchi = ev["mcp_servers"].as_array().into_iter().flatten().find(|s| s["name"] == "kimchi");
            match kimchi.and_then(|s| s["status"].as_str()) {
                // Without its tools the model could only pretend: stop here.
                Some("failed") => out.abort = Some("Claude Code couldn't start kimchi-mcp, so it has no way to edit. Restart kimchi; if it keeps happening, reinstall it.".into()),
                _ => run.status("Claude Code is connected to kimchi"),
            }
        }
        "stream_event" => {
            out.partials = true;
            let s = &ev["event"];
            match s["type"].as_str().unwrap_or("") {
                "message_start" => run.break_text(),
                "content_block_start" if s["content_block"]["type"] == "tool_use" => {
                    run.status(format!("Running {}…", tool_label(s["content_block"]["name"].as_str().unwrap_or("a command"))));
                }
                "content_block_delta" if s["delta"]["type"] == "text_delta" => {
                    out.say(run, s["delta"]["text"].as_str().unwrap_or(""));
                }
                _ => {}
            }
        }
        "assistant" => {
            // Without partial messages (older CLIs), the text arrives here whole.
            if !out.partials {
                run.break_text();
                for b in ev["message"]["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => out.say(run, b["text"].as_str().unwrap_or("")),
                        Some("tool_use") => run.status(format!("Running {}…", tool_label(b["name"].as_str().unwrap_or("a command")))),
                        _ => {}
                    }
                }
            }
        }
        "result" => {
            out.completed = true;
            if let Some(id) = ev["session_id"].as_str() {
                out.session_id = Some(id.to_string());
            }
            let u = &ev["usage"];
            run.usage(u["input_tokens"].as_u64().unwrap_or(0) + u["cache_read_input_tokens"].as_u64().unwrap_or(0), u["output_tokens"].as_u64().unwrap_or(0));
            if ev["is_error"].as_bool() == Some(true) || ev["subtype"].as_str().is_some_and(|s| s.starts_with("error")) {
                let why = ev["result"].as_str().filter(|r| !r.is_empty()).or_else(|| ev["subtype"].as_str()).unwrap_or("Claude Code reported an error");
                out.error = Some(bounded(why, 2000));
            } else if out.reply.is_empty()
                && let Some(r) = ev["result"].as_str()
            {
                out.say(run, r);
            }
        }
        _ => {}
    }
}

// ---- Codex ------------------------------------------------------------------------

/// A TOML string (JSON's escaping is valid TOML for basic strings).
fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// The arguments for one `codex exec` turn (the prompt goes on stdin: `-`). `others`: the
/// person's own MCP servers (when Codex runs on their home), switched off for the turn.
pub(crate) fn codex_args(live: &Live, model: &str, resume: Option<&str>, others: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec!["exec".into(), "--json".into(), "--skip-git-repo-check".into(), "--sandbox".into(), "read-only".into()];
    let features = CODEX_DISABLED.iter().map(|f| format!("features.{f}=false"));
    let servers = others.iter().filter(|n| n.as_str() != "kimchi").map(|n| format!("mcp_servers.{n}.enabled=false"));
    for c in [
        "approval_policy=\"never\"".to_string(),
        "features.shell_tool=false".into(),
        "web_search=\"disabled\"".into(),
        format!("mcp_servers.kimchi.command={}", toml_str(&live.mcp.to_string_lossy())),
        "mcp_servers.kimchi.args=[\"--live\"]".into(),
        format!("mcp_servers.kimchi.env.KIMCHI_CONTROL={}", toml_str(&live.control.to_string_lossy())),
        "mcp_servers.kimchi.env.KIMCHI_MCP_BUILTIN_AGENT=\"1\"".into(),
        // Without it, Codex refuses every tool marked destructive (clip_delete, track_remove…)
        // under approval_policy "never": kimchi's own permissions decide instead.
        "mcp_servers.kimchi.default_tools_approval_mode=\"approve\"".into(),
        "mcp_servers.kimchi.startup_timeout_sec=30".into(),
        // Generation commands can wait for a render.
        "mcp_servers.kimchi.tool_timeout_sec=900".into(),
    ]
    .into_iter()
    .chain(features)
    .chain(servers)
    {
        args.extend(["-c".into(), c]);
    }
    if !model.is_empty() {
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(id) = resume {
        args.extend(["resume".into(), id.into()]);
    }
    args.push("-".into());
    args
}

fn codex_original_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home("~/.codex"))
}

/// A Codex home of kimchi's own (no user MCP servers, hooks or plugins), sharing the person's
/// sign-in through `auth.json` (a link, or a copy on Windows). Kept between runs so `exec resume`
/// finds its threads.
struct CodexHome {
    dir: PathBuf,
    /// The person's own `auth.json`.
    auth: PathBuf,
}

impl CodexHome {
    fn new(run: &Run) -> Option<Self> {
        let auth = codex_original_home().join("auth.json");
        if !auth.is_file() {
            // Signed in through the keychain: use the person's own home.
            return None;
        }
        let dir = run.session.data_dir.join("agent-codex-home");
        std::fs::create_dir_all(&dir).ok()?;
        let link = dir.join("auth.json");
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&auth, &link).ok()?;
        #[cfg(not(unix))]
        std::fs::copy(&auth, &link).ok()?;
        Some(Self { dir, auth })
    }

    /// When Codex refreshed its tokens during the turn, our `auth.json` is the only valid one
    /// (refresh tokens are single use): hand it back, or the person's own Codex is signed out.
    /// A link written through needs nothing; a copy, or a link Codex replaced, does.
    fn sync_back(&self) {
        let ours = self.dir.join("auth.json");
        let Ok(meta) = std::fs::symlink_metadata(&ours) else { return };
        if !meta.is_file() {
            return;
        }
        let newer = match (meta.modified(), std::fs::metadata(&self.auth).and_then(|m| m.modified())) {
            (Ok(a), Ok(b)) => a > b,
            _ => false,
        };
        if newer && std::fs::read(&ours).ok() != std::fs::read(&self.auth).ok() {
            let part = self.auth.with_extension("json.kimchi");
            if std::fs::copy(&ours, &part).is_ok() && std::fs::rename(&part, &self.auth).is_err() {
                let _ = std::fs::remove_file(&part);
            }
        }
    }
}

impl Drop for CodexHome {
    fn drop(&mut self) {
        self.sync_back();
    }
}

/// The MCP servers named in a Codex `config.toml` (`[mcp_servers.<name>]` tables, and keys of a
/// plain `[mcp_servers]` table), as written: quoted names stay quoted, for `-c` paths.
pub(crate) fn codex_mcp_servers(config: &str) -> Vec<String> {
    let mut names: Vec<String> = vec![];
    let mut in_table = false;
    let mut push = |n: &str| {
        let n = n.trim();
        if !n.is_empty() && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    for line in config.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').filter(|l| !l.starts_with('[')) {
            let header = header.split(']').next().unwrap_or("").trim();
            in_table = header == "mcp_servers";
            if let Some(rest) = header.strip_prefix("mcp_servers.") {
                push(&toml_first_key(rest));
            }
        } else if in_table && let Some((key, _)) = line.split_once('=') {
            push(&toml_first_key(key));
        }
    }
    names
}

/// The first segment of a dotted TOML key: `node_repl`, `"a.b"` of `"a.b".env`.
fn toml_first_key(path: &str) -> String {
    let path = path.trim();
    match path.chars().next() {
        Some(q @ ('"' | '\'')) => path[1..].find(q).map(|end| path[..end + 2].to_string()).unwrap_or_default(),
        _ => path.split('.').next().unwrap_or("").trim().to_string(),
    }
}

async fn codex(run: &mut Run, exe: &Path, live: &Live, workspace: &Path, resume: Option<&str>, prompt: &str, conv: &Conversation) -> (Outcome, Result<(), String>) {
    let own = CodexHome::new(run);
    // On the person's own home, their MCP servers are there too: off for this turn.
    let others = match &own {
        Some(_) => vec![],
        None => std::fs::read_to_string(codex_original_home().join("config.toml")).map(|c| codex_mcp_servers(&c)).unwrap_or_default(),
    };
    let mut cmd = Command::new(exe);
    cmd.args(codex_args(live, &run.config.model(), resume, &others)).current_dir(workspace).env("PATH", child_path(exe));
    if let Some(h) = &own {
        cmd.env("CODEX_HOME", &h.dir);
    }
    let input = if resume.is_some() {
        prompt.to_string()
    } else {
        format!("{SYSTEM_PROMPT}\nkimchi's commands are the tools of the `kimchi` MCP server; use no other tools.\n\n{}", with_context(conv, prompt))
    };
    run.status("Starting Codex…");
    // `own` hands the sign-in back when dropped, also when the run is cancelled.
    run_child(run, cmd, input, "Codex", parse_codex).await
}

pub(crate) fn parse_codex(run: &Run, ev: &Value, out: &mut Outcome) {
    match ev["type"].as_str().unwrap_or("") {
        "thread.started" => {
            out.started = true;
            if let Some(id) = ev["thread_id"].as_str() {
                out.session_id = Some(id.to_string());
            }
            run.status("Codex is connected to kimchi");
        }
        "turn.started" => out.started = true,
        "item.started" | "item.updated" | "item.completed" => {
            let item = &ev["item"];
            let id = item["id"].as_str().unwrap_or("").to_string();
            match item["type"].as_str().unwrap_or("") {
                "agent_message" => {
                    let text = item["text"].as_str().unwrap_or("");
                    let seen = out.items.get(&id).copied();
                    if seen.is_none() {
                        run.break_text();
                    }
                    let seen = seen.unwrap_or(0);
                    if text.len() > seen && text.is_char_boundary(seen) {
                        out.say(run, &text[seen..]);
                        out.items.insert(id, text.len());
                    } else {
                        out.items.entry(id).or_insert(0);
                    }
                }
                "mcp_tool_call" if ev["type"] == "item.started" => {
                    run.status(format!("Running {}…", item["tool"].as_str().map(tool_label).unwrap_or_else(|| "a command".into())));
                }
                "error" => run.status(item["message"].as_str().unwrap_or("Codex reported a problem")),
                _ => {}
            }
        }
        "turn.completed" => {
            out.completed = true;
            let u = &ev["usage"];
            run.usage(u["input_tokens"].as_u64().unwrap_or(0), u["output_tokens"].as_u64().unwrap_or(0));
        }
        "turn.failed" | "error" => {
            let msg = ev["error"]["message"].as_str().or_else(|| ev["message"].as_str()).unwrap_or("Codex reported a failed turn");
            out.error = Some(bounded(msg, 2000));
        }
        _ => {}
    }
}

// ---- the child process ------------------------------------------------------------

/// What one CLI turn produced.
#[derive(Default)]
pub(crate) struct Outcome {
    pub reply: String,
    pub session_id: Option<String>,
    pub error: Option<String>,
    /// The CLI got as far as opening a session.
    pub started: bool,
    /// The CLI reported the end of its turn.
    pub completed: bool,
    /// Stop the CLI now, with this error.
    pub abort: Option<String>,
    /// The CLI streams partial messages (text arrives as deltas).
    partials: bool,
    items: HashMap<String, usize>,
}

impl Outcome {
    fn say(&mut self, run: &Run, text: &str) {
        if text.is_empty() {
            return;
        }
        if run.text_break_pending() && !self.reply.is_empty() {
            self.reply.push_str("\n\n");
        }
        self.reply.push_str(text);
        run.text(text);
    }
}

/// Kills the child's whole process tree (the CLI and its `kimchi-mcp`) when dropped, which is
/// also what cancelling a run does. On Unix the child leads its own process group, which outlives
/// it; on Windows `taskkill /T` walks the tree from the child, so it has to run while the child
/// is still alive ([`Group::kill`] before waiting).
struct Group {
    pid: Option<u32>,
    /// The child was waited for: on Windows its tree can't be found any more.
    reaped: bool,
}

impl Group {
    fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 1) {
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            // SAFETY: the child was started as the leader of its own process group,
            // so -pid names that group only; 9 is SIGKILL on macOS and Linux.
            unsafe {
                kill(-pid, 9);
            }
        }
        #[cfg(windows)]
        if let Some(pid) = self.pid.take().filter(|_| !self.reaped) {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .status();
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}

type Parser = fn(&Run, &Value, &mut Outcome);

pub(crate) async fn run_child(run: &mut Run, mut cmd: Command, input: String, label: &str, parse: Parser) -> (Outcome, Result<(), String>) {
    let mut out = Outcome::default();
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (out, Err(format!("Couldn't start {label}: {e}"))),
    };
    let mut group = Group { pid: child.id(), reaped: false };
    if let Some(mut stdin) = child.stdin.take() {
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let stderr = child.stderr.take().map(|mut e| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = e.read_to_end(&mut buf).await;
            let s = String::from_utf8_lossy(&buf).trim().to_string();
            let start = s.len().saturating_sub(4000);
            let start = (start..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(s.len());
            s[start..].to_string()
        })
    });
    let Some(stdout) = child.stdout.take() else { return (out, Err(format!("{label} has no output stream"))) };
    let mut lines = BufReader::new(stdout).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    if let Ok(ev) = serde_json::from_str::<Value>(&line) {
                        parse(run, &ev, &mut out);
                        if let Some(e) = out.abort.take() {
                            out.error = Some(e);
                            group.kill();
                            let _ = child.start_kill();
                            break;
                        }
                    } else if !line.trim().is_empty() {
                        tracing::debug!("{label}: {line}");
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    out.error.get_or_insert_with(|| format!("Couldn't read {label}'s output: {e}"));
                    // It may go on writing to a pipe nobody reads: stop it, or the wait never ends.
                    group.kill();
                    let _ = child.start_kill();
                    break;
                }
            },
            ev = run.commands.recv() => {
                if let Ok(Event::Command { record }) = ev
                    && record.source == Source::Mcp
                {
                    run.command(record, None);
                }
            }
        }
    }
    let status = child.wait().await;
    group.reaped = true;
    // Whatever the CLI left running (its MCP server) goes with it, and lets go of stderr.
    drop(group);
    let stderr = match stderr {
        Some(t) => tokio::time::timeout(std::time::Duration::from_secs(2), t).await.ok().and_then(Result::ok).unwrap_or_default(),
        None => String::new(),
    };
    let signin = match label {
        "Codex" => "Check that Codex is signed in: run `codex login` in a terminal.",
        _ => "Check that Claude Code is signed in: run `claude` once in a terminal.",
    };
    let result = if let Some(e) = out.error.clone() {
        Err(if e.starts_with(label) { e } else { format!("{label}: {e}") })
    } else if !status.as_ref().is_ok_and(|s| s.success()) {
        let detail = if stderr.is_empty() { signin.to_string() } else { bounded(&stderr, 1500) };
        Err(format!("{label} stopped with an error. {detail}"))
    } else if !out.completed {
        Err(format!("{label} ended without finishing its turn. Finished edits stay."))
    } else {
        Ok(())
    };
    if let Err(e) = &result {
        tracing::warn!("{e}");
    }
    (out, result)
}
