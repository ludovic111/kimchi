//! The captions panel (left column): transcribe the cut with Whisper on this computer (model
//! and language, progress, cancel), import and export SRT / WebVTT, add one at the playhead,
//! style them all, and the list of captions (click to go there, double-click to edit the words
//! in the inspector).
//!
//! Every action is a `captions.*` command. The panel watches the running transcription (any
//! client may have started it) a few times a second.

use std::time::Duration;

use gpui::{App, Context, Entity, PathPromptOptions, Render, SharedString, Subscription, Task, Window, div, prelude::*, px};
use kimchi_core::{ClipContent, Id};
use serde_json::{Value, json};

use crate::store::{Store, StoreEvent, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::TextInput;
use crate::ui::scrub::{Scrub, ScrubChange, Step};
use crate::ui::{Button, caps, icon, segmented, switch};

/// A speech model as `captions.models` lists it.
#[derive(Clone)]
struct ModelRow {
    id: String,
    label: String,
    size_mb: u64,
    downloaded: bool,
}

pub struct CaptionsPanel {
    store: Entity<Store>,
    language: Entity<TextInput>,
    model: String,
    models: Vec<ModelRow>,
    /// The running transcription: stage and progress.
    running: Option<(String, f64)>,
    size: Entity<Scrub>,
    height: Entity<Scrub>,
    _subs: Vec<Subscription>,
    _poll: Task<()>,
}

impl CaptionsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let language = cx.new(|cx| TextInput::new(cx).placeholder("Detect the language"));
        let size = cx.new(|_| Scrub::new("Size", 1.0, 0).range(8.0, 400.0));
        let height = cx.new(|_| Scrub::new("Height", 2.0, 0));
        let mut subs = vec![cx.observe(&store, |_, _, cx| cx.notify())];
        subs.push(cx.subscribe(&size, |this: &mut Self, _, ch: &ScrubChange, cx| this.restyle(json!({ "style": { "fontSize": ch.value } }), "size", ch.step(), cx)));
        subs.push(cx.subscribe(&height, |this: &mut Self, _, ch: &ScrubChange, cx| this.restyle(json!({ "y": ch.value }), "y", ch.step(), cx)));
        // Watch the transcription (started here or by any other client).
        let poll = cx.spawn(async move |this, cx| {
            let mut busy = false;
            loop {
                // Quick while a transcription runs; a calm check otherwise (an idle window shouldn't wake often).
                cx.background_executor().timer(Duration::from_millis(if busy { 250 } else { 1500 })).await;
                let now = kimchi_control::commands::captions::current().map(|s| (s.stage.to_string(), s.progress));
                busy = now.is_some();
                let Ok(()) = this.update(cx, |this, cx| {
                    if this.running != now {
                        let finished = this.running.is_some() && now.is_none();
                        this.running = now;
                        if finished {
                            this.refresh_models(cx);
                        }
                        cx.notify();
                    }
                }) else {
                    break;
                };
            }
        });
        let mut this = Self { store, language, model: "base".into(), models: vec![], running: None, size, height, _subs: subs, _poll: poll };
        this.refresh_models(cx);
        this
    }

    fn refresh_models(&mut self, cx: &mut Context<Self>) {
        let task = self.store.update(cx, |s, cx| s.call("captions.models", json!({}), cx));
        cx.spawn(async move |this, cx| {
            let Ok(v) = task.await else { return };
            this.update(cx, |this, cx| {
                this.models = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|m| ModelRow {
                        id: m["id"].as_str().unwrap_or_default().to_string(),
                        label: m["label"].as_str().unwrap_or_default().to_string(),
                        size_mb: m["sizeMb"].as_u64().unwrap_or(0),
                        downloaded: m["downloaded"].as_bool().unwrap_or(false),
                    })
                    .collect();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn restyle(&mut self, mut params: Value, key: &str, step: Step, cx: &mut Context<Self>) {
        if let Some(k) = step.coalesce(format!("captions:{key}")) {
            params["coalesce"] = json!(k);
        }
        self.store.update(cx, |s, cx| s.run("captions.setStyle", params, cx));
    }

    fn transcribe(&mut self, cx: &mut Context<Self>) {
        let mut params = json!({ "model": self.model });
        let lang = self.language.read(cx).text().trim().to_string();
        if !lang.is_empty() {
            params["language"] = json!(lang);
        }
        // Shown at once; the poll takes over.
        self.running = Some(("mixing".into(), 0.0));
        cx.notify();
        self.store.update(cx, |s, cx| s.run("captions.transcribe", params, cx));
    }
}

/// What the running stage says.
fn stage_text(stage: &str, p: f64, model: Option<&ModelRow>) -> String {
    let pc = (p * 100.0).round();
    match stage {
        "downloading" => format!("Downloading the speech model ({} MB, once)… {pc}%", model.map_or(0, |m| m.size_mb)),
        "mixing" => "Mixing the sound…".into(),
        _ => format!("Listening… {pc}%"),
    }
}

fn import(cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Import captions".into()) });
    let store = cx.store();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = rx.await else { return };
        let Some(path) = paths.into_iter().next() else { return };
        store.update(cx, |s, cx| s.run("captions.import", json!({ "path": path.to_string_lossy() }), cx));
    })
    .detach();
}

fn export(name: String, cx: &mut App) {
    let dir = crate::views::dialogs::export::default_dir();
    let rx = cx.prompt_for_new_path(&dir, Some(&format!("{name}.srt")));
    let store = cx.store();
    cx.spawn(async move |cx| {
        let Some(path) = rx.await.ok().and_then(Result::ok).flatten() else { return };
        store.update(cx, |s, cx| {
            s.run_then("captions.export", json!({ "path": path.to_string_lossy() }), cx, |s, v, cx| {
                s.toast(kimchi_control::ToastKind::Success, format!("Saved {} captions", v["captions"]), cx)
            })
        });
    })
    .detach();
}

/// Selects a caption and moves the playhead to it (a double-click goes on to its words).
fn go_to(id: Id, start: f64, edit: bool, cx: &mut App) {
    cx.store().update(cx, |s, cx| {
        s.set_selection(vec![id], cx);
        s.playback.update(cx, |p, cx| p.seek(start + 0.001, cx));
        if edit {
            cx.emit(StoreEvent::EditText);
        }
    });
}

impl Render for CaptionsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let Some(project) = s.project.clone() else { return div().id("captions-panel") };
        let selection = s.selection.clone();
        let playhead = s.playback.read(cx).playhead;
        let captions: Vec<(Id, f64, f64, String)> = project.captions().into_iter().map(|(_, c, text)| (c.id, c.start, c.end(), text.to_string())).collect();
        let first_style = project.captions().first().and_then(|(_, c, _)| match &c.content {
            ClipContent::Text { style } => Some((style.font_size, style.background.is_some(), c.transform.y)),
            _ => None,
        });
        if let Some((fs, _, y)) = first_style {
            let h = project.settings.height as f64;
            self.size.update(cx, |s, _| s.set_value(fs));
            self.height.update(cx, |s, _| {
                s.min = -h / 2.0;
                s.max = h / 2.0;
                s.set_value(y)
            });
        }
        let _ = window;

        // Transcribe.
        let model = self.model.clone();
        let this = cx.entity().downgrade();
        let models = segmented(
            "speech-model",
            self.models.iter().map(|m| (m.id.clone(), SharedString::from(m.label.clone()))).collect(),
            model.clone(),
            move |v, _, cx| {
                this.update(cx, |this, cx| {
                    this.model = v.clone();
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let running = self.running.clone();
        let current_model = self.models.iter().find(|m| m.id == model).cloned();
        let needs_download = current_model.as_ref().is_some_and(|m| !m.downloaded);
        let has_sound = project.clips().any(|(tr, c)| !tr.muted && c.asset_id().and_then(|a| project.asset(a)).is_some_and(|a| a.meta.has_audio));
        let this = cx.entity().downgrade();
        let mut listen = div()
            .flex()
            .flex_col()
            .gap(px(9.))
            .px(px(14.))
            .py(px(14.))
            .border_b_1()
            .border_color(t.line)
            .child(caps("Transcribe", cx))
            .child(
                div()
                    .text_size(px(sz::SM))
                    .text_color(t.text_2)
                    .line_height(px(sz::SM * 1.5))
                    .child("Listens to the cut on this computer (Whisper) and puts the words on the captions track. Nothing is sent anywhere."),
            )
            .child(models)
            // The model's size under the choice (the segments stay short in a narrow panel).
            .when_some(self.models.iter().find(|m| m.id == model).filter(|m| !m.downloaded), |d, m| {
                d.child(div().text_size(px(sz::XS)).text_color(t.text_3).child(format!("{} MB, downloaded once", m.size_mb)))
            })
            .child(self.language.clone());
        listen = match &running {
            Some((stage, p)) => {
                let label = stage_text(stage, *p, current_model.as_ref());
                listen.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(div().flex().items_center().gap(px(6.)).text_size(px(sz::SM)).text_color(t.accent_text).child(icon("loader-circle").text_color(t.accent_text)).child(label))
                        .child(div().h(px(4.)).bg(t.line_strong).child(div().h_full().bg(t.accent).w(gpui::relative(p.clamp(0.02, 1.0) as f32))))
                        .child(Button::new("captions-cancel", "Cancel").small().on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("captions.cancel", json!({}), cx)))),
                )
            }
            None => listen.child(
                Button::new("captions-transcribe", if captions.is_empty() { "Transcribe the cut" } else { "Transcribe again" })
                    .primary()
                    .with_icon("captions")
                    .full_width()
                    .disabled(!has_sound)
                    .tooltip(if !has_sound {
                        "Nothing on the timeline makes a sound"
                    } else if needs_download {
                        "Downloads the speech model first (once)"
                    } else {
                        "Replaces the captions already there"
                    })
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.transcribe(cx)).ok();
                    }),
            ),
        };

        // Files and one more.
        let name = project.name.clone();
        let files = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(14.))
            .py(px(12.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(div().flex_1().child(Button::new("captions-import", "Import…").small().with_icon("import").full_width().tooltip("Read an .srt or .vtt file").on_click(|_, _, cx| import(cx))))
                    .child(
                        div().flex_1().child(
                            Button::new("captions-export", "Export…")
                                .small()
                                .with_icon("download")
                                .full_width()
                                .disabled(captions.is_empty())
                                .tooltip("Save them as an .srt (or .vtt) file")
                                .on_click(move |_, _, cx| export(name.clone(), cx)),
                        ),
                    ),
            )
            .child(Button::new("captions-add", "Add a caption at the playhead").small().with_icon("plus").full_width().on_click(move |_, _, cx| {
                cx.store().update(cx, |s, cx| {
                    s.run_then("captions.add", json!({ "text": "New caption", "start": playhead }), cx, |s, v, cx| {
                        if let Some(id) = v["clipId"].as_str().and_then(|i| i.parse().ok()) {
                            s.set_selection(vec![id], cx);
                            cx.emit(StoreEvent::EditText);
                        }
                    })
                })
            }));

        // Style, for all of them.
        let style = first_style.map(|(_, boxed, _)| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(14.))
                .py(px(12.))
                .border_b_1()
                .border_color(t.line)
                .child(caps("Look of every caption", cx))
                .child(div().grid().grid_cols(2).gap(px(6.)).child(self.size.clone()).child(self.height.clone()))
                .child(switch(
                    "captions-box",
                    "Box behind the words",
                    boxed,
                    |on, _, cx| {
                        let bg = if on { json!("#000000b3") } else { Value::Null };
                        cx.store().update(cx, |s, cx| s.run("captions.setStyle", json!({ "style": { "background": bg } }), cx))
                    },
                    cx,
                ))
        });

        // The list.
        let rows = captions.iter().enumerate().map(|(i, (id, start, end, text))| {
            let (id, start) = (*id, *start);
            let selected = selection.contains(&id);
            let here = playhead >= start && playhead < *end;
            div()
                .id(("caption", i))
                .flex()
                .gap(px(10.))
                .px(px(14.))
                .py(px(7.))
                .cursor_pointer()
                .when(selected, |d| d.bg(t.accent_soft))
                .when(!selected, |d| d.hover(|s| s.bg(t.hover)))
                .child(
                    div()
                        .flex_none()
                        .w(px(48.))
                        .font_family(MONO)
                        .text_size(px(sz::XS))
                        .text_color(if here { t.accent_text } else { t.text_3 })
                        .child(crate::ui::timecode(start)),
                )
                .child(div().flex_1().min_w_0().text_size(px(sz::SM)).text_color(t.text).line_height(px(sz::SM * 1.4)).child(text.clone()))
                .on_click(move |e, _, cx| go_to(id, start, e.click_count() >= 2, cx))
        });
        let list = div()
            .flex()
            .flex_col()
            .py(px(6.))
            .child(div().flex().items_center().justify_between().px(px(14.)).py(px(6.)).child(caps(format!("{} captions", captions.len()), cx)).when(!captions.is_empty(), |d| {
                d.child(Button::new("captions-clear", "Clear").small().danger().on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("captions.clear", json!({}), cx))))
            }))
            .when(captions.is_empty(), |d| {
                d.child(div().px(px(14.)).text_size(px(sz::SM)).text_color(t.text_3).child("None yet. Transcribe the cut, import a file, or add one at the playhead."))
            })
            .children(rows);

        div()
            .id("captions-panel")
            .size_full()
            .overflow_y_scroll()
            .child(listen)
            .child(files)
            .children(style)
            .child(list)
    }
}

#[cfg(test)]
pub fn go_to_for_test(id: Id, start: f64, cx: &mut App) {
    go_to(id, start, false, cx);
}
