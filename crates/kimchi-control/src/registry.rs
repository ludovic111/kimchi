//! The public command registry.
//!
//! Every action a person can take in the kimchi window is a named command here
//! (`family.verb`, JSON parameters in, JSON result out). The window, the
//! built-in agent, `kimchi-cli` and `kimchi-mcp` are peers: they all go through
//! [`call`], so validation, permissions and the undo history behave the same
//! whoever made the change. CLI help, the MCP tool list, the agent's tools and
//! `docs/COMMANDS.md` are generated from [`COMMANDS`], never written by hand.
//!
//! Conventions: times are seconds on the timeline; parameters are camelCase;
//! results use the project file's field names; ids and unique names are both
//! accepted wherever an id is expected (`clipId`, `trackId`, `assetId`,
//! `markerId`, `projectId`).

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::session::{CmdResult, CommandRecord, Event, Session, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    String,
    Number,
    Integer,
    Boolean,
    Array,
    Object,
    /// Any JSON value.
    Any,
}

impl Kind {
    pub fn schema_type(self) -> Option<&'static str> {
        Some(match self {
            Kind::String => "string",
            Kind::Number => "number",
            Kind::Integer => "integer",
            Kind::Boolean => "boolean",
            Kind::Array => "array",
            Kind::Object => "object",
            Kind::Any => return None,
        })
    }

    fn accepts(self, v: &Value) -> bool {
        match self {
            Kind::String => v.is_string(),
            Kind::Number => v.is_number(),
            Kind::Integer => v.as_i64().is_some() || v.as_u64().is_some() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
            Kind::Boolean => v.is_boolean(),
            Kind::Array => v.is_array(),
            Kind::Object => v.is_object(),
            Kind::Any => true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
    pub doc: &'static str,
}

pub const fn req(name: &'static str, kind: Kind, doc: &'static str) -> Param {
    Param { name, kind, required: true, doc }
}

pub const fn opt(name: &'static str, kind: Kind, doc: &'static str) -> Param {
    Param { name, kind, required: false, doc }
}

/// What an agent needs to be allowed to run a command (`settings.agent.permissions`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perm {
    /// Reading, and editing the open project (always undoable).
    Edit,
    Files,
    Projects,
    Generate,
    Settings,
    AppControl,
    /// Never from an agent: API keys and the agent's own permissions stay with the person.
    PersonOnly,
}

impl Perm {
    pub fn label(self) -> &'static str {
        match self {
            Perm::Edit => "always allowed",
            Perm::Files => "files",
            Perm::Projects => "projects",
            Perm::Generate => "generate",
            Perm::Settings => "settings",
            Perm::AppControl => "app control",
            Perm::PersonOnly => "person only",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: &'static [Param],
    /// False for queries; true when the command can change the project, files or the app.
    pub mutates: bool,
    pub perm: Perm,
    /// Only the running app can do it (playback, selection, panels).
    pub needs_window: bool,
}

impl Spec {
    pub fn family(&self) -> &'static str {
        self.name.split('.').next().unwrap_or(self.name)
    }

    /// MCP tool name: the dot becomes an underscore.
    pub fn tool_name(&self) -> String {
        self.name.replace('.', "_")
    }

    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.name == name)
    }
}

pub const fn query(name: &'static str, doc: &'static str, params: &'static [Param]) -> Spec {
    Spec { name, doc, params, mutates: false, perm: Perm::Edit, needs_window: false }
}

pub const fn edit(name: &'static str, doc: &'static str, params: &'static [Param]) -> Spec {
    Spec { name, doc, params, mutates: true, perm: Perm::Edit, needs_window: false }
}

impl Spec {
    pub const fn perm(mut self, perm: Perm) -> Self {
        self.perm = perm;
        self
    }

    pub const fn window(mut self) -> Self {
        self.needs_window = true;
        self
    }
}

/// Accepted by every command that edits the project: edits sharing a key within
/// about a second fold into one undo step (sliders, drags).
pub const COALESCE: Param = opt("coalesce", Kind::String, "Edits with the same key within ~1 s fold into one undo step (drags, sliders).");

/// Every public command, in the order the docs list them.
pub fn commands() -> &'static [Spec] {
    crate::commands::SPECS
}

pub fn spec(name: &str) -> Option<&'static Spec> {
    commands().iter().find(|s| s.name == name)
}

/// Runs a command. This is the only door into kimchi: the window, the agent,
/// the CLI and MCP all come through here.
pub async fn call(session: &Arc<Session>, source: Source, name: &str, params: Value) -> CmdResult {
    let spec = match spec(name) {
        Some(s) => s,
        None => return Err(unknown_command(name)),
    };
    let result = run_checked(session, source, spec, params.clone()).await;
    if source != Source::Window || spec.mutates {
        let record = CommandRecord {
            seq: session.next_seq(),
            source,
            command: spec.name.to_string(),
            params,
            ok: result.is_ok(),
            error: result.as_ref().err().cloned(),
            mutates: spec.mutates,
            at: chrono::Utc::now(),
        };
        session.emit(Event::Command { record });
    }
    result
}

/// Boxed so `project.batch` can call commands recursively.
pub(crate) fn call_boxed<'a>(
    session: &'a Arc<Session>,
    source: Source,
    spec: &'static Spec,
    params: Value,
) -> futures::future::BoxFuture<'a, CmdResult> {
    Box::pin(run_checked(session, source, spec, params))
}

async fn run_checked(session: &Arc<Session>, source: Source, spec: &'static Spec, params: Value) -> CmdResult {
    let params = match params {
        Value::Null => Value::Object(Map::new()),
        Value::Object(_) => params,
        other => return Err(format!("`{}` takes an object of parameters, not {other}", spec.name)),
    };
    allowed(session, source, spec)?;
    validate(spec, &params)?;
    if spec.needs_window && !session.has_ui() {
        return Err(format!(
            "`{}` needs the kimchi window. Start the app and use kimchi-cli without --file, or kimchi-mcp --live.",
            spec.name
        ));
    }
    crate::commands::dispatch(session, &Ctx { source, spec }, Args(params.as_object().cloned().unwrap_or_default())).await
}

/// Checks `settings.agent.permissions` for agent and MCP requests.
pub fn allowed(session: &Session, source: Source, spec: &Spec) -> CmdResult<()> {
    if !source.is_agent() {
        return Ok(());
    }
    let p = session.settings().agent.permissions;
    if !p.enabled {
        return Err("Agents are turned off in Settings › Agent › Permissions.".into());
    }
    let ok = match spec.perm {
        crate::registry::Perm::Edit => true,
        Perm::Files => p.files,
        Perm::Projects => p.projects,
        Perm::Generate => p.generate,
        Perm::Settings => p.settings,
        Perm::AppControl => p.app_control,
        Perm::PersonOnly => false,
    };
    if ok {
        Ok(())
    } else if spec.perm == Perm::PersonOnly {
        Err(format!("`{}` stays with the person: agents can't run it.", spec.name))
    } else {
        Err(format!(
            "`{}` needs the \"{}\" permission, which is off. The person can turn it on in Settings › Agent › Permissions.",
            spec.name,
            spec.perm.label()
        ))
    }
}

/// Checks parameter names and types against the spec.
pub fn validate(spec: &Spec, params: &Value) -> CmdResult<()> {
    let map = params.as_object().ok_or("parameters must be an object")?;
    for (k, v) in map {
        if v.is_null() {
            continue;
        }
        let p = match spec.param(k) {
            Some(p) => p,
            None if k == COALESCE.name && spec.mutates => &COALESCE,
            None => {
                let names: Vec<&str> = spec.params.iter().map(|p| p.name).collect();
                let hint = closest(k, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
                let list = if names.is_empty() { "none".to_string() } else { names.join(", ") };
                return Err(format!("`{}` has no parameter `{k}`.{hint} Parameters: {list}.", spec.name));
            }
        };
        if !p.kind.accepts(v) {
            let want = p.kind.schema_type().unwrap_or("a value");
            return Err(format!("`{k}` should be {} {want} ({}), got {v}", article(want), p.doc));
        }
    }
    for p in spec.params.iter().filter(|p| p.required) {
        if map.get(p.name).is_none_or(Value::is_null) {
            return Err(format!("`{}` needs `{}`: {}", spec.name, p.name, p.doc));
        }
    }
    Ok(())
}

fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
}

fn unknown_command(name: &str) -> String {
    let names: Vec<&str> = commands().iter().map(|s| s.name).collect();
    match closest(name, &names) {
        Some(c) => format!("Unknown command `{name}`. Did you mean `{c}`? (`app.commands` lists them all.)"),
        None => format!("Unknown command `{name}`. `app.commands` lists them all."),
    }
}

/// The closest candidate by edit distance, if it is close enough to be a typo.
pub fn closest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let w = word.to_lowercase();
    candidates
        .iter()
        .map(|c| (levenshtein(&w, &c.to_lowercase()), *c))
        .filter(|(d, c)| *d <= (c.len().max(3) / 3).max(2))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let cur = row[j + 1];
            row[j + 1] = if ca == b[j] { prev } else { 1 + prev.min(row[j]).min(row[j + 1]) };
            prev = cur;
        }
    }
    row[b.len()]
}

/// The calling context handed to every command.
pub struct Ctx {
    pub source: Source,
    pub spec: &'static Spec,
}

impl Ctx {
    pub fn label(&self) -> &'static str {
        self.spec.name
    }
}

/// A command's parameters, already validated against its spec.
#[derive(Debug, Clone, Default)]
pub struct Args(pub Map<String, Value>);

impl Args {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name).filter(|v| !v.is_null())
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn str(&self, name: &str) -> CmdResult<&str> {
        self.opt_str(name).ok_or_else(|| format!("`{name}` is required"))
    }

    pub fn opt_str(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(Value::as_str)
    }

    pub fn f64(&self, name: &str) -> CmdResult<f64> {
        self.opt_f64(name).ok_or_else(|| format!("`{name}` is required"))
    }

    pub fn opt_f64(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(Value::as_f64)
    }

    pub fn opt_i64(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
    }

    pub fn opt_u32(&self, name: &str) -> Option<u32> {
        self.opt_i64(name).map(|v| v.clamp(0, u32::MAX as i64) as u32)
    }

    pub fn opt_bool(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(Value::as_bool)
    }

    pub fn bool_or(&self, name: &str, default: bool) -> bool {
        self.opt_bool(name).unwrap_or(default)
    }

    pub fn array(&self, name: &str) -> Option<&Vec<Value>> {
        self.get(name).and_then(Value::as_array)
    }

    /// A list of strings given as an array, or a single string.
    pub fn strings(&self, name: &str) -> Vec<String> {
        match self.get(name) {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            _ => vec![],
        }
    }

    pub fn coalesce(&self) -> Option<&str> {
        self.opt_str(COALESCE.name)
    }

    pub fn object(&self, name: &str) -> Option<&Map<String, Value>> {
        self.get(name).and_then(Value::as_object)
    }
}

// ---- introspection --------------------------------------------------------

/// JSON Schema of a command's parameters (MCP `inputSchema`).
pub fn input_schema(spec: &Spec) -> Value {
    let mut props = Map::new();
    for p in spec.params {
        let mut s = Map::new();
        if let Some(t) = p.kind.schema_type() {
            s.insert("type".into(), json!(t));
        }
        s.insert("description".into(), json!(p.doc));
        props.insert(p.name.into(), Value::Object(s));
    }
    let required: Vec<&str> = spec.params.iter().filter(|p| p.required).map(|p| p.name).collect();
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// One command described as JSON (`app.commands`, `kimchi-cli commands --json`).
pub fn describe(spec: &Spec) -> Value {
    json!({
        "name": spec.name,
        "description": spec.doc,
        "mutates": spec.mutates,
        "permission": spec.perm.label(),
        "needsWindow": spec.needs_window,
        "params": spec.params.iter().map(|p| json!({
            "name": p.name,
            "type": p.kind.schema_type().unwrap_or("any"),
            "required": p.required,
            "description": p.doc,
        })).collect::<Vec<_>>(),
    })
}

/// `docs/COMMANDS.md`, generated.
pub fn markdown() -> String {
    let mut out = String::from(
        "# kimchi commands\n\n\
         Generated from the command registry (`crates/kimchi-control`) by `kimchi-cli docs`. Do not edit by hand.\n\n\
         Every command works the same from the window, the built-in agent, `kimchi-cli` and `kimchi-mcp` \
         (where `family.verb` becomes the tool `family_verb`). Times are seconds on the timeline; ids and unique \
         names are accepted wherever an id is expected. Commands that edit the project also accept `coalesce` \
         (edits with the same key within about a second fold into one undo step). See [AI_CONTROL.md](AI_CONTROL.md).\n",
    );
    let mut family = "";
    for s in commands() {
        if s.family() != family {
            family = s.family();
            out.push_str(&format!("\n## {family}\n"));
        }
        let mut tags = vec![if s.mutates { "changes things" } else { "read only" }];
        if s.perm != Perm::Edit {
            tags.push(match s.perm {
                Perm::PersonOnly => "person only",
                Perm::Files => "permission: files",
                Perm::Projects => "permission: projects",
                Perm::Generate => "permission: generate",
                Perm::Settings => "permission: settings",
                Perm::AppControl => "permission: app control",
                Perm::Edit => unreachable!(),
            });
        }
        if s.needs_window {
            tags.push("needs the window");
        }
        out.push_str(&format!("\n### `{}`\n\n{} _({})_\n", s.name, s.doc, tags.join(" · ")));
        if !s.params.is_empty() {
            out.push_str("\n| Parameter | Type | | Description |\n| --- | --- | --- | --- |\n");
            for p in s.params {
                out.push_str(&format!(
                    "| `{}` | {} | {} | {} |\n",
                    p.name,
                    p.kind.schema_type().unwrap_or("any"),
                    if p.required { "required" } else { "" },
                    p.doc.replace('|', "\\|")
                ));
            }
        }
    }
    out
}
