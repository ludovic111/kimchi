//! The Generate tab: write a prompt, pick a model, set what the model offers,
//! and send it with `generate.submit`. The result lands on the timeline (at the
//! playhead, or the spot an AI action chose) or only in the library. The AI
//! actions elsewhere prefill it through `StoreEvent::Compose`.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, ClickEvent, Context, Entity, FontWeight, PathPromptOptions, Pixels, Render, ScrollHandle, Subscription, Window, div, img,
    prelude::*, px,
};
use kimchi_core::{ClipContent, MediaKind, TrackKind};
use kimchi_gen::{ModelInfo, ProviderKind, Task};
use serde_json::{Value, json};

use crate::playback::Playback;
use crate::store::{ComposeRef, Dialog, Store, StoreEvent, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, icon, segmented, switch, timecode};
use crate::views::generate::params::ParamField;
use crate::views::generate::{COMMON_RATIOS, Draft, chip, chip_base, eyebrow, model_key, ratio_parts, role_label};
use crate::views::timeline::dnd::MediaDrag;

const IMAGE_PLACEHOLDER: &str = "An overhead shot of a ceramic bowl of kimchi on linen, soft morning light…";
const VIDEO_PLACEHOLDER: &str = "A slow dolly through a neon-lit market at night, rain on the lens…";
const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "webp", "heic", "avif", "gif"];

pub struct GeneratePanel {
    pub(crate) store: Entity<Store>,
    /// The project the draft's timeline target belongs to.
    project_id: Option<kimchi_core::Id>,
    pub(crate) draft: Draft,
    pub(crate) prompt: Entity<TextInput>,
    pub(crate) negative: Entity<TextInput>,
    pub(crate) seed: Entity<TextInput>,
    pub(crate) picker_open: bool,
    pub(crate) picker_query: Entity<TextInput>,
    pub(crate) picker_anchor: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Model count the search placeholder was written for.
    pub(crate) picker_total: Option<usize>,
    /// The model the parameter fields were built for.
    pub(crate) params_for: Option<String>,
    pub(crate) param_fields: Vec<ParamField>,
    pub(crate) show_advanced: bool,
    submitting: bool,
    /// Ready provider ids the model list was loaded for.
    ready: Option<Vec<String>>,
    playhead: Entity<PlayheadTime>,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl GeneratePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let prompt = cx.new(|cx| {
            let mut i = TextInput::new(cx).multiline(4).placeholder(IMAGE_PLACEHOLDER);
            i.bare = true;
            i
        });
        let negative = cx.new(|cx| TextInput::new(cx).placeholder("blurry, text, watermark"));
        let seed = cx.new(|cx| {
            let mut i = TextInput::new(cx).placeholder("random");
            i.mono = true;
            i
        });
        let picker_query = cx.new(|cx| {
            let mut i = TextInput::new(cx).placeholder("Search models");
            i.bare = true;
            i
        });
        let playback = store.read(cx).playback.clone();
        let playhead = cx.new(|cx| PlayheadTime::new(playback, cx));

        let mut subs = vec![
            cx.observe(&store, |this, _, cx| {
                this.on_store_changed(cx);
                cx.notify();
            }),
            cx.subscribe_in(&store, window, |this, _, e: &StoreEvent, window, cx| match e {
                StoreEvent::Compose(req) => {
                    this.apply_compose((**req).clone(), cx);
                    crate::ui::input::focus(&this.prompt, window, cx);
                }
                StoreEvent::FocusPrompt => crate::ui::input::focus(&this.prompt, window, cx),
                StoreEvent::EditText | StoreEvent::AskRemoveAsset(_) | StoreEvent::OpenStudio(_) | StoreEvent::SetupClosed => {}
            }),
            cx.subscribe(&prompt, |this, _, e: &InputEvent, cx| match e {
                InputEvent::Submit => this.submit(cx),
                InputEvent::Changed(_) => cx.notify(),
                _ => {}
            }),
            cx.subscribe(&picker_query, |this, _, e: &InputEvent, cx| match e {
                InputEvent::Cancel => {
                    this.picker_open = false;
                    cx.notify();
                }
                InputEvent::Submit => {
                    // Enter picks the first match.
                    if let Some(m) = this.picker_groups(cx).into_iter().next().and_then(|(_, list)| list.into_iter().next()) {
                        this.pick_model(&m, cx);
                    }
                }
                _ => cx.notify(),
            }),
        ];
        for input in [&negative, &seed] {
            subs.push(cx.subscribe(input, |_, _, e: &InputEvent, cx| {
                if let InputEvent::Changed(_) = e {
                    cx.notify();
                }
            }));
        }

        let mut this = Self {
            store: store.clone(),
            draft: Draft::default(),
            prompt,
            negative,
            seed,
            picker_open: false,
            picker_query,
            picker_anchor: Rc::new(Cell::new(None)),
            picker_total: None,
            params_for: None,
            param_fields: vec![],
            show_advanced: false,
            submitting: false,
            ready: None,
            project_id: None,
            playhead,
            scroll: ScrollHandle::new(),
            _subs: subs,
        };
        this.on_store_changed(cx);
        this
    }

    // ---- derived ---------------------------------------------------------

    /// Models that can do what the draft asks for (image or video, with or without input images).
    pub(crate) fn available(&self, cx: &App) -> Vec<ModelInfo> {
        let task = self.draft.task();
        self.store.read(cx).models.iter().filter(|m| m.supports(task)).cloned().collect()
    }

    /// The model the composer will use: the person's choice, else the default
    /// in settings, else the first featured model, else the first one.
    pub(crate) fn current_model(&self, cx: &App) -> Option<ModelInfo> {
        let s = self.store.read(cx);
        let task = self.draft.task();
        let default = match self.draft.audio_task {
            Some(Task::TextToSpeech) => &s.settings.generate.speech_model,
            Some(_) => &s.settings.generate.audio_model,
            None if self.draft.video => &s.settings.generate.video_model,
            None => &s.settings.generate.image_model,
        };
        let wanted = self.draft.model.clone().or_else(|| Some(default.clone()).filter(|d| !d.is_empty()));
        let fits = || s.models.iter().filter(|m| m.supports(task));
        wanted
            .and_then(|k| fits().find(|m| model_key(m) == k))
            .or_else(|| fits().find(|m| m.featured))
            .or_else(|| fits().next())
            .cloned()
    }

    pub(crate) fn provider_name(&self, id: &str, cx: &App) -> String {
        self.store.read(cx).providers.iter().find(|p| p.info.id == id).map(|p| p.info.name.clone()).unwrap_or_else(|| id.to_string())
    }

    pub(crate) fn provider_is_local(&self, id: &str, cx: &App) -> bool {
        self.store.read(cx).providers.iter().any(|p| p.info.id == id && p.info.kind == ProviderKind::Local)
    }

    /// Providers usable now: cloud ones with a key, local servers that answered with models.
    fn connected(&self, cx: &App) -> usize {
        let s = self.store.read(cx);
        s.providers.iter().filter(|p| p.ready && p.info.tasks.contains(&self.draft.task())
            && (p.info.kind == ProviderKind::Cloud || s.models.iter().any(|m| m.provider == p.info.id && m.supports(self.draft.task())))).count()
    }

    fn project_ratio(&self, cx: &App) -> String {
        self.store.read(cx).project.as_ref().map(|p| p.settings.aspect_ratio()).unwrap_or_else(|| "16:9".into())
    }

    fn can_submit(&self, cx: &App) -> bool {
        !self.submitting && self.current_model(cx).is_some() && (!self.prompt.read(cx).text().trim().is_empty() || !self.draft.refs.is_empty())
    }

    // ---- state changes ---------------------------------------------------

    fn on_store_changed(&mut self, cx: &mut Context<Self>) {
        let project_id = self.store.read(cx).project.as_ref().map(|p| p.id);
        if project_id != self.project_id {
            // "Lands after <clip>" means nothing in another project.
            self.project_id = project_id;
            self.draft.target = None;
        }
        // Load (again) when the set of ready providers changes: a key added, a local server enabled.
        let ready: Vec<String> = self.store.read(cx).providers.iter().filter(|p| p.ready).map(|p| p.info.id.clone()).collect();
        // (While a load runs, wait for it: the next notification compares again.)
        if self.ready.as_ref() != Some(&ready) && !self.store.read(cx).models_loading {
            self.ready = Some(ready);
            self.store.update(cx, |s, cx| s.load_models(false, cx));
        }
        self.sync_model(cx);
    }

    /// Rebuilds the parameter fields when the model in use changes, and drops
    /// choices the new model doesn't offer.
    pub(crate) fn sync_model(&mut self, cx: &mut Context<Self>) {
        let m = self.current_model(cx);
        let key = m.as_ref().map(model_key);
        if key == self.params_for {
            return;
        }
        self.params_for = key;
        let d = &mut self.draft;
        d.params.clear();
        self.param_fields.clear();
        let Some(m) = m else { return };
        if d.duration.is_some_and(|x| !m.durations.contains(&x)) {
            d.duration = None;
        }
        if d.resolution.as_ref().is_some_and(|r| !m.resolutions.contains(r)) {
            d.resolution = None;
        }
        if d.aspect.as_ref().is_some_and(|a| !m.aspect_ratios.is_empty() && !m.aspect_ratios.contains(a)) {
            d.aspect = None;
        }
        d.count = d.count.clamp(1, m.max_outputs.max(1));
        for p in &m.params {
            d.params.insert(p.key.clone(), p.default.clone());
        }
        self.param_fields = m.params.iter().map(|spec| ParamField::new(spec.clone(), cx)).collect();
        if d.audio_task.is_some() && !m.params.is_empty() {
            self.show_advanced = true;
        }
    }

    fn set_video(&mut self, video: bool, cx: &mut Context<Self>) {
        self.draft.set_video(video);
        self.prompt.update(cx, |i, cx| i.set_placeholder(if video { VIDEO_PLACEHOLDER } else { IMAGE_PLACEHOLDER }, cx));
        self.sync_model(cx);
        cx.notify();
    }

    fn set_mode(&mut self, mode: u8, cx: &mut Context<Self>) {
        self.set_video(mode == 1, cx);
        if mode >= 2 {
            self.draft.audio_task = Some(if mode == 3 { Task::TextToSpeech } else { Task::TextToAudio });
            self.draft.refs.clear(); self.draft.model = None; self.draft.duration = None; self.draft.params.clear();
            self.params_for = None;
            self.prompt.update(cx, |i, cx| i.set_placeholder(if mode == 3 { "Write the words to speak…" } else { "Describe music, atmosphere or a sound effect…" }, cx));
            self.sync_model(cx);
        }
        cx.notify();
    }

    pub(crate) fn pick_model(&mut self, m: &ModelInfo, cx: &mut Context<Self>) {
        let key = model_key(m);
        self.draft.model = Some(key.clone());
        self.picker_open = false;
        self.picker_query.update(cx, |i, cx| i.set_text("", cx));
        self.params_for = None;
        self.sync_model(cx);
        // Remember it for this mode (it is also what the CLI and MCP use by default).
        let setting = match self.draft.audio_task {
            Some(Task::TextToSpeech) => "generate.speechModel", Some(_) => "generate.audioModel",
            None if self.draft.video => "generate.videoModel", None => "generate.imageModel",
        };
        let current = self.store.read(cx).settings.get(setting).and_then(|v| v.as_str().map(str::to_string));
        if current.as_deref() != Some(key.as_str()) {
            self.store.update(cx, |s, cx| s.run("app.setSetting", json!({ "key": setting, "value": key }), cx));
        }
        cx.notify();
    }

    fn apply_compose(&mut self, req: crate::store::ComposeRequest, cx: &mut Context<Self>) {
        self.set_mode(match req.audio_task { Some(Task::TextToSpeech) => 3, Some(_) => 2, None if req.video => 1, None => 0 }, cx);
        if let Some(p) = req.prompt {
            self.prompt.update(cx, |i, cx| i.set_text(p, cx));
        }
        if let Some(n) = req.negative {
            self.negative.update(cx, |i, cx| i.set_text(n, cx));
        }
        if let Some(s) = req.seed {
            self.seed.update(cx, |i, cx| i.set_text(s, cx));
        }
        if req.model.is_some() {
            self.draft.model = req.model;
        }
        if req.duration.is_some() {
            self.draft.duration = req.duration;
        }
        if req.aspect.is_some() {
            self.draft.aspect = req.aspect;
        }
        self.draft.refs = req.refs;
        self.draft.refs.truncate(if self.draft.video { 2 } else { 8 });
        if req.target.is_some() {
            self.draft.to_timeline = true;
        }
        self.draft.target = req.target;
        self.params_for = None;
        self.sync_model(cx);
        if let Some(model) = self.current_model(cx) {
            self.param_fields = model.params.iter().map(|spec| {
                let mut spec = spec.clone();
                if let Some(value) = req.params.get(&spec.key) { spec.default = value.clone(); }
                self.draft.params.insert(spec.key.clone(), spec.default.clone());
                ParamField::new(spec, cx)
            }).collect();
        }
        if self.draft.negative_or_seed_set(&self.negative, &self.seed, cx) {
            self.show_advanced = true;
        }
        cx.notify();
    }

    fn remove_ref(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.draft.refs.len() {
            self.draft.refs.remove(i);
        }
        self.sync_model(cx);
        cx.notify();
    }

    fn add_ref(&mut self, r: ComposeRef, cx: &mut Context<Self>) {
        self.draft.add_ref(r);
        self.sync_model(cx);
        cx.notify();
    }

    fn pick_file(&mut self, cx: &mut Context<Self>) {
        let role = self.draft.next_role(self.current_model(cx).as_ref());
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use image".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update(cx, |this, cx| {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                    this.store.update(cx, |s, cx| s.error("Choose an image (PNG, JPEG, WebP, HEIC or AVIF).", cx));
                    return;
                }
                let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
                this.add_ref(ComposeRef { role, path: path.to_string_lossy().into_owned(), label, asset_id: None }, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Uses the frame under the playhead (the selected clip's, else the top picture there).
    fn grab_frame(&mut self, cx: &mut Context<Self>) {
        let role = self.draft.next_role(self.current_model(cx).as_ref());
        let s = self.store.read(cx);
        let ph = s.playback.read(cx).playhead;
        let fps = s.fps();
        let is_picture = |c: &&kimchi_core::Clip| {
            matches!(c.content, ClipContent::Media { .. }) && ph >= c.start && ph < c.end() && s.asset_of(c).is_some_and(|a| a.kind != MediaKind::Audio)
        };
        let clip = s.selected_clips().into_iter().find(is_picture).or_else(|| {
            s.project.as_ref().and_then(|p| p.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.hidden).flat_map(|t| t.clips.iter()).find(is_picture))
        });
        let Some(clip) = clip.cloned() else {
            self.store.update(cx, |s, cx| s.info("Move the playhead over a video or image clip first.", cx));
            return;
        };
        let label = format!("{} @ {}", clip.name, crate::ui::smpte(ph, fps));
        let task = self.store.update(cx, |s, cx| s.call("media.frame", json!({ "clipId": clip.id.to_string(), "time": ph }), cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| match r {
                Ok(v) => match v["path"].as_str() {
                    Some(path) => this.add_ref(ComposeRef { role, path: path.to_string(), label, asset_id: clip.asset_id() }, cx),
                    None => this.store.update(cx, |s, cx| s.error("The frame couldn't be saved.", cx)),
                },
                Err(e) => this.store.update(cx, |s, cx| s.error(e, cx)),
            })
            .ok();
        })
        .detach();
    }

    fn on_drop_media(&mut self, d: &MediaDrag, cx: &mut Context<Self>) {
        let Some(asset) = self.store.read(cx).asset(d.asset_id).cloned() else { return };
        if asset.kind != MediaKind::Image {
            self.store.update(cx, |s, cx| s.info("Drop an image here. For a video, put the playhead on it and use its frame.", cx));
            return;
        }
        let role = self.draft.next_role(self.current_model(cx).as_ref());
        self.add_ref(ComposeRef { role, path: asset.path.clone(), label: asset.name.clone(), asset_id: Some(asset.id) }, cx);
    }

    fn open_models_settings(cx: &mut App) {
        cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("models".into()) }, cx));
    }

    /// `generate.submit` with everything the composer holds.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.current_model(cx) else {
            Self::open_models_settings(cx);
            return;
        };
        if !self.can_submit(cx) || self.store.read(cx).project.is_none() {
            return;
        }
        let d = &self.draft;
        let prompt = self.prompt.read(cx).text().trim().to_string();
        let s = self.store.read(cx);
        let max = d.max_refs(Some(&m));
        // A library image goes by id (its provenance is recorded); a file or a frame by path.
        let images: Vec<Value> = d
            .refs
            .iter()
            .take(max)
            .map(|r| match r.asset_id.and_then(|id| s.asset(id)).filter(|a| a.kind == MediaKind::Image && a.path == r.path) {
                Some(a) => json!({ "role": r.role, "assetId": a.id.to_string() }),
                None => json!({ "role": r.role, "path": r.path }),
            })
            .collect();
        let mut p = json!({
            "prompt": prompt,
            "provider": m.provider,
            "model": m.id,
            "video": d.video,
            "task": d.task().as_str(),
            "images": images,
            "aspectRatio": d.aspect.clone().unwrap_or_else(|| self.project_ratio(cx)),
            "params": Value::Object(d.params.clone()),
            "place": if d.to_timeline { "timeline" } else { "library" },
        });
        let o = p.as_object_mut().expect("object");
        let negative = self.negative.read(cx).text().trim().to_string();
        if m.negative_prompt && !negative.is_empty() {
            o.insert("negativePrompt".into(), json!(negative));
        }
        if d.video || d.audio_task.is_some() {
            o.insert("duration".into(), json!(d.duration.or_else(|| m.durations.first().copied()).unwrap_or(5.0)));
        }
        if let Some(r) = d.resolution.clone().or_else(|| m.resolutions.first().cloned()) {
            o.insert("resolution".into(), json!(r));
        }
        if m.seed
            && let Ok(seed) = self.seed.read(cx).text().trim().parse::<i64>()
        {
            o.insert("seed".into(), json!(seed));
        }
        if m.max_outputs > 1 {
            o.insert("count".into(), json!(d.count.clamp(1, m.max_outputs)));
        }
        if m.audio {
            o.insert("audio".into(), json!(d.audio));
        }
        if d.to_timeline {
            match &d.target {
                Some(t) => {
                    if let Some(track) = t.track_id {
                        o.insert("trackId".into(), json!(track.to_string()));
                    }
                    o.insert("start".into(), json!(t.start));
                    o.insert("length".into(), json!(t.duration));
                }
                None => {
                    o.insert("start".into(), json!(s.playback.read(cx).playhead));
                }
            }
        }
        self.submitting = true;
        cx.notify();
        let task = self.store.update(cx, |s, cx| s.call("generate.submit", p, cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                this.submitting = false;
                match r {
                    Ok(_) => this.draft.target = None,
                    Err(e) => this.store.update(cx, |s, cx| s.error(e, cx)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- rendering -------------------------------------------------------

    fn connect_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (quick, description): (&[&'static str], &str) = match self.draft.audio_task {
            Some(Task::TextToSpeech) => (&["elevenlabs"], "Connect ElevenLabs to generate speech with voices from your account."),
            Some(_) => (&["elevenlabs", "stability"], "Connect ElevenLabs for music and sound effects, or Stability AI for audio and ambience."),
            None => (&["openrouter", "fal", "replicate", "openai", "google", "comfyui"], "Bring an API key from OpenRouter, fal, Replicate, OpenAI, Google, Runway, Luma… or run models on this computer with ComfyUI, Forge or Ollama."),
        };
        div()
            .mb(px(12.))
            .p(px(16.))
            .rounded(px(sz::R_LG))
            .bg(t.accent_soft)
            .border_1()
            .border_color(t.accent_ring)
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().size(px(32.)).mb(px(6.)).rounded(px(sz::R_MD)).bg(t.accent).flex().items_center().justify_center().child(icon("sparkles").size(px(18.)).text_color(t.text_on_accent)))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Connect a model"))
            .child(
                div()
                    .text_size(px(sz::SM))
                    .text_color(t.text_2)
                    .line_height(px(18.))
                    .mb(px(8.))
                    .child(description),
            )
            .child(
                div().flex().flex_wrap().gap(px(6.)).children(quick.iter().copied().map(|id| {
                    let name = self.provider_name(id, cx);
                    div()
                        .id(gpui::ElementId::Name(format!("connect-{id}").into()))
                        .h(px(26.))
                        .pl(px(7.))
                        .pr(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .bg(t.bg_sunken.opacity(0.5))
                        .border_1()
                        .border_color(t.line_strong)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .child(crate::ui::logo(id, px(14.)))
                        .child(name)
                        .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some(id.to_string()) }, cx)))
                })),
            )
            .child(div().mt(px(8.)).flex().child(Button::new("connect-open", "Open Models & keys").small().primary().with_icon("key-round").on_click(|_, _, cx| Self::open_models_settings(cx))))
    }

    fn refs_row(&self, model: Option<&ModelInfo>, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let max = self.draft.max_refs(model);
        let video = self.draft.video;
        let n = self.draft.refs.len();
        let add_tip = if video { if n > 0 { "End frame from a file" } else { "Start frame from a file" } } else { "Reference image from a file" };
        let add_box = |id: &'static str, ic: &'static str, tip: &'static str| {
            div()
                .id(id)
                .size(px(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(sz::R_MD))
                .border_1()
                .border_dashed()
                .border_color(t.line_strong)
                .text_color(t.text_2)
                .cursor_pointer()
                .hover(|s| s.text_color(t.text).border_color(t.text_3).bg(t.hover))
                .tooltip(move |_, cx| crate::ui::tooltip(tip.into(), cx))
                .child(icon(ic).size(px(15.)))
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .px(px(12.))
            .pt(px(4.))
            .pb(px(8.))
            .children(self.draft.refs.iter().enumerate().map(|(i, r)| {
                let label: gpui::SharedString = r.label.clone().into();
                div()
                    .id(("ref", i))
                    .group("ref")
                    .relative()
                    .w(px(52.))
                    .h(px(40.))
                    .rounded(px(sz::R_MD))
                    .overflow_hidden()
                    .border_1()
                    .border_color(t.line_strong)
                    .bg(t.bg_sunken)
                    .tooltip(move |_, cx| crate::ui::tooltip(label.clone(), cx))
                    .child(img(crate::views::generate::image_path(&r.path)).size_full().object_fit(gpui::ObjectFit::Cover))
                    .child(
                        div()
                            .absolute()
                            .left(px(3.))
                            .bottom(px(3.))
                            .px(px(4.))
                            .rounded(px(sz::R_XS))
                            .bg(gpui::black().opacity(0.72))
                            .text_color(gpui::white())
                            .text_size(px(9.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(role_label(r.role)),
                    )
                    .child(
                        div()
                            .id(("ref-remove", i))
                            .absolute()
                            .top(px(3.))
                            .right(px(3.))
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(gpui::black().opacity(0.72))
                            .text_color(gpui::white())
                            .opacity(0.)
                            .group_hover("ref", |s| s.opacity(1.))
                            .cursor_pointer()
                            .tooltip(|_, cx| crate::ui::tooltip("Remove".into(), cx))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_ref(i, cx)))
                            .child(icon("x").size(px(10.))),
                    )
            }))
            .when(n < max, |d| {
                d.child(add_box("ref-add-file", "image-plus", add_tip).on_click(cx.listener(|this, _, _, cx| this.pick_file(cx))))
                    .child(add_box("ref-add-frame", "scan-line", "Frame at the playhead").on_click(cx.listener(|this, _, _, cx| this.grab_frame(cx))))
            })
    }

    fn composer(&mut self, model: Option<&ModelInfo>, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let focused = self.prompt.read(cx).is_focused(window);
        let can_submit = self.can_submit(cx);
        let target = self.draft.target.clone().filter(|_| self.draft.to_timeline);
        let accent = t.accent;
        let soft = t.accent_soft;
        div()
            .id("composer")
            .relative()
            .rounded(px(sz::R_LG))
            .bg(t.bg_sunken.opacity(if t.is_dark() { 0.55 } else { 0.7 }))
            .border_1()
            .border_color(if focused { t.accent_ring } else { t.line_strong })
            .drag_over::<MediaDrag>(move |s, _, _, _| s.border_color(accent).bg(soft))
            .on_drop(cx.listener(|this, d: &MediaDrag, _, cx| this.on_drop_media(d, cx)))
            .when_some(target, |d, target| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .mx(px(12.))
                        .mt(px(10.))
                        .pl(px(8.))
                        .pr(px(4.))
                        .py(px(3.))
                        .max_w_full()
                        .text_size(px(11.5))
                        .text_color(t.accent_text)
                        .bg(t.accent_soft)
                        .child(icon("map-pin").size(px(12.)))
                        .child(div().min_w_0().truncate().child(format!("Lands {}", target.label)))
                        .child(
                            div()
                                .id("clear-target")
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover))
                                .tooltip(|_, cx| crate::ui::tooltip("Land at the playhead instead".into(), cx))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.draft.target = None;
                                    cx.notify();
                                }))
                                .child(icon("x").size(px(12.))),
                        ),
                )
            })
            .child(div().min_h(px(92.)).child(self.prompt.clone()))
            .when(self.draft.audio_task.is_none(), |d| d.child(self.refs_row(model, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .p(px(6.))
                    .pl(px(4.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(self.picker(model, window, cx))
                    .child(
                        div()
                            .id("generate-go")
                            .flex_none()
                            .size(px(36.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(t.accent)
                            .text_color(t.text_on_accent)
                            .tooltip(|_, cx| crate::ui::tooltip(format!("Generate ({})", crate::actions::keys_label("M-enter")).into(), cx))
                            .child(icon(if self.submitting { "loader-circle" } else { "arrow-up" }).size(px(17.)).text_color(t.text_on_accent))
                            .when(!can_submit, |d| d.opacity(0.35).cursor_not_allowed())
                            .when(can_submit, |d| {
                                d.cursor_pointer().hover(|s| s.bg(t.accent_hover)).active(|s| s.opacity(0.85)).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))
                            }),
                    ),
            )
    }

    fn settings(&mut self, m: &ModelInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let d = self.draft.clone();
        let project_ratio = self.project_ratio(cx);
        let ratios: Vec<String> = if m.aspect_ratios.is_empty() { COMMON_RATIOS.iter().map(|s| s.to_string()).collect() } else { m.aspect_ratios.clone() };
        let current_ratio = d.aspect.clone().unwrap_or_else(|| project_ratio.clone());
        let section = |name: &str, body: gpui::AnyElement| div().flex().flex_col().gap(px(6.)).child(eyebrow(name, cx)).child(body);
        let has_advanced = m.seed || m.negative_prompt || !m.params.is_empty();

        let aspect = div().flex().flex_wrap().gap(px(4.)).children(ratios.iter().enumerate().map(|(i, r)| {
            let on = &current_ratio == r;
            let (w, h) = ratio_parts(r);
            let glyph_h = 10.0;
            let glyph_w = (glyph_h * w / h).clamp(5.0, 20.0);
            let color = if on { t.accent_text } else { t.text_2 };
            let value = if *r == project_ratio { None } else { Some(r.clone()) };
            chip_base(("aspect", i), on, cx)
                .child(div().w(px(glyph_w)).h(px(glyph_h.min(glyph_w * h / w).max(5.))).border(px(1.5)).border_color(color))
                .child(r.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.draft.aspect = value.clone();
                    cx.notify();
                }))
        }));

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(16.))
            .px(px(2.))
            .when(d.audio_task.is_none(), |el| el.child(section("Aspect", aspect.into_any_element())))
            .when((d.video || d.audio_task.is_some()) && !m.durations.is_empty(), |el| {
                let current = d.duration.or_else(|| m.durations.first().copied());
                el.child(section(
                    "Length",
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(m.durations.iter().enumerate().map(|(i, &s)| {
                            chip(("duration", i), format!("{}s", trim_float(s)), current == Some(s), cx).on_click(cx.listener(move |this, _, _, cx| {
                                this.draft.duration = Some(s);
                                cx.notify();
                            }))
                        }))
                        .into_any_element(),
                ))
            })
            .when(m.resolutions.len() > 1, |el| {
                let current = d.resolution.clone().or_else(|| m.resolutions.first().cloned());
                el.child(section(
                    "Quality",
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(m.resolutions.iter().enumerate().map(|(i, r)| {
                            let r2 = r.clone();
                            chip(("resolution", i), r.clone(), current.as_ref() == Some(r), cx).on_click(cx.listener(move |this, _, _, cx| {
                                this.draft.resolution = Some(r2.clone());
                                cx.notify();
                            }))
                        }))
                        .into_any_element(),
                ))
            })
            .when(m.max_outputs > 1, |el| {
                el.child(section(
                    "Variations",
                    div()
                        .flex()
                        .gap(px(4.))
                        .children((1..=m.max_outputs.min(4)).map(|n| {
                            chip(("count", n as usize), n.to_string(), d.count == n, cx).on_click(cx.listener(move |this, _, _, cx| {
                                this.draft.count = n;
                                cx.notify();
                            }))
                        }))
                        .into_any_element(),
                ))
            })
            .when(m.audio, |el| {
                el.child(switch(
                    "gen-audio",
                    "Sound",
                    d.audio,
                    {
                        let this = cx.entity().downgrade();
                        move |on, _, cx| {
                            this.update(cx, |p, cx| {
                                p.draft.audio = on;
                                cx.notify();
                            })
                            .ok();
                        }
                    },
                    cx,
                ))
            })
            .when(has_advanced, |el| {
                el.child(
                    div()
                        .id("gen-more")
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(sz::SM))
                        .text_color(t.text_2)
                        .cursor_pointer()
                        .hover(|s| s.text_color(t.text))
                        .child(icon(if self.show_advanced { "chevron-up" } else { "chevron-down" }))
                        .child(if self.show_advanced { "Fewer options" } else { "More options" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_advanced = !this.show_advanced;
                            cx.notify();
                        })),
                )
            })
            .when(has_advanced && self.show_advanced, |el| el.child(self.advanced(m, cx)))
            .child(self.destination(cx))
    }

    fn advanced(&mut self, m: &ModelInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let fields: Vec<gpui::AnyElement> = (0..self.param_fields.len()).map(|i| self.render_param(i, cx)).collect();
        let field = |name: &str, body: gpui::AnyElement| div().flex().flex_col().gap(px(5.)).child(eyebrow(name, cx)).child(body);
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(sz::R_MD))
            .bg(t.bg_sunken.opacity(0.6))
            .border_1()
            .border_color(t.line)
            .when(m.negative_prompt, |d| d.child(field("Avoid", self.negative.clone().into_any_element())))
            .when(m.seed, |d| {
                d.child(field(
                    "Seed",
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(div().flex_1().child(self.seed.clone()))
                        .child(Button::icon("seed-roll", "dices", "Random seed").on_click(cx.listener(|this, _, _, cx| {
                            let seed = rand_seed();
                            this.seed.update(cx, |i, cx| i.set_text(seed.to_string(), cx));
                            cx.notify();
                        })))
                        .into_any_element(),
                ))
            })
            .children(fields)
    }

    fn destination(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let d = &self.draft;
        let label: gpui::AnyElement = if !d.to_timeline {
            div().child("Library only").into_any_element()
        } else if let Some(target) = &d.target {
            div().min_w_0().truncate().child(format!("On the timeline, {}", target.label)).into_any_element()
        } else {
            div().flex().gap(px(4.)).child("On the timeline at").child(self.playhead.clone()).into_any_element()
        };
        let this = cx.entity().downgrade();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(sz::R_MD))
            .bg(t.bg_sunken.opacity(0.5))
            .border_1()
            .border_color(t.line)
            .text_size(px(sz::SM))
            .text_color(t.text_2)
            .child(div().flex().items_center().gap(px(7.)).min_w_0().child(icon(if d.to_timeline { "clapperboard" } else { "folder" })).child(label))
            .child(switch(
                "gen-to-timeline",
                "",
                d.to_timeline,
                move |on, _, cx| {
                    this.update(cx, |p, cx| {
                        p.draft.to_timeline = on;
                        cx.notify();
                    })
                    .ok();
                },
                cx,
            ))
    }
}

impl Draft {
    fn negative_or_seed_set(&self, negative: &Entity<TextInput>, seed: &Entity<TextInput>, cx: &App) -> bool {
        !negative.read(cx).text().trim().is_empty() || !seed.read(cx).text().trim().is_empty()
    }
}

/// `5` rather than `5.0`, `2.5` stays.
fn trim_float(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") }
}

/// A seed for the dice button (no `rand` dependency: time and address bits are plenty).
fn rand_seed() -> u32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) as u64;
    let mut x = nanos ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x & 0x7fff_ffff) as u32
}

impl Render for GeneratePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.current_model(cx);
        let loading = self.store.read(cx).models_loading;
        let show_connect = self.connected(cx) == 0 && !loading;
        let mode = match self.draft.audio_task { Some(Task::TextToSpeech) => 3u8, Some(_) => 2, None if self.draft.video => 1, None => 0 };
        let this = cx.entity().downgrade();
        let composer = self.composer(model.as_ref(), window, cx).into_any_element();
        let settings = model.as_ref().map(|m| self.settings(m, cx).into_any_element());
        let recent = self.recent(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .child(div().flex_1().child(segmented(
                        "gen-mode",
                        vec![(0u8, "Image".into()), (1, "Video".into()), (2, "Audio".into()), (3, "Speech".into())],
                        mode,
                        move |v, _, cx| {
                            this.update(cx, |p, cx| p.set_mode(*v, cx)).ok();
                        },
                        cx,
                    ))),
            )
            .child(
                div()
                    .id("gen-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(12.))
                    .pb(px(16.))
                    .when(show_connect, |d| d.child(self.connect_card(cx)))
                    .child(composer)
                    .children(settings)
                    .children(recent),
            )
    }
}

/// The playhead time, re-rendered on its own while playing (the panel stays put).
pub struct PlayheadTime {
    playback: Entity<Playback>,
    _sub: Subscription,
}

impl PlayheadTime {
    fn new(playback: Entity<Playback>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&playback, |_, _, cx| cx.notify());
        Self { playback, _sub: sub }
    }
}

impl Render for PlayheadTime {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ph = self.playback.read(cx).playhead;
        div().font_family(MONO).text_color(cx.theme().text).child(timecode(ph))
    }
}
