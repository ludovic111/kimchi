//! Motion clips rendered ahead, in the background (`motion.render`): progress, cancelling, and
//! the clip switching to its rendered frames when they are done (one undo step, so undoing it
//! goes back to drawing the clip live).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use kimchi_core::{ClipPatch, Edit, Id};
use serde::{Deserialize, Serialize};

use crate::session::{CmdResult, Event, Session, Source, err};

/// Progress of one render.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RenderStatus {
    pub id: String,
    pub clip_id: Id,
    pub clip_name: String,
    pub progress: f64,
    pub done: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// `standard` or `path`.
    pub engine: String,
    pub started_at: DateTime<Utc>,
}

pub(crate) struct RenderEntry {
    status: RenderStatus,
    cancel: Arc<AtomicBool>,
}

impl Session {
    pub fn renders(&self) -> Vec<RenderStatus> {
        self.renders.lock().iter().map(|e| e.status.clone()).collect()
    }

    fn set_render(&self, status: RenderStatus) {
        if let Some(e) = self.renders.lock().iter_mut().find(|e| e.status.id == status.id) {
            e.status = status.clone();
        }
        self.emit(Event::Render { render: status });
    }

    /// Stops a running render; false when there is none with that id.
    pub fn cancel_render(&self, id: &str) -> bool {
        match self.renders.lock().iter().find(|e| e.status.id == id && !e.status.done) {
            Some(e) => {
                e.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }
}

/// Starts rendering a motion clip; when it is done the clip plays the frames (an undo step by
/// `source`). A render already running for the clip is replaced.
pub fn start(s: &Arc<Session>, clip_id: Id, source: Source) -> CmdResult<String> {
    let tools = s.tools()?;
    let project = s.project()?;
    let clip = project.clip(clip_id).cloned().ok_or("clip not found")?;
    let kimchi_core::ClipContent::Motion { scene, .. } = &clip.content else {
        return Err(format!("\"{}\" isn't a motion clip; only motion clips are rendered ahead.", clip.name));
    };
    let engine = match scene {
        kimchi_core::Scene::Space(sp) => sp.render.engine.clone(),
        kimchi_core::Scene::Flat(_) => "standard".into(),
    };
    for e in s.renders.lock().iter().filter(|e| e.status.clip_id == clip_id && !e.status.done) {
        e.cancel.store(true, Ordering::Relaxed);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    let status = RenderStatus { id: id.clone(), clip_id, clip_name: clip.name.clone(), progress: 0.0, done: false, error: None, engine, started_at: Utc::now() };
    {
        let mut list = s.renders.lock();
        list.retain(|e| !e.status.done || e.status.started_at > Utc::now() - chrono::Duration::hours(1));
        list.push(RenderEntry { status: status.clone(), cancel: cancel.clone() });
    }
    s.set_render(status.clone());
    let dir = s.cache_dir(project.id).join("rendered");
    let s2 = s.clone();
    s.runtime().spawn(async move {
        let progress_session = s2.clone();
        let base = status.clone();
        let progress = move |p: f64| progress_session.set_render(RenderStatus { progress: p, ..base.clone() });
        let result = kimchi_media::render::cache::render(&tools, &project, clip_id, &dir, &progress, &cancel).await.map_err(err);
        let end = match result {
            Ok(rendered) => {
                let patch = ClipPatch { rendered: Some(Some(rendered)), ..Default::default() };
                match s2.apply("motion.render", source, &Edit::UpdateClip { clip_id, patch }, None) {
                    Ok(_) => RenderStatus { progress: 1.0, done: true, ..status },
                    Err(e) => RenderStatus { done: true, error: Some(format!("rendered, but the clip couldn't take it: {e}")), ..status },
                }
            }
            Err(e) => RenderStatus { done: true, error: Some(e), ..status },
        };
        if let Some(e) = &end.error {
            tracing::warn!(clip = %end.clip_name, "render failed: {e}");
        }
        s2.set_render(end);
    });
    Ok(id)
}

/// Waits for a render to end; an error if it failed or was cancelled.
pub async fn wait(s: &Arc<Session>, id: &str) -> CmdResult<RenderStatus> {
    let mut rx = s.subscribe();
    loop {
        let st = s.renders().into_iter().find(|e| e.id == id).ok_or_else(|| format!("No render `{id}`."))?;
        if st.done {
            return match &st.error {
                Some(e) => Err(format!("Render of \"{}\" failed: {e}", st.clip_name)),
                None => Ok(st),
            };
        }
        if let Err(tokio::sync::broadcast::error::RecvError::Closed) = rx.recv().await {
            return Err("kimchi is shutting down".into());
        }
    }
}
