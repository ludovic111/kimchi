//! The look library: kimchi's built-in looks ([`kimchi_core::effects::LOOKS`]) and the ones a
//! person imported from other apps (LUTs, Lightroom / Camera Raw presets, Premiere Lumetri
//! presets: [`kimchi_interop::looks`]) or saved from a clip.
//!
//! Stored in `<data>/looks/`: one `<id>.json` per look, the LUT files it uses copied into
//! `<data>/looks/luts/` so a look keeps working when the original moves. Folders are a field
//! of each look (the folder it was imported from, or the one given).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use kimchi_core::Effects;
use kimchi_core::effects::{EFFECT_PROPS, LOOKS};
use kimchi_interop::Report;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A look in the library.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryLook {
    pub id: String,
    pub name: String,
    /// Where it shows in the picker ("Film", "Lightroom/Travel"); `None` at the top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The look format it came from (`kimchi_interop::looks::LOOK_FORMATS` id), or `saved`.
    pub format: String,
    /// The file it was imported from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Its corrections and LUT (the LUT's path is the library's copy). No chroma key, no plugins.
    pub effects: Effects,
    /// What didn't come over as it was.
    #[serde(default)]
    pub report: Report,
    pub created_at: DateTime<Utc>,
}

impl LibraryLook {
    /// "From a Lightroom / Camera Raw preset", "Saved from a clip".
    pub fn source_label(&self) -> String {
        match kimchi_interop::looks::LOOK_FORMATS.iter().find(|f| f.id == self.format) {
            Some(f) => format!("From a {}", f.label),
            None => "Saved from a clip".to_string(),
        }
    }
}

/// A look by id or name: built-in or from the library.
#[derive(Debug, Clone)]
pub enum Found {
    BuiltIn(&'static kimchi_core::effects::Look),
    Library(LibraryLook),
}

impl Found {
    pub fn name(&self) -> String {
        match self {
            Found::BuiltIn(l) => l.label.to_string(),
            Found::Library(l) => l.name.clone(),
        }
    }

    /// `clip`'s effects with this look on them: built-in looks set the corrections and keep the
    /// clip's LUT (as `clip.setEffects look` always has); library looks set the corrections and
    /// the LUT. The chroma key and plugins stay. `strength` sets the LUT's.
    pub fn apply(&self, clip: &Effects, strength: Option<f64>) -> Result<Effects, String> {
        let mut out = match self {
            Found::BuiltIn(l) => kimchi_core::effects::apply_look(clip, l.id)?,
            Found::Library(l) => Effects { chroma_key: clip.chroma_key.clone(), plugins: clip.plugins.clone(), ..l.effects.clone() },
        };
        if let (Some(s), Some(lut)) = (strength, out.lut.as_mut()) {
            lut.strength = s;
        }
        Ok(out.clamped())
    }
}

pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("looks") }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn luts(&self) -> PathBuf {
        self.dir.join("luts")
    }

    /// Every library look, by folder then name.
    pub fn list(&self) -> Vec<LibraryLook> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return vec![] };
        let mut out: Vec<LibraryLook> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<LibraryLook>(&b).map_err(|e| e.to_string())) {
                Ok(l) => Some(l),
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "skipping an unreadable look");
                    None
                }
            })
            .collect();
        out.sort_by(|a, b| a.folder.cmp(&b.folder).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        out
    }

    /// A look by id or name (case-insensitive): built-in looks first.
    pub fn find(&self, key: &str) -> Result<Found, String> {
        let k = key.trim();
        if let Some(l) = LOOKS.iter().find(|l| l.id.eq_ignore_ascii_case(k) || l.label.eq_ignore_ascii_case(k)) {
            return Ok(Found::BuiltIn(l));
        }
        let all = self.list();
        if let Some(l) = all.iter().find(|l| l.id.eq_ignore_ascii_case(k)).or_else(|| all.iter().find(|l| l.name.eq_ignore_ascii_case(k))) {
            return Ok(Found::Library(l.clone()));
        }
        let mut names: Vec<&str> = LOOKS.iter().map(|l| l.id).collect();
        names.extend(all.iter().map(|l| l.name.as_str()));
        let hint = kimchi_core::closest(k, &names).map(|c| format!(" Did you mean “{c}”?")).unwrap_or_default();
        Err(format!("No look “{k}”.{hint} looks.list shows them all; looks.import adds LUTs and presets from other apps."))
    }

    /// A name no other look has: `name`, else `name 2`, `name 3`…
    fn unique_name(&self, name: &str, taken: &[String]) -> String {
        let base = if name.trim().is_empty() { "Look" } else { name.trim() };
        let used = |n: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(n)) || LOOKS.iter().any(|l| l.label.eq_ignore_ascii_case(n) || l.id.eq_ignore_ascii_case(n));
        if !used(base) {
            return base.to_string();
        }
        (2..).map(|k| format!("{base} {k}")).find(|n| !used(n)).expect("a free name")
    }

    fn unique_id(&self, name: &str) -> String {
        let slug: String = name.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
        let slug = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
        let slug = if slug.is_empty() { "look".to_string() } else { slug.chars().take(48).collect() };
        let free = |id: &str| !self.dir.join(format!("{id}.json")).exists() && !LOOKS.iter().any(|l| l.id == id);
        if free(&slug) {
            return slug;
        }
        (2..).map(|k| format!("{slug}-{k}")).find(|id| free(id)).expect("a free id")
    }

    /// Copies a LUT into the library (once per file content and name) and returns the copy.
    fn keep_lut(&self, path: &Path) -> Result<PathBuf, String> {
        let dir = self.luts();
        if path.starts_with(&dir) {
            return Ok(path.to_path_buf());
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("Can't make {}: {e}", dir.display()))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "lut.cube".into());
        let bytes = std::fs::read(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) => (s.to_string(), format!(".{e}")),
            None => (name.clone(), String::new()),
        };
        for k in 1.. {
            let candidate = dir.join(if k == 1 { name.clone() } else { format!("{stem} {k}{ext}") });
            match std::fs::read(&candidate) {
                Ok(existing) if existing == bytes => return Ok(candidate),
                Ok(_) => continue,
                Err(_) => {
                    std::fs::write(&candidate, &bytes).map_err(|e| format!("Can't copy the LUT into {}: {e}", dir.display()))?;
                    return Ok(candidate);
                }
            }
        }
        unreachable!()
    }

    /// Adds a look: a unique name and id, its LUT copied in.
    pub fn add(&self, name: &str, folder: Option<String>, format: &str, source: Option<&Path>, effects: &Effects, report: Report) -> Result<LibraryLook, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("Can't make {}: {e}", self.dir.display()))?;
        let taken: Vec<String> = self.list().into_iter().map(|l| l.name).collect();
        let name = self.unique_name(name, &taken);
        let id = self.unique_id(&name);
        let mut effects = Effects { chroma_key: None, plugins: vec![], ..effects.clone() }.clamped();
        if let Some(lut) = effects.lut.as_mut() {
            lut.path = self.keep_lut(Path::new(&lut.path))?.to_string_lossy().into_owned();
        }
        let look = LibraryLook {
            id: id.clone(),
            name,
            folder: folder.map(|f| f.trim().trim_matches('/').to_string()).filter(|f| !f.is_empty()),
            format: format.to_string(),
            source: source.map(|p| p.to_string_lossy().into_owned()),
            effects,
            report,
            created_at: Utc::now(),
        };
        let json = serde_json::to_vec_pretty(&look).map_err(|e| e.to_string())?;
        std::fs::write(self.dir.join(format!("{id}.json")), json).map_err(|e| format!("Can't save the look: {e}"))?;
        Ok(look)
    }

    /// Removes a library look (and its LUT copy when no other look uses it).
    pub fn remove(&self, key: &str) -> Result<LibraryLook, String> {
        let look = match self.find(key)? {
            Found::BuiltIn(l) => return Err(format!("“{}” is one of kimchi's built-in looks: they stay.", l.label)),
            Found::Library(l) => l,
        };
        std::fs::remove_file(self.dir.join(format!("{}.json", look.id))).map_err(|e| format!("Can't remove the look: {e}"))?;
        if let Some(lut) = &look.effects.lut {
            let p = PathBuf::from(&lut.path);
            let shared = self.list().iter().any(|l| l.effects.lut.as_ref().is_some_and(|x| x.path == lut.path));
            if p.starts_with(self.luts()) && !shared {
                let _ = std::fs::remove_file(p);
            }
        }
        Ok(look)
    }
}

/// The non-zero corrections of `e`, by name.
pub fn values(e: &Effects) -> Value {
    let mut m = serde_json::Map::new();
    for p in EFFECT_PROPS {
        let v = e.get(p).unwrap_or(0.0);
        if v.abs() > 1e-9 {
            m.insert(p.to_string(), json!((v * 1000.0).round() / 1000.0));
        }
    }
    Value::Object(m)
}

/// How `looks.list` shows a library look.
pub fn describe(l: &LibraryLook) -> Value {
    let format = kimchi_interop::looks::LOOK_FORMATS.iter().find(|f| f.id == l.format);
    let mut v = json!({
        "id": l.id,
        "name": l.name,
        "builtIn": false,
        "folder": l.folder,
        "format": l.format,
        "from": format.map_or("Saved from a clip", |f| f.label),
        "values": values(&l.effects),
    });
    if let Some(lut) = &l.effects.lut {
        v["lut"] = json!({ "file": Path::new(&lut.path).file_name().map(|n| n.to_string_lossy().into_owned()), "path": lut.path, "strength": lut.strength });
    }
    if !l.report.approximated.is_empty() || !l.report.dropped.is_empty() {
        v["approximated"] = json!(l.report.approximated);
        v["dropped"] = json!(l.report.dropped);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_names_ids_and_luts() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path());
        let lut = dir.path().join("Film.cube");
        std::fs::write(&lut, "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n").unwrap();
        let fx = Effects { contrast: 0.2, lut: Some(kimchi_core::Lut { path: lut.to_string_lossy().into(), strength: 1.0 }), chroma_key: Some(Default::default()), ..Default::default() };
        let a = lib.add("Film", Some("Imported".into()), "cube", Some(&lut), &fx, Report::default()).unwrap();
        let b = lib.add("Film", None, "cube", Some(&lut), &fx, Report::default()).unwrap();
        assert_eq!((a.name.as_str(), b.name.as_str()), ("Film", "Film 2"));
        assert_ne!(a.id, b.id);
        assert!(a.effects.chroma_key.is_none(), "keys stay with clips");
        // The same LUT is copied once.
        assert_eq!(a.effects.lut, b.effects.lut);
        assert!(a.effects.lut.as_ref().unwrap().path.starts_with(lib.luts().to_string_lossy().as_ref()));
        // Built-in names aren't taken, and built-ins come first when looking up.
        assert_eq!(lib.add("Warm", None, "saved", None, &Effects::default(), Report::default()).unwrap().name, "Warm 2");
        assert!(matches!(lib.find("warm").unwrap(), Found::BuiltIn(_)));
        assert!(matches!(lib.find("film 2").unwrap(), Found::Library(_)));
        assert!(lib.find("flim").unwrap_err().contains("Film"));
        assert_eq!(lib.list().len(), 3);
        // Applying keeps the clip's key and plugins, replaces the LUT.
        let clip = Effects { saturation: 0.5, chroma_key: Some(Default::default()), ..Default::default() };
        let out = lib.find(&a.id).unwrap().apply(&clip, Some(0.4)).unwrap();
        assert_eq!((out.contrast, out.saturation), (0.2, 0.0));
        assert!(out.chroma_key.is_some() && out.lut.as_ref().unwrap().strength == 0.4);
        // Removing keeps a LUT another look uses.
        lib.remove(&a.id).unwrap();
        assert!(Path::new(&b.effects.lut.as_ref().unwrap().path).exists());
        lib.remove("Film 2").unwrap();
        assert!(!Path::new(&b.effects.lut.as_ref().unwrap().path).exists());
        assert!(lib.remove("punchy").unwrap_err().contains("built-in"));
    }
}
