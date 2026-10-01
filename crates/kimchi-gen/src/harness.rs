//! The harness owns every provider, their settings and keys, and the queue of
//! running jobs. The editor only ever talks to this.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

/// Where API keys live. The desktop app backs this with the OS keychain.
pub trait SecretStore: Send + Sync {
    fn get(&self, provider: &str) -> Option<String>;
    fn set(&self, provider: &str, key: &str) -> Result<(), String>;
    fn delete(&self, provider: &str) -> Result<(), String>;
}

/// In-memory secrets, for tests and headless use.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, provider: &str) -> Option<String> {
        self.0.lock().get(provider).cloned()
    }
    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        self.0.lock().insert(provider.into(), key.into());
        Ok(())
    }
    fn delete(&self, provider: &str) -> Result<(), String> {
        self.0.lock().remove(provider);
        Ok(())
    }
}

/// User settings for one provider (persisted by the app; keys are not in here).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ProviderSettings {
    /// Hidden from model pickers when false.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Overrides [`ProviderInfo::default_base_url`].
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    #[ts(type = "Record<string, unknown>")]
    pub options: Map<String, Value>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum KeySource {
    None,
    Keychain,
    Env,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ProviderStatus {
    pub info: ProviderInfo,
    pub settings: ProviderSettings,
    pub key_source: KeySource,
    /// Last 4 characters of the key, for display.
    pub key_preview: Option<String>,
    /// Usable right now: enabled, and has a key if it needs one.
    pub ready: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_done(self) -> bool {
        matches!(self, JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled)
    }
}

/// A job as the UI sees it. Every change is broadcast as a full snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Job {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub model_name: String,
    pub request: GenRequest,
    pub status: JobStatus,
    pub progress: Progress,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub elapsed_ms: u64,
    pub outputs: Vec<SavedOutput>,
    pub error: Option<String>,
    pub seed: Option<i64>,
    pub cost_usd: Option<f64>,
    /// Free-form tag the editor uses to remember what to do with the result
    /// (e.g. which placeholder clip to replace).
    #[ts(type = "unknown")]
    pub tag: Value,
}

struct JobEntry {
    job: Job,
    cancel: CancellationToken,
}

const MODEL_CACHE_TTL: Duration = Duration::from_secs(60 * 30);
/// Jobs that may run at once per provider. Cloud APIs queue on their side
/// anyway; local GPUs really want one at a time.
const CLOUD_CONCURRENCY: usize = 6;
const LOCAL_CONCURRENCY: usize = 1;

pub struct Harness {
    http: reqwest::Client,
    providers: Vec<Arc<dyn Provider>>,
    settings: RwLock<HashMap<String, ProviderSettings>>,
    secrets: Arc<dyn SecretStore>,
    model_cache: Mutex<HashMap<String, (Instant, Vec<ModelInfo>)>>,
    jobs: Mutex<Vec<JobEntry>>,
    limits: HashMap<String, Arc<Semaphore>>,
    events: broadcast::Sender<Job>,
}

impl Harness {
    /// A harness with every built-in provider.
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self::with_providers(secrets, crate::providers::all())
    }

    pub fn with_providers(secrets: Arc<dyn SecretStore>, providers: Vec<Arc<dyn Provider>>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("kimchi/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60 * 15))
            .build()
            .expect("http client");
        let limits = providers
            .iter()
            .map(|p| {
                let info = p.info();
                let n = if info.kind == ProviderKind::Local { LOCAL_CONCURRENCY } else { CLOUD_CONCURRENCY };
                (info.id, Arc::new(Semaphore::new(n)))
            })
            .collect();
        Self {
            http,
            providers,
            settings: RwLock::new(HashMap::new()),
            secrets,
            model_cache: Mutex::new(HashMap::new()),
            jobs: Mutex::new(vec![]),
            limits,
            events: broadcast::channel(256).0,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Job> {
        self.events.subscribe()
    }

    fn provider(&self, id: &str) -> GenResult<Arc<dyn Provider>> {
        self.providers
            .iter()
            .find(|p| p.info().id == id)
            .cloned()
            .ok_or_else(|| GenError::Provider(format!("unknown provider `{id}`")))
    }

    // ---- settings & keys -------------------------------------------------

    pub fn load_settings(&self, all: HashMap<String, ProviderSettings>) {
        *self.settings.write() = all;
        self.model_cache.lock().clear();
    }

    pub fn settings(&self) -> HashMap<String, ProviderSettings> {
        self.settings.read().clone()
    }

    pub fn set_settings(&self, provider: &str, s: ProviderSettings) {
        self.settings.write().insert(provider.to_string(), s);
        self.model_cache.lock().remove(provider);
    }

    pub fn set_key(&self, provider: &str, key: Option<&str>) -> GenResult<()> {
        self.provider(provider)?;
        let r = match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => self.secrets.set(provider, k),
            None => self.secrets.delete(provider),
        };
        self.model_cache.lock().remove(provider);
        r.map_err(GenError::Provider)
    }

    fn resolve_key(&self, info: &ProviderInfo) -> (Option<String>, KeySource) {
        if let Some(k) = self.secrets.get(&info.id).filter(|k| !k.is_empty()) {
            return (Some(k), KeySource::Keychain);
        }
        for var in &info.key_env {
            if let Ok(k) = std::env::var(var)
                && !k.trim().is_empty()
            {
                return (Some(k.trim().to_string()), KeySource::Env);
            }
        }
        (None, KeySource::None)
    }

    pub fn statuses(&self) -> Vec<ProviderStatus> {
        let settings = self.settings.read();
        self.providers
            .iter()
            .map(|p| {
                let info = p.info();
                let s = settings.get(&info.id).cloned().unwrap_or(ProviderSettings { enabled: true, ..Default::default() });
                let (key, key_source) = self.resolve_key(&info);
                let key_preview = key.as_ref().map(|k| {
                    let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                    format!("…{tail}")
                });
                let ready = s.enabled && (!info.needs_key || key.is_some());
                ProviderStatus { info, settings: s, key_source, key_preview, ready }
            })
            .collect()
    }

    fn ctx(&self, provider: &dyn Provider) -> Ctx {
        let info = provider.info();
        let s = self.settings.read().get(&info.id).cloned().unwrap_or_default();
        let base = s.base_url.filter(|u| !u.trim().is_empty()).unwrap_or(info.default_base_url.clone());
        let (key, _) = self.resolve_key(&info);
        Ctx::new(info.id, self.http.clone(), base).with_key(key).with_options(s.options)
    }

    // ---- models ----------------------------------------------------------

    pub async fn check(&self, provider: &str) -> GenResult<String> {
        let p = self.provider(provider)?;
        let cx = self.ctx(p.as_ref());
        if p.info().needs_key {
            cx.key()?;
        }
        p.check(&cx).await
    }

    pub async fn models(&self, provider: &str, refresh: bool) -> GenResult<Vec<ModelInfo>> {
        if !refresh
            && let Some((at, models)) = self.model_cache.lock().get(provider)
            && at.elapsed() < MODEL_CACHE_TTL
        {
            return Ok(models.clone());
        }
        let p = self.provider(provider)?;
        let cx = self.ctx(p.as_ref());
        let models = p.models(&cx).await?;
        self.model_cache.lock().insert(provider.to_string(), (Instant::now(), models.clone()));
        Ok(models)
    }

    /// Models from every ready provider. Providers that fail are skipped.
    pub async fn all_models(&self) -> Vec<ModelInfo> {
        let ready: Vec<String> = self.statuses().into_iter().filter(|s| s.ready).map(|s| s.info.id).collect();
        let lists = futures::future::join_all(ready.iter().map(|id| self.models(id, false))).await;
        lists.into_iter().flatten().flatten().collect()
    }

    // ---- jobs ------------------------------------------------------------

    pub fn jobs(&self) -> Vec<Job> {
        self.jobs.lock().iter().map(|e| e.job.clone()).collect()
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        self.jobs.lock().iter().find(|e| e.job.id == id).map(|e| e.job.clone())
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) {
        let snapshot = {
            let mut jobs = self.jobs.lock();
            let Some(e) = jobs.iter_mut().find(|e| e.job.id == id) else { return };
            f(&mut e.job);
            e.job.clone()
        };
        let _ = self.events.send(snapshot);
    }

    pub fn cancel(&self, id: &str) {
        if let Some(e) = self.jobs.lock().iter().find(|e| e.job.id == id) {
            e.cancel.cancel();
        }
    }

    /// Removes finished jobs from the list.
    pub fn clear_finished(&self) {
        self.jobs.lock().retain(|e| !e.job.status.is_done());
    }

    /// Queues a generation. Outputs are written into `out_dir`. Returns
    /// immediately; follow progress with [`Harness::subscribe`].
    pub fn submit(self: &Arc<Self>, provider: &str, mut req: GenRequest, out_dir: PathBuf, tag: Value) -> GenResult<Job> {
        let p = self.provider(provider)?;
        let info = p.info();
        if !info.tasks.contains(&req.task) {
            return Err(GenError::Unsupported(format!("{} doesn't do {}", info.name, req.task.as_str())));
        }
        if req.prompt.trim().is_empty() && !req.task.needs_image() {
            return Err(GenError::Provider("Write a prompt first.".into()));
        }
        if req.task.needs_image() && req.images.is_empty() {
            return Err(GenError::Provider("This task needs an input image.".into()));
        }
        let cx = self.ctx(p.as_ref());
        if info.needs_key {
            cx.key()?;
        }
        load_images(&mut req)?;
        req.count = req.count.clamp(1, 8);

        let model_name = self
            .model_cache
            .lock()
            .get(provider)
            .and_then(|(_, ms)| ms.iter().find(|m| m.id == req.model).map(|m| m.name.clone()))
            .unwrap_or_else(|| req.model.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let mut stored = req.clone();
        for img in &mut stored.images {
            img.data = Bytes::new();
        }
        let job = Job {
            id: id.clone(),
            provider: provider.to_string(),
            model: req.model.clone(),
            model_name,
            request: stored,
            status: JobStatus::Queued,
            progress: Progress::message("Queued"),
            created_at: Utc::now(),
            finished_at: None,
            elapsed_ms: 0,
            outputs: vec![],
            error: None,
            seed: req.seed,
            cost_usd: None,
            tag,
        };
        let cancel = CancellationToken::new();
        self.jobs.lock().push(JobEntry { job: job.clone(), cancel: cancel.clone() });
        let _ = self.events.send(job.clone());

        let this = Arc::clone(self);
        let limit = self.limits.get(provider).cloned().unwrap_or_else(|| Arc::new(Semaphore::new(1)));
        tokio::spawn(async move {
            let job_id = id.clone();
            let progress_target = Arc::clone(&this);
            let progress_id = id.clone();
            let cx = cx.with_cancel(cancel.clone()).with_progress(move |pr| {
                progress_target.update(&progress_id, |j| j.progress = pr);
            });

            let run = async {
                let _permit = tokio::select! {
                    p = limit.acquire_owned() => p.map_err(|_| GenError::Cancelled)?,
                    _ = cancel.cancelled() => return Err(GenError::Cancelled),
                };
                this.update(&job_id, |j| {
                    j.status = JobStatus::Running;
                    j.progress = Progress::message("Starting");
                });
                let started = Instant::now();
                let out = tokio::select! {
                    r = p.generate(&cx, &req) => r?,
                    _ = cancel.cancelled() => return Err(GenError::Cancelled),
                };
                if out.items.is_empty() {
                    return Err(GenError::Provider("The model returned nothing. Try another prompt or model.".into()));
                }
                cx.report(Progress::fraction(0.98, "Saving"));
                let saved = tokio::select! {
                    r = save_outputs(&cx, &out, &out_dir, &job_id) => r?,
                    _ = cancel.cancelled() => return Err(GenError::Cancelled),
                };
                Ok((out, saved, started.elapsed()))
            };

            let created = Instant::now();
            let result = run.await;
            this.update(&job_id, |j| {
                j.finished_at = Some(Utc::now());
                j.elapsed_ms = created.elapsed().as_millis() as u64;
                match result {
                    Ok((out, saved, _)) => {
                        j.status = JobStatus::Succeeded;
                        j.progress = Progress::fraction(1.0, "Done");
                        j.outputs = saved;
                        j.seed = out.seed.or(j.seed);
                        j.cost_usd = out.cost_usd;
                    }
                    Err(GenError::Cancelled) => {
                        j.status = JobStatus::Cancelled;
                        j.progress = Progress::message("Cancelled");
                    }
                    Err(e) => {
                        tracing::warn!(job = %j.id, provider = %j.provider, "generation failed: {e}");
                        j.status = JobStatus::Failed;
                        j.progress = Progress::message("Failed");
                        j.error = Some(e.to_string());
                    }
                }
            });
        });
        Ok(job)
    }
}

/// Reads input images from disk and normalises their MIME types.
fn load_images(req: &mut GenRequest) -> GenResult<()> {
    for img in &mut req.images {
        if img.data.is_empty() {
            if img.path.is_empty() {
                return Err(GenError::Provider("input image has no data".into()));
            }
            img.data = Bytes::from(std::fs::read(&img.path)?);
        }
        if img.mime.is_empty() || !img.mime.starts_with("image/") {
            img.mime = util::sniff_mime(&img.data).unwrap_or("image/png").to_string();
        }
    }
    Ok(())
}

async fn save_outputs(cx: &Ctx, out: &GenOutput, dir: &Path, job_id: &str) -> GenResult<Vec<SavedOutput>> {
    tokio::fs::create_dir_all(dir).await?;
    let short = &job_id[..8.min(job_id.len())];
    let mut saved = Vec::with_capacity(out.items.len());
    for (i, item) in out.items.iter().enumerate() {
        let (data, mime) = match &item.source {
            OutputSource::Bytes { data, mime } => (data.clone(), mime.clone()),
            OutputSource::Url { url, headers } => util::download(cx, url, headers).await?,
        };
        let mime = if mime.contains('/') && mime != "application/octet-stream" {
            mime
        } else {
            util::sniff_mime(&data).unwrap_or("application/octet-stream").to_string()
        };
        let kind = util::kind_for_mime(&mime).unwrap_or(item.kind);
        let path = dir.join(format!("{short}-{}.{}", i + 1, util::extension_for(&mime)));
        tokio::fs::write(&path, &data).await?;
        saved.push(SavedOutput { path: path.to_string_lossy().into_owned(), kind, mime });
    }
    Ok(saved)
}
