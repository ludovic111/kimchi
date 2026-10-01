//! On-disk layout of a kimchi library:
//!
//! ```text
//! <root>/projects/<id>/project.json   the document
//! <root>/projects/<id>/generated/     media made by AI models
//! <root>/projects/<id>/cache/         thumbnails, filmstrips, waveforms, proxies
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{Id, Project};

const FILE: &str = "project.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ProjectSummary {
    pub id: Id,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    /// Poster of the first visual asset, if any.
    pub cover: Option<String>,
    pub generated_count: usize,
}

#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project_dir(&self, id: Id) -> PathBuf {
        self.root.join("projects").join(id.to_string())
    }

    pub fn generated_dir(&self, id: Id) -> PathBuf {
        self.project_dir(id).join("generated")
    }

    pub fn cache_dir(&self, id: Id) -> PathBuf {
        self.project_dir(id).join("cache")
    }

    pub fn save(&self, project: &Project) -> io::Result<()> {
        let dir = self.project_dir(project.id);
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_vec_pretty(project).map_err(io::Error::other)?;
        // Write-then-rename so a crash never leaves a half-written project.
        let tmp = dir.join(format!("{FILE}.tmp"));
        fs::write(&tmp, json)?;
        fs::rename(tmp, dir.join(FILE))
    }

    pub fn load(&self, id: Id) -> io::Result<Project> {
        let bytes = fs::read(self.project_dir(id).join(FILE))?;
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }

    pub fn delete(&self, id: Id) -> io::Result<()> {
        fs::remove_dir_all(self.project_dir(id))
    }

    pub fn list(&self) -> Vec<ProjectSummary> {
        let Ok(entries) = fs::read_dir(self.root.join("projects")) else { return vec![] };
        let mut out: Vec<ProjectSummary> = entries
            .flatten()
            .filter_map(|e| {
                let bytes = fs::read(e.path().join(FILE)).ok()?;
                let p: Project = serde_json::from_slice(&bytes).ok()?;
                Some(summarize(&p))
            })
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }
}

pub fn summarize(p: &Project) -> ProjectSummary {
    let cover = p
        .clips()
        .filter_map(|(_, c)| c.asset_id())
        .chain(p.assets.iter().map(|a| a.id))
        .filter_map(|id| p.asset(id))
        .find_map(|a| a.thumbnail.clone());
    ProjectSummary {
        id: p.id,
        name: p.name.clone(),
        updated_at: p.updated_at,
        duration: p.duration(),
        width: p.settings.width,
        height: p.settings.height,
        cover,
        generated_count: p.assets.iter().filter(|a| a.is_generated()).count(),
    }
}
