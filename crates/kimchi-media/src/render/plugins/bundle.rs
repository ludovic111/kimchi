//! lsuite plugin bundles (lsuite `PLUGINS.md`): a folder with `plugin.toml` and the library.
//!
//! ```toml
//! id = "com.example.halftone"      # reverse-DNS, the bundle's folder name once installed
//! name = "Halftone"
//! version = "0.1.0"
//! app = "kimchi"
//! kind = "effect"                  # effect, generator or transition (what the library mostly is)
//! abi = 1                          # kimchi_plugin::ABI_VERSION
//! description = "Dots like print."
//! authors = ["Ada"]
//!
//! [library]
//! macos = "libhalftone.dylib"
//! linux = "libhalftone.so"
//! windows = "halftone.dll"
//! ```
//!
//! Installed bundles live in `~/.lsuite/plugins/kimchi/<id>/` (`LSUITE_HOME` replaces
//! `~/.lsuite`); the sources an agent writes, in `~/.lsuite/plugins-src/kimchi/<name>/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "plugin.toml";
pub const APP: &str = "kimchi";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    pub app: String,
    #[serde(default = "effect")]
    pub kind: String,
    pub abi: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub library: Libraries,
}

fn effect() -> String {
    "effect".into()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Libraries {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macos: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
}

impl Libraries {
    /// This system's library file name.
    pub fn here(&self) -> Option<&str> {
        if cfg!(target_os = "macos") {
            self.macos.as_deref()
        } else if cfg!(windows) {
            self.windows.as_deref()
        } else {
            self.linux.as_deref()
        }
    }

    /// The names a crate called `lib_name` (its `[lib] name`, `-` as `_`) builds to.
    pub fn for_crate(lib_name: &str) -> Self {
        let n = lib_name.replace('-', "_");
        Self { macos: Some(format!("lib{n}.dylib")), linux: Some(format!("lib{n}.so")), windows: Some(format!("{n}.dll")) }
    }
}

/// An id is reverse-DNS: letters, digits, dots, `-` and `_`, with a dot, not starting or ending
/// with one (it is also a folder name).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.contains('.') && !id.starts_with('.') && !id.ends_with('.') && !id.contains("..") && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

impl Manifest {
    /// Reads and checks `<dir>/plugin.toml`.
    pub fn read(dir: &Path) -> Result<Self, String> {
        let path = dir.join(FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
        let m: Manifest = toml::from_str(&text).map_err(|e| format!("{} isn't a valid plugin.toml: {}", path.display(), e.message()))?;
        m.check()?;
        Ok(m)
    }

    pub fn check(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!("The bundle id `{}` should be reverse-DNS like com.example.halftone (letters, digits, dots, - and _).", self.id));
        }
        if self.app != APP {
            return Err(format!("This bundle is for {}, not kimchi.", self.app));
        }
        if self.abi != kimchi_plugin::ABI_VERSION {
            return Err(format!("This bundle is for kimchi plugin ABI {}; this kimchi runs ABI {}. Rebuild it with the matching kimchi-plugin.", self.abi, kimchi_plugin::ABI_VERSION));
        }
        if !matches!(self.kind.as_str(), "effect" | "generator" | "transition") {
            return Err(format!("kind is effect, generator or transition, not `{}`.", self.kind));
        }
        if self.name.trim().is_empty() {
            return Err("The bundle has no name.".into());
        }
        Ok(())
    }

    /// The library for this system inside `dir`.
    pub fn library(&self, dir: &Path) -> Result<PathBuf, String> {
        let name = self.library.here().ok_or_else(|| format!("{} has no library for this system ([library] {}).", self.name, os_key()))?;
        if name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(format!("The library's name `{name}` should be a file name in the bundle."));
        }
        let path = dir.join(name);
        if !path.is_file() {
            return Err(format!("{} is missing from the bundle.", path.display()));
        }
        Ok(path)
    }

    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let body = format!("# A kimchi plugin bundle: this file and the library, in one folder (lsuite PLUGINS.md).\n{text}");
        std::fs::write(dir.join(FILE), body).map_err(|e| format!("Couldn't write {}: {e}", dir.join(FILE).display()))
    }
}

/// `macos`, `linux` or `windows`.
pub fn os_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// Bundle folders in `dir` (a folder with `plugin.toml`), and in its subfolders two levels down.
pub fn find(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    walk(dir, 0, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(FILE).is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    if depth >= 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
            walk(&p, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manifest {
            id: "com.example.dots".into(),
            name: "Dots".into(),
            version: "0.1.0".into(),
            app: "kimchi".into(),
            kind: "effect".into(),
            abi: kimchi_plugin::ABI_VERSION,
            description: "Dots.".into(),
            authors: vec!["Ada".into()],
            library: Libraries::for_crate("dots"),
        };
        m.write(dir.path()).unwrap();
        assert_eq!(Manifest::read(dir.path()).unwrap(), m);
        assert!(m.library(dir.path()).unwrap_err().contains("missing"));
        std::fs::write(dir.path().join(m.library.here().unwrap()), b"x").unwrap();
        assert!(m.library(dir.path()).is_ok());
        assert_eq!(find(dir.path()), vec![dir.path().to_path_buf()]);
        for (bad, why) in [
            (Manifest { app: "ryolune".into(), ..m.clone() }, "for ryolune"),
            (Manifest { abi: 99, ..m.clone() }, "ABI 99"),
            (Manifest { id: "dots".into(), ..m.clone() }, "reverse-DNS"),
            (Manifest { id: "com/../x".into(), ..m.clone() }, "reverse-DNS"),
            (Manifest { kind: "audio".into(), ..m.clone() }, "effect, generator or transition"),
        ] {
            assert!(bad.check().unwrap_err().contains(why), "{why}");
        }
    }
}
