//! Durable conversations, grouped by the document's stable project id.
use super::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationInfo {
    pub id: Id,
    pub title: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SavedThread {
    pub info: ConversationInfo,
    pub conversation: Conversation,
    pub entries: Vec<Entry>,
    pub runs: Vec<RunInfo>,
    pub base: usize,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct ProjectThreads {
    pub current: Option<Id>,
    pub threads: Vec<SavedThread>,
    #[serde(default)]
    pub memory: String,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Archive {
    pub projects: HashMap<String, ProjectThreads>,
    pub next_run: u64,
}

impl State {
    pub(super) fn project_key(&self) -> String {
        self.project.map(|id| id.to_string()).unwrap_or_else(|| "welcome".into())
    }

    pub(super) fn save_current(&mut self) {
        let key = self.project_key();
        let info = self.thread.clone();
        let saved = SavedThread { info, conversation: self.conversation.clone(), entries: self.entries.clone(), runs: self.runs.clone(), base: self.base };
        let project = self.archive.projects.entry(key).or_default();
        project.current = Some(saved.info.id);
        project.memory = self.memory.clone();
        if let Some(existing) = project.threads.iter_mut().find(|t| t.info.id == saved.info.id) {
            *existing = saved;
        } else {
            project.threads.push(saved);
        }
        self.archive.next_run = self.next_run;
    }

    pub(super) fn load_project(&mut self) {
        let project = self.archive.projects.get(&self.project_key()).cloned().unwrap_or_default();
        self.memory = project.memory;
        match project.threads.into_iter().find(|t| Some(t.info.id) == project.current) {
            Some(thread) => self.load_thread(thread),
            None => self.fresh_thread(),
        }
    }

    pub(super) fn fresh_thread(&mut self) {
        self.clear_thread();
        self.runs.clear();
        self.reverts.clear();
        self.unreverts.clear();
        self.thread = ConversationInfo::default();
        self.base = 0;
    }

    pub(super) fn load_thread(&mut self, saved: SavedThread) {
        self.thread = saved.info;
        self.conversation = saved.conversation;
        self.entries = saved.entries;
        self.runs = saved.runs;
        self.base = saved.base;
        self.seen.clear();
        self.reverts.clear();
        self.unreverts.clear();
        // Command sequence numbers and undo checkpoints are session-local. Never
        // allow a restored conversation to revert an unrelated edit after restart.
    }

    pub(super) fn conversations(&self) -> Vec<ConversationInfo> {
        let mut list: Vec<_> = self.archive.projects.get(&self.project_key()).into_iter().flat_map(|p| &p.threads)
            .filter(|t| t.info.id != self.thread.id).map(|t| t.info.clone()).collect();
        list.push(self.thread.clone());
        list.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
        list
    }
}

impl Default for ConversationInfo {
    fn default() -> Self {
        Self { id: Id::new_v4(), title: "New conversation".into(), updated_at: Utc::now() }
    }
}

pub(super) fn read(path: &Path) -> CmdResult<Archive> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Archive::default()),
        Err(e) => return Err(format!("Could not read conversation history at {}: {e}. Saving is paused to protect the existing file; restore it and restart Kimchi.", path.display())),
    };
    let mut archive: Archive = match serde_json::from_slice(&bytes) {
        Ok(archive) => archive,
        Err(e) => {
            // Preserve the original for recovery instead of overwriting a damaged file.
            let backup = path.with_extension(format!("unreadable-{}.json", Utc::now().timestamp_millis()));
            if let Err(error) = std::fs::copy(path, &backup) { tracing::error!(%error, "Could not back up conversation history"); }
            tracing::error!(%e, ?backup, "Could not decode agent conversations");
            return Err(format!("Conversation history at {} could not be decoded: {e}. Saving is paused to protect it; restore the file and restart Kimchi.", path.display()));
        }
    };
    for project in archive.projects.values_mut() {
        for thread in &mut project.threads {
            for run in &mut thread.runs {
                run.checkpoint = None;
                if run.state == RunState::Running {
                    run.state = RunState::Cancelled;
                    run.activity = None;
                    run.finished_at = Some(Utc::now());
                }
            }
        }
    }
    Ok(archive)
}

pub(super) fn write(path: &PathBuf, archive: &Archive) -> CmdResult<()> {
    let parent = path.parent().ok_or("No conversation storage directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, archive).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| format!("Could not save agent conversations: {e}"))?;
    Ok(())
}
