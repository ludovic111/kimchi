//! `kimchi-cli ask`: the built-in agent on a project file (or the library), without the window.
//!
//! The same agent as the Agent panel (the harness brief, the tools, the live context, the look
//! and the one-step revert), run in this process: API providers call the registry directly; Claude
//! Code, Codex and Gemini CLI get `kimchi-mcp --live` pointed at a private bridge on this session.
//! The evals (`evals/run.py`) run jobs through it.

use std::sync::Arc;

use kimchi_agent::{Agent, AgentConfig, AgentEvent, Conversation, ProviderKind};
use kimchi_cli::{Backend, Invocation};
use kimchi_control::Source;

pub const USAGE: &str = "kimchi-cli --file <project.json> ask \"<request>\" [--provider <id>] [--model <id>] [--max-steps N] [--json]
  Runs kimchi's built-in agent on the project (the provider and model from Settings › Agent unless given).
  --json prints every event as one JSON line (text, commands, usage, the outcome).";

pub async fn run(inv: &Invocation) -> Result<(), String> {
    let mut prompt: Vec<&str> = vec![];
    let (mut provider, mut model, mut max_steps, mut as_json) = (None, None, None, false);
    let mut args = inv.rest.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--provider" => provider = Some(args.next().ok_or("--provider needs a value")?.clone()),
            "--model" => model = Some(args.next().ok_or("--model needs a value")?.clone()),
            "--max-steps" => max_steps = Some(args.next().ok_or("--max-steps needs a value")?.parse::<usize>().map_err(|_| "--max-steps takes a number")?),
            "--json" => as_json = true,
            _ if a.starts_with("--") => return Err(format!("Unknown option `{a}`.\n{USAGE}")),
            _ => prompt.push(a),
        }
    }
    let prompt = prompt.join(" ");
    if prompt.trim().is_empty() {
        return Err(format!("Say what to do.\n{USAGE}"));
    }
    let backend = match &inv.file {
        Some(path) => Backend::file(path, Source::Cli).await?,
        None if inv.headless => Backend::headless(Source::Cli).await?,
        None => return Err("ask runs the agent in this process, on --file <project.json> or --headless. For the running app use agent.send.".into()),
    };
    let Backend::Local { session, .. } = &backend else { unreachable!("a local backend") };
    let session: Arc<kimchi_control::Session> = session.clone();

    let mut config = AgentConfig::from_settings(&session.settings().agent);
    if let Some(p) = provider {
        config.provider = ProviderKind::parse(&p).ok_or_else(|| {
            let ids: Vec<&str> = ProviderKind::ALL.iter().map(|k| k.id()).collect();
            format!("Unknown provider `{p}`. Providers: {}.", ids.join(", "))
        })?;
        if model.is_none() {
            config.model.clear();
        }
    }
    if let Some(m) = model {
        config.model = m;
    }
    if let Some(n) = max_steps {
        config.max_steps = n.max(1);
    }
    // The CLIs reach this session through a bridge of its own (never the app's control file).
    let _bridge = if config.provider.is_cli() {
        let dir = std::env::temp_dir().join(format!("kimchi-ask-{}", std::process::id()));
        Some(kimchi_control::bridge::Server::start_at(session.clone(), dir.join("control.json")).await.map_err(|e| format!("Couldn't start the bridge for {}: {e}", config.provider.label()))?)
    } else {
        None
    };
    let label = config.provider.label();
    let mut run = Agent::start(&session, config, prompt, Conversation::new());
    let mut outcome: Result<(), String> = Err("The run ended without an outcome.".into());
    while let Some(event) = run.next_event().await {
        if as_json {
            println!("{}", serde_json::to_string(&event).unwrap_or_default());
        } else {
            show(&event);
        }
        match event {
            AgentEvent::Done { .. } => outcome = Ok(()),
            AgentEvent::Error { message, .. } => outcome = Err(format!("{label}: {message}")),
            AgentEvent::Cancelled { .. } => outcome = Err("The run was stopped.".into()),
            _ => {}
        }
    }
    if let Some(path) = backend.path() {
        eprintln!("(saved {})", path.display());
    }
    outcome
}

/// A person reading the terminal: the reply on stdout, the commands on stderr.
fn show(event: &AgentEvent) {
    use std::io::Write;
    match event {
        AgentEvent::Text { delta } => {
            print!("{delta}");
            let _ = std::io::stdout().flush();
        }
        AgentEvent::Command { record, .. } => {
            eprintln!("\n· {} {}{}", record.command, if record.ok { "ok" } else { "failed" }, record.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default());
        }
        AgentEvent::Done { changes, .. } => println!("\n\n({changes} change{})", if *changes == 1 { "" } else { "s" }),
        AgentEvent::Error { message, .. } => eprintln!("\nerror: {message}"),
        _ => {}
    }
}
