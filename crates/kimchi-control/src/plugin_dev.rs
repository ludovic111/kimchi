//! Making plugins with an agent (lsuite `PLUGINS.md`, "The recipe"): the Rust toolchain on this
//! computer, a crate from the SDK's template in `~/.lsuite/plugins-src/kimchi/<name>/`, files
//! written inside it only, `cargo build --release` with the compiler's errors as data, and the
//! built library made into a bundle that `plugin.install` takes.

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use kimchi_media::render::plugins::{bundle, catalogue};

use crate::session::CmdResult;

/// How long a build may take (a first build fetches and compiles the SDK).
const BUILD_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Errors returned from one build at most (the first ones matter).
const MAX_ERRORS: usize = 20;

/// Where Rust is installed by rustup when it isn't on the `PATH` (the app started from the Dock
/// or Finder gets a bare `PATH`).
fn cargo_home_bin() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".cargo")))?;
    Some(home.join("bin"))
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// A program on the `PATH`, else in rustup's folder.
fn find_tool(name: &str) -> Option<PathBuf> {
    let file = exe(name);
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join(&file)).find(|p| p.is_file());
    on_path.or_else(|| cargo_home_bin().map(|b| b.join(&file)).filter(|p| p.is_file()))
}

/// The `PATH` builds run with: the person's, with rustup's folder in front.
fn build_path() -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = cargo_home_bin().into_iter().collect();
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    if cfg!(target_os = "macos") {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Toolchain {
    pub cargo: Option<PathBuf>,
    pub rustc: Option<PathBuf>,
    /// `rustc --version`'s answer.
    pub version: Option<String>,
    pub ok: bool,
    /// How to install Rust (rustup), for the person to agree to.
    pub install_hint: String,
    pub install_url: &'static str,
}

/// What `plugin.toolchain` answers.
pub async fn toolchain() -> Toolchain {
    let cargo = find_tool("cargo");
    let rustc = find_tool("rustc");
    let version = match &rustc {
        Some(r) => {
            let out = tokio::time::timeout(Duration::from_secs(15), tokio::process::Command::new(r).arg("--version").env("PATH", build_path()).stdin(Stdio::null()).output()).await;
            out.ok().and_then(|o| o.ok()).filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        }
        None => None,
    };
    let install_hint = if cfg!(windows) {
        "Download and run rustup-init.exe from https://rustup.rs (it also asks for the Visual Studio build tools).".to_string()
    } else {
        "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y".to_string()
    };
    Toolchain { ok: cargo.is_some() && version.is_some(), cargo, rustc, version, install_hint, install_url: "https://rustup.rs" }
}

/// A plugin crate's name: lowercase letters, digits and `-`, starting with a letter.
pub fn check_name(name: &str) -> CmdResult<String> {
    let n = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    let ok = n.len() <= 48 && n.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !n.ends_with('-');
    if !ok {
        return Err(format!("`{name}` can't be a plugin's name: use lowercase letters, digits and -, starting with a letter (like halftone-dots)."));
    }
    Ok(n)
}

/// `~/.lsuite/plugins-src/kimchi`.
pub fn sources() -> CmdResult<PathBuf> {
    catalogue::sources_folder().ok_or_else(|| "Plugins aren't set up in this program.".to_string())
}

/// A plugin crate's folder (it may not exist yet).
pub fn crate_dir(name: &str) -> CmdResult<PathBuf> {
    Ok(sources()?.join(check_name(name)?))
}

fn existing_crate(name: &str) -> CmdResult<PathBuf> {
    let dir = crate_dir(name)?;
    if !dir.join("Cargo.toml").is_file() {
        return Err(format!("There is no plugin crate `{name}` in {} (plugin.new makes one).", sources()?.display()));
    }
    Ok(dir)
}

/// `halftone-dots` → `HalftoneDots`.
fn type_name(name: &str) -> String {
    name.split('-').filter(|p| !p.is_empty()).map(|p| {
        let mut c = p.chars();
        c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
    }).collect::<String>()
}

/// `halftone-dots` → `Halftone dots`.
fn display_name(name: &str) -> String {
    let s = name.replace('-', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
}

/// The SDK dependency line of a new crate: a local copy when `KIMCHI_PLUGIN_SDK` names one (or
/// this kimchi was built from a checkout that is still there), else the repository at this
/// kimchi's tag.
pub fn sdk_dependency() -> (String, &'static str) {
    let local = std::env::var_os("KIMCHI_PLUGIN_SDK").map(PathBuf::from).filter(|p| p.join("Cargo.toml").is_file()).or_else(|| {
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../kimchi-plugin");
        dev.join("Cargo.toml").is_file().then(|| dev.canonicalize().unwrap_or(dev))
    });
    match local {
        Some(p) => (format!("kimchi-plugin = {{ path = {} }}", toml_string(&p.to_string_lossy())), "local"),
        None => (format!("kimchi-plugin = {{ git = \"https://github.com/ludovic111/kimchi\", tag = \"v{}\" }}", env!("CARGO_PKG_VERSION")), "git"),
    }
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn cargo_toml(name: &str) -> String {
    let (dep, _) = sdk_dependency();
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
# The kimchi plugin SDK, from kimchi's repository at the tag of the kimchi this was made in
# (any tag or branch works: tag = "v0.11.0", or branch = "main"). Set KIMCHI_PLUGIN_SDK to a local
# kimchi-plugin folder before plugin.new to build against that copy instead.
{dep}

[profile.release]
# Never "abort": a panic must reach the SDK's guard so kimchi can switch the plugin off.
panic = "unwind"

# A plugin crate stands on its own, outside any workspace.
[workspace]
"#
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCrate {
    pub name: String,
    pub path: PathBuf,
    pub id: String,
    pub kind: String,
    pub files: Vec<String>,
    /// `local` (a copy of the SDK on this computer) or `git`.
    pub sdk: &'static str,
}

/// `plugin.new`: a crate from the SDK's template. Refuses to overwrite one that exists.
pub fn new_crate(name: &str, kind: &str, id: Option<&str>, description: Option<&str>) -> CmdResult<NewCrate> {
    let name = check_name(name)?;
    let kind = kind.trim().to_ascii_lowercase();
    let (code, ty, template_id, template_name) =
        kimchi_plugin::template::of(&kind).ok_or_else(|| format!("kind is effect, generator or transition, not `{kind}`."))?;
    let id = match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(i) if bundle::valid_id(i) => i.to_string(),
        Some(i) => return Err(format!("`{i}` isn't a reverse-DNS id (like com.example.{name}).")),
        None => format!("local.plugins.{name}"),
    };
    let dir = crate_dir(&name)?;
    if dir.join("Cargo.toml").exists() {
        return Err(format!("{} exists already: write into it (plugin.writeSource), or choose another name.", dir.display()));
    }
    std::fs::create_dir_all(dir.join("src")).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let display = display_name(&name);
    let mut lib = code.replace(ty, &type_name(&name)).replace(template_id, &id).replace(template_name, &display);
    if let Some(d) = description.map(str::trim).filter(|d| !d.is_empty()) {
        // The template's one-line description.
        if let Some(start) = lib.find(".describe(\"") {
            let from = start + ".describe(\"".len();
            if let Some(len) = lib[from..].find("\")") {
                lib.replace_range(from..from + len, &d.replace('"', "'"));
            }
        }
    }
    lib.push_str(&format!("\nkimchi_plugin::export_plugins!({});\n", type_name(&name)));
    let manifest = bundle::Manifest {
        id: id.clone(),
        name: display.clone(),
        version: "0.1.0".into(),
        app: bundle::APP.into(),
        kind: kind.clone(),
        abi: kimchi_plugin::ABI_VERSION,
        description: description.unwrap_or("").trim().to_string(),
        authors: vec![],
        library: bundle::Libraries::for_crate(&name),
    };
    let write = |rel: &str, text: &str| std::fs::write(dir.join(rel), text).map_err(|e| format!("Couldn't write {rel}: {e}"));
    write("Cargo.toml", &cargo_toml(&name))?;
    write("src/lib.rs", &lib)?;
    manifest.write(&dir)?;
    write(".gitignore", "target/\nbundle/\n")?;
    Ok(NewCrate { name, path: dir, id, kind, files: vec!["Cargo.toml".into(), "plugin.toml".into(), "src/lib.rs".into()], sdk: sdk_dependency().1 })
}

/// `plugin.writeSource`: one file inside the crate. Paths are relative to the crate; leaving it,
/// absolute paths and the build's own folders are refused.
pub fn write_source(name: &str, path: &str, contents: &str) -> CmdResult<Value> {
    let dir = existing_crate(name)?;
    let rel = Path::new(path.trim());
    let safe = !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
    if !safe {
        return Err(format!("`{path}` must be a path inside the crate, like src/lib.rs (no .., no absolute path)."));
    }
    let first = rel.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
    if matches!(first.as_str(), "target" | "bundle" | ".git") {
        return Err(format!("{first}/ belongs to the build: write sources only."));
    }
    if contents.len() > 2 * 1024 * 1024 {
        return Err("That file is over 2 MB: a plugin's source should be much smaller.".into());
    }
    let full = dir.join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
    }
    std::fs::write(&full, contents).map_err(|e| format!("Couldn't write {}: {e}", full.display()))?;
    Ok(json!({ "name": name, "path": rel, "bytes": contents.len(), "file": full }))
}

/// One problem the compiler found.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub level: String,
    /// Relative to the crate.
    pub file: String,
    pub line: u64,
    pub column: u64,
    pub message: String,
    /// The compiler's own rendering, with the code and the arrows.
    pub rendered: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Build {
    pub ok: bool,
    pub name: String,
    pub errors: Vec<Diagnostic>,
    pub warnings: usize,
    /// The built library.
    pub library: Option<PathBuf>,
    pub seconds: f64,
    /// When the build failed before compiling (a dependency, the network): cargo's own words.
    pub cargo_error: Option<String>,
}

static TARGET_OVERRIDE: parking_lot::RwLock<Option<PathBuf>> = parking_lot::RwLock::new(None);

/// For tests: builds share this target folder (so the SDK compiles once across runs).
#[doc(hidden)]
pub fn set_target_dir_for_tests(dir: PathBuf) {
    *TARGET_OVERRIDE.write() = Some(dir);
}

/// The folder builds share (the SDK is compiled once for every plugin).
fn target_dir() -> CmdResult<PathBuf> {
    if let Some(d) = TARGET_OVERRIDE.read().clone() {
        return Ok(d);
    }
    Ok(sources()?.join(".target"))
}

/// `plugin.build`: `cargo build --release`, with the compiler's errors as data.
pub async fn build(name: &str) -> CmdResult<Build> {
    let dir = existing_crate(name)?;
    let cargo = find_tool("cargo").ok_or("Rust isn't installed (no cargo): plugin.toolchain says how to install it.")?;
    let started = std::time::Instant::now();
    let mut child = tokio::process::Command::new(&cargo)
        .args(["build", "--release", "--message-format=json-diagnostic-rendered-ansi"])
        .current_dir(&dir)
        .env("PATH", build_path())
        .env("CARGO_TARGET_DIR", target_dir()?)
        .env("CARGO_TERM_COLOR", "never")
        .env_remove("RUSTFLAGS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Couldn't start cargo: {e}"))?;
    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });
    let crate_name = check_name(name)?.replace('-', "_");
    let mut errors = vec![];
    let mut warnings = 0usize;
    let mut library = None;
    let mut lines = BufReader::new(stdout).lines();
    let read = async {
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            match v["reason"].as_str() {
                Some("compiler-message") => {
                    let m = &v["message"];
                    let level = m["level"].as_str().unwrap_or("");
                    if level == "warning" {
                        warnings += 1;
                        continue;
                    }
                    if !level.starts_with("error") || errors.len() >= MAX_ERRORS {
                        continue;
                    }
                    let span = m["spans"].as_array().and_then(|s| s.iter().find(|x| x["is_primary"] == true).or(s.first()));
                    errors.push(Diagnostic {
                        level: level.to_string(),
                        file: span.and_then(|s| s["file_name"].as_str()).unwrap_or("").to_string(),
                        line: span.and_then(|s| s["line_start"].as_u64()).unwrap_or(0),
                        column: span.and_then(|s| s["column_start"].as_u64()).unwrap_or(0),
                        message: m["message"].as_str().unwrap_or("").to_string(),
                        rendered: strip_ansi(m["rendered"].as_str().unwrap_or("")),
                    });
                }
                Some("compiler-artifact") if v["target"]["name"].as_str() == Some(crate_name.as_str()) => {
                    library = v["filenames"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).find(|p| p.extension().is_some_and(|e| e == kimchi_media::render::plugins::native::library_extension()));
                }
                _ => {}
            }
        }
    };
    if tokio::time::timeout(BUILD_TIMEOUT, read).await.is_err() {
        let _ = child.kill().await;
        return Err(format!("The build took more than {} minutes and was stopped.", BUILD_TIMEOUT.as_secs() / 60));
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let stderr = err_task.await.unwrap_or_default();
    // Errors about the whole crate ("aborting due to…") say nothing more.
    errors.retain(|d| !d.message.starts_with("aborting due to"));
    let ok = status.success() && errors.is_empty();
    let cargo_error = (!ok && errors.is_empty()).then(|| {
        let lines: Vec<&str> = stderr.lines().filter(|l| l.trim_start().starts_with("error") || l.trim_start().starts_with("Caused by") || l.starts_with("  ")).collect();
        let text = if lines.is_empty() { stderr.trim().to_string() } else { lines.join("\n") };
        strip_ansi(&text).chars().take(4000).collect()
    });
    Ok(Build { ok, name: name.to_string(), errors, warnings, library: library.filter(|_| ok), seconds: (started.elapsed().as_secs_f64() * 10.0).round() / 10.0, cargo_error })
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ … letter
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The built library and `plugin.toml` as a bundle folder in the crate (`bundle/`), ready for
/// `plugin.install`.
pub fn make_bundle(name: &str, library: &Path) -> CmdResult<PathBuf> {
    let dir = existing_crate(name)?;
    let mut manifest = bundle::Manifest::read(&dir)?;
    // The library's file name on this system is what the build made.
    let file = library.file_name().ok_or("The build made no library file.")?.to_string_lossy().to_string();
    match bundle::os_key() {
        "macos" => manifest.library.macos = Some(file.clone()),
        "windows" => manifest.library.windows = Some(file.clone()),
        _ => manifest.library.linux = Some(file.clone()),
    }
    let out = dir.join("bundle");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| format!("Couldn't create {}: {e}", out.display()))?;
    std::fs::copy(library, out.join(&file)).map_err(|e| format!("Couldn't copy the library: {e}"))?;
    manifest.write(&out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths_are_kept_inside_the_crate() {
        assert_eq!(check_name("Halftone Dots").unwrap(), "halftone-dots");
        assert!(check_name("../x").is_err() && check_name("9lives").is_err() && check_name("").is_err());
        assert_eq!(type_name("halftone-dots"), "HalftoneDots");
        assert_eq!(display_name("halftone-dots"), "Halftone dots");
        assert_eq!(strip_ansi("\u{1b}[1m\u{1b}[38;5;9merror\u{1b}[0m: x"), "error: x");
    }
}
