//! `kimchi-cli`: every command of kimchi's registry from the terminal, on the running app or on
//! a project file. The window, the built-in agent, this CLI and `kimchi-mcp` run the same
//! commands with the same undo history; see docs/AI_CONTROL.md.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use kimchi_cli::{Backend, Invocation, kimchi_control};
use kimchi_control::{Source, registry};
use serde_json::{Value, json};

/// `println!` that leaves quietly when stdout is closed (`kimchi-cli commands | head`) instead of
/// panicking. Defined before `mod generate` so it can use it too.
macro_rules! say {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        let mut out = std::io::stdout().lock();
        if let Err(e) = writeln!(out, $($t)*) {
            crate::stdout_failed(e);
        }
    }};
}

mod generate;

/// Nobody reads our output any more: stop, quietly for a closed pipe (128 + SIGPIPE, as a shell
/// reports it).
fn stdout_failed(e: std::io::Error) -> ! {
    if e.kind() != std::io::ErrorKind::BrokenPipe {
        eprintln!("error: couldn't write the output: {e}");
        std::process::exit(1);
    }
    std::process::exit(141)
}

const USAGE: &str = "kimchi-cli — kimchi from the terminal

USAGE
  kimchi-cli [OPTIONS] <command> [--param value | param=value ...]
  kimchi-cli commands [--json]        list every command (--json: the full parameter schema)
  kimchi-cli help <command>           one command's parameters
  kimchi-cli batch [--continue]       run JSON lines from stdin: {\"command\":\"clip.addText\",\"params\":{...}}
                                     one JSON result per line; stops at the first error unless --continue
  kimchi-cli doctor [--json]          check ffmpeg, the running app, versions, folders and the lsuite entry
  kimchi-cli mcp-config [--json]      how to add kimchi-mcp to Claude Code, Codex, Cursor or Claude Desktop
  kimchi-cli docs [--out PATH]        write the command reference (default docs/COMMANDS.md; - for stdout)
  kimchi-cli render <project.json> <output> [--format mp4|hevc|prores|webm|gif|audio] [--quality draft|standard|high]
                                     = --file <project.json> export.start path=<output> wait=true
  kimchi-cli generate <provider> <model> \"<prompt>\" [--video] [--image PATH] [--end PATH] [--aspect 16:9]
                    [--duration 5] [--seed N] [--count N] [--out DIR]
                                     one generation straight to files, without a project
  kimchi-cli providers | models [<provider>]
                                     = generate.providers | generate.models provider=<provider>

OPTIONS
  --file <project.json>   work on a project file in this process; every change is saved back to it.
                          project.create makes the file. A file open in the running app is refused.
  --live                  require the running app (the default without --file or --headless)
  --headless              work on the library in this process (refused while the app runs)
  --agent                 run as an agent: Settings › Agent › Permissions apply
  --args <json>           parameters as one JSON object, merged with the --param values
  --compact               one line of JSON
  --continue              batch: keep going after a failed line

Parameters are camelCase; times are seconds; ids and unique names both work (\"Title\", \"Video 1\").
Arrays and objects are JSON (a single value is a one-item array); booleans may stand alone (--wait).

EXAMPLES
  kimchi-cli project.overview
  kimchi-cli clip.addText --text \"Hello\" --start 0 --duration 3
  kimchi-cli media.import --paths ~/clips/a.mp4 --paths ~/clips/b.mp4 --place
  kimchi-cli generate.submit --prompt \"slow pan over a misty harbour at dawn\" --video --wait
  kimchi-cli --file cut.json project.create name=Trailer
  kimchi-cli --file cut.json export.start path=cut.mp4

Exit status: 0 on success, 1 when a command fails, 2 on a usage error.";

enum Failure {
    Usage(String),
    Command(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Command(message)
    }
}

type Res = Result<(), Failure>;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("{message}\n\nRun `kimchi-cli --help` for usage.");
            ExitCode::from(2)
        }
        Err(Failure::Command(message)) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[String]) -> Res {
    let inv = kimchi_cli::parse_args(args).map_err(Failure::Usage)?;
    if inv.version {
        say!("kimchi-cli {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let Some(command) = inv.command.clone() else {
        if inv.help {
            say!("{USAGE}");
            return Ok(());
        }
        return Err(Failure::Usage("Missing command".into()));
    };
    if inv.help {
        return match registry::spec(&command) {
            Some(spec) => describe(spec),
            None => {
                say!("{USAGE}");
                Ok(())
            }
        };
    }
    match command.as_str() {
        "commands" => commands(&inv),
        "help" => match inv.rest.first() {
            None => {
                say!("{USAGE}");
                Ok(())
            }
            Some(name) => match registry::spec(name) {
                Some(spec) => describe(spec),
                None => Err(Failure::Usage(kimchi_cli::unknown_command(name))),
            },
        },
        "docs" => docs(&inv),
        "doctor" => doctor(&inv).await,
        "mcp-config" => mcp_config(&inv),
        "batch" => batch(&inv).await,
        "render" => render(&inv).await,
        "generate" => generate::run(&inv.rest).await.map_err(Failure::Command),
        "providers" => {
            let p = serde_json::Map::new();
            run_command(&inv, "generate.providers", p).await
        }
        "models" => {
            let mut p = serde_json::Map::new();
            if let Some(provider) = inv.rest.first() {
                p.insert("provider".into(), json!(provider));
            }
            run_command(&inv, "generate.models", p).await
        }
        _ => run_command(&inv, &command, inv.params.clone()).await,
    }
}

/// Opens the backend the options ask for.
async fn backend(inv: &Invocation) -> Result<Backend, Failure> {
    let source = if inv.agent { Source::Agent } else { Source::Cli };
    if let Some(path) = &inv.file {
        return Ok(Backend::file(path, source).await?);
    }
    if inv.headless {
        return Ok(Backend::headless(source).await?);
    }
    Backend::live(if inv.agent { "agent" } else { "cli" }).await.map_err(|e| {
        Failure::Command(format!("{e} (--headless works on the library without the app.)"))
    })
}

async fn run_command(inv: &Invocation, command: &str, params: serde_json::Map<String, Value>) -> Res {
    let spec = registry::spec(command).ok_or_else(|| Failure::Usage(kimchi_cli::unknown_command(command)))?;
    let mut params = Value::Object(params);
    registry::coerce(spec, &mut params);
    registry::validate(spec, &params).map_err(Failure::Usage)?;
    let backend = backend(inv).await?;
    let result = backend.call(command, params).await?;
    print_json(&result, inv.compact);
    Ok(())
}

fn print_json(v: &Value, compact: bool) {
    let text = if compact { serde_json::to_string(v) } else { serde_json::to_string_pretty(v) };
    say!("{}", text.unwrap_or_default());
}

fn has(inv: &Invocation, flag: &str) -> bool {
    inv.rest.iter().any(|a| a == flag)
}

fn flag_value<'a>(inv: &'a Invocation, flag: &str) -> Result<Option<&'a str>, Failure> {
    match inv.rest.iter().position(|a| a == flag) {
        Some(i) => inv.rest.get(i + 1).map(|v| Some(v.as_str())).ok_or_else(|| Failure::Usage(format!("{flag} needs a value"))),
        None => Ok(None),
    }
}

fn commands(inv: &Invocation) -> Res {
    if has(inv, "--json") {
        print_json(&json!(registry::commands().iter().map(registry::describe).collect::<Vec<_>>()), inv.compact);
        return Ok(());
    }
    let mut family = "";
    for spec in registry::commands() {
        if spec.family() != family {
            family = spec.family();
            say!("\n{}", family.to_uppercase());
        }
        let params: Vec<String> = spec.params.iter().map(|p| if p.required { format!("--{}", p.name) } else { format!("[--{}]", p.name) }).collect();
        say!("  {:<26} {}", spec.name, params.join(" "));
        say!("  {:<26} {}", "", first_sentence(spec.doc));
    }
    Ok(())
}

fn first_sentence(doc: &str) -> &str {
    match doc.find(". ") {
        Some(i) => &doc[..=i],
        None => doc,
    }
}

fn describe(spec: &registry::Spec) -> Res {
    say!("{}\n  {}\n", spec.name, spec.doc);
    let mut notes = vec![if spec.mutates { "Changes the project, files or the app." } else { "Read only." }.to_string()];
    if spec.perm != registry::Perm::Edit {
        notes.push(match spec.perm {
            registry::Perm::PersonOnly => "Person only: refused for agents (--agent, MCP).".into(),
            p => format!("Agents need the \"{}\" permission.", p.label()),
        });
    }
    if spec.needs_window {
        notes.push("Needs the running window (not with --file or --headless).".into());
    }
    say!("  {}\n", notes.join(" "));
    if spec.params.is_empty() {
        say!("  No parameters.");
    }
    for p in spec.params {
        say!("  --{:<16} {:<8} {}{}", p.name, p.kind.schema_type().unwrap_or("any"), if p.required { "" } else { "(optional) " }, p.doc);
    }
    Ok(())
}

fn docs(inv: &Invocation) -> Res {
    let out = flag_value(inv, "--out")?.unwrap_or("docs/COMMANDS.md");
    let md = registry::markdown();
    if out == "-" {
        if let Err(e) = std::io::stdout().lock().write_all(md.as_bytes()) {
            stdout_failed(e);
        }
        return Ok(());
    }
    let path = PathBuf::from(out);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, md).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    eprintln!("wrote {} ({} commands)", path.display(), registry::commands().len());
    Ok(())
}

async fn doctor(inv: &Invocation) -> Res {
    let report = kimchi_cli::doctor().await;
    if has(inv, "--json") {
        print_json(&report, inv.compact);
    } else {
        for check in report["checks"].as_array().into_iter().flatten() {
            say!(
                "{} {:<14} {}",
                if check["ok"] == true { "ok " } else { "!! " },
                check["check"].as_str().unwrap_or(""),
                check["detail"].as_str().unwrap_or("")
            );
        }
    }
    if report["ok"] == true { Ok(()) } else { Err(Failure::Command("Some checks failed".into())) }
}

fn mcp_config(inv: &Invocation) -> Res {
    let c = kimchi_cli::mcp_config();
    if has(inv, "--json") {
        print_json(&c, inv.compact);
        return Ok(());
    }
    let block = serde_json::to_string_pretty(&c["json"]).unwrap_or_default();
    say!("Claude Code:\n  {}\n", c["claudeCode"].as_str().unwrap_or(""));
    say!("Codex CLI:\n  {}\n", c["codex"].as_str().unwrap_or(""));
    say!("Cursor (~/.cursor/mcp.json), Claude Desktop (claude_desktop_config.json) and most other MCP clients:\n{block}\n");
    say!("--live drives the running kimchi window; use --file <project.json> instead to work on a file without it.");
    Ok(())
}

/// JSON lines in, JSON lines out, on one backend for the whole run. Each line runs as it
/// arrives, so another program can hold a conversation over the pipes.
async fn batch(inv: &Invocation) -> Res {
    let mut backend: Option<Backend> = None;
    let mut failed = false;
    for (index, line) in std::io::stdin().lock().lines().enumerate() {
        let line = line.map_err(|e| Failure::Command(format!("Couldn't read stdin: {e}")))?;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let parsed = kimchi_cli::parse_batch_line(&line);
        let result = match parsed {
            Err(e) => Err(e),
            Ok((name, params)) => {
                if backend.is_none() {
                    backend = Some(match self::backend(inv).await {
                        Ok(b) => b,
                        Err(Failure::Usage(e) | Failure::Command(e)) => return Err(Failure::Command(e)),
                    });
                }
                let b = backend.as_ref().expect("opened above");
                b.call(&name, params).await.map(|v| (name, v))
            }
        };
        let reply = match result {
            Ok((name, value)) => json!({ "line": index + 1, "command": name, "ok": true, "result": value }),
            Err(error) => {
                failed = true;
                json!({ "line": index + 1, "ok": false, "error": error })
            }
        };
        let mut out = std::io::stdout().lock();
        if let Err(e) = writeln!(out, "{reply}").and_then(|()| out.flush()) {
            stdout_failed(e);
        }
        if failed && !inv.keep_going {
            return Err(Failure::Command(format!("Batch stopped at line {} (use --continue to keep going)", index + 1)));
        }
    }
    if failed { Err(Failure::Command("Some batch lines failed".into())) } else { Ok(()) }
}

/// `render <project.json> <output>` = `--file <project.json> export.start path=<output> wait=true`.
async fn render(inv: &Invocation) -> Res {
    let mut pos = vec![];
    let mut params = serde_json::Map::new();
    let mut it = inv.rest.iter();
    while let Some(a) = it.next() {
        match a.strip_prefix("--") {
            Some(name @ ("format" | "quality" | "width" | "height" | "fps" | "from" | "to")) => {
                let v = it.next().ok_or_else(|| Failure::Usage(format!("--{name} needs a value")))?;
                params.insert(name.into(), kimchi_cli::coerce("export.start", name, v).map_err(Failure::Usage)?);
            }
            Some(other) => return Err(Failure::Usage(format!("render doesn't take --{other}"))),
            None => pos.push(a.clone()),
        }
    }
    let [project, output] = &pos[..] else { return Err(Failure::Usage("render needs <project.json> <output>".into())) };
    if !std::path::Path::new(project).exists() {
        return Err(Failure::Command(format!("{project} doesn't exist")));
    }
    let output = kimchi_cli::absolute(std::path::Path::new(output))?;
    params.insert("path".into(), json!(output));
    params.insert("wait".into(), json!(true));
    let inv = Invocation { file: Some(PathBuf::from(project)), agent: inv.agent, compact: inv.compact, ..Default::default() };
    let spec = registry::spec("export.start").expect("export.start is a command");
    let params = Value::Object(params);
    registry::validate(spec, &params).map_err(Failure::Usage)?;
    let backend = backend(&inv).await?;
    let status = backend.call("export.start", params).await?;
    if let Some(e) = status["error"].as_str() {
        return Err(Failure::Command(e.to_string()));
    }
    print_json(&status, inv.compact);
    Ok(())
}
