//! End-to-end job flow through the harness with a fake provider.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kimchi_gen::*;

struct Fake {
    delay: Duration,
}

#[async_trait]
impl Provider for Fake {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "fake".into(),
            name: "Fake".into(),
            kind: ProviderKind::Local,
            tagline: String::new(),
            website: String::new(),
            needs_key: false,
            key_env: vec![],
            key_url: None,
            key_hint: None,
            default_base_url: "http://localhost".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToVideo],
        }
    }
    async fn models(&self, _: &Ctx) -> GenResult<Vec<ModelInfo>> {
        Ok(vec![ModelInfo::new("fake", "m", "Fake model", &[Task::TextToImage])])
    }
    async fn check(&self, _: &Ctx) -> GenResult<String> {
        Ok("ok".into())
    }
    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        cx.report(Progress::fraction(0.5, "halfway"));
        tokio::time::sleep(self.delay).await;
        // A 1×1 PNG.
        let png = util::b64_decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==").unwrap();
        Ok(GenOutput { items: (0..req.count).map(|_| OutputItem::bytes(OutputKind::Image, png.clone(), "image/png")).collect(), seed: Some(42), cost_usd: None })
    }
}

fn harness(delay: Duration) -> Arc<Harness> {
    Arc::new(Harness::with_providers(Arc::new(MemorySecrets::default()), vec![Arc::new(Fake { delay })]))
}

async fn wait_done(rx: &mut tokio::sync::broadcast::Receiver<Job>, id: &str) -> Job {
    loop {
        let j = rx.recv().await.unwrap();
        if j.id == id && j.status.is_done() {
            return j;
        }
    }
}

#[tokio::test]
async fn job_runs_and_saves_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let h = harness(Duration::from_millis(10));
    let mut rx = h.subscribe();
    let mut req = GenRequest::new("m", Task::TextToImage, "a cat");
    req.count = 2;
    let job = h.submit("fake", req, dir.path().to_path_buf(), serde_json::json!({"x": 1})).unwrap();
    let done = wait_done(&mut rx, &job.id).await;
    assert_eq!(done.status, JobStatus::Succeeded);
    assert_eq!(done.outputs.len(), 2);
    assert_eq!(done.seed, Some(42));
    assert_eq!(done.tag["x"], 1);
    for o in &done.outputs {
        assert!(o.path.ends_with(".png"));
        assert!(std::path::Path::new(&o.path).exists());
    }
}

#[tokio::test]
async fn jobs_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let h = harness(Duration::from_secs(30));
    let mut rx = h.subscribe();
    let job = h.submit("fake", GenRequest::new("m", Task::TextToImage, "slow"), dir.path().to_path_buf(), serde_json::Value::Null).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.cancel(&job.id);
    assert_eq!(wait_done(&mut rx, &job.id).await.status, JobStatus::Cancelled);
}

#[tokio::test]
async fn submit_validates_requests() {
    let dir = tempfile::tempdir().unwrap();
    let h = harness(Duration::ZERO);
    let out = dir.path().to_path_buf();
    assert!(h.submit("nope", GenRequest::new("m", Task::TextToImage, "x"), out.clone(), Default::default()).is_err());
    assert!(h.submit("fake", GenRequest::new("m", Task::TextToVideo, "x"), out.clone(), Default::default()).is_err(), "unsupported task");
    assert!(h.submit("fake", GenRequest::new("m", Task::TextToImage, "  "), out.clone(), Default::default()).is_err(), "empty prompt");
    assert!(h.submit("fake", GenRequest::new("m", Task::ImageToVideo, "x"), out, Default::default()).is_err(), "missing image");
}

#[tokio::test]
async fn statuses_reflect_settings() {
    let h = harness(Duration::ZERO);
    assert!(h.statuses()[0].ready);
    h.set_settings("fake", ProviderSettings { enabled: false, ..Default::default() });
    assert!(!h.statuses()[0].ready);
}
