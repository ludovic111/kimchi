//! Export: format, size, frame rate, range, quality and encoder (GPU or CPU, from
//! `export.encoders`), a destination from the save panel, then `export.start`. Progress comes from the store's export list (the session's events), so an
//! export started by the agent or the CLI shows here too, and closing the dialog doesn't stop it.

use std::path::PathBuf;

use gpui::{AnyElement, App, Context, Entity, FocusHandle, FontWeight, KeyBinding, Render, SharedString, Subscription, Window, actions, div, prelude::*, px, relative};
use kimchi_control::ExportStatus;
use serde_json::{Value, json};

use crate::store::{Dialog, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::scrub::{Scrub, ScrubChange};
use crate::ui::{Button, caps, icon, segmented};
use crate::views::home::short;

actions!(export_dialog, [Confirm]);

/// (id, name, extension, what it is for).
const FORMATS: [(&str, &str, &str, &str); 6] = [
    ("mp4", "MP4", "mp4", "H.264 · plays everywhere"),
    ("hevc", "HEVC", "mp4", "H.265 · half the size"),
    ("prores", "ProRes", "mov", "422 HQ · for finishing"),
    ("webm", "WebM", "webm", "VP9 · for the web"),
    ("gif", "GIF", "gif", "Loops, no sound"),
    ("audio", "Audio", "m4a", "Sound only · WAV, FLAC, MP3…"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Size {
    Project,
    P720,
    P1080,
    P2160,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Range {
    Whole,
    Selection,
    Custom,
}

pub struct ExportDialog {
    store: Entity<Store>,
    /// The export preset picked (`export.presets`); the choices below go on top of it.
    preset: Option<&'static str>,
    format: usize,
    quality: &'static str,
    /// auto, hardware or software.
    encoder: &'static str,
    /// `export.encoders`: what each format is encoded with here; `None` until it answers.
    encoders: Option<Value>,
    /// Captions: "burn", "file", "both" or "none".
    captions: &'static str,
    size: Size,
    /// `None`: the project's frame rate.
    fps: Option<u32>,
    range: Range,
    from: Entity<Scrub>,
    to: Entity<Scrub>,
    /// A sound-only export's file type, rate, depth or bitrate, and stems.
    pub(crate) sound: crate::views::mixer::export::SoundOptions,
    /// The export this dialog follows.
    job: Option<String>,
    /// The save panel is up, or `export.start` is running.
    preparing: bool,
    focus: FocusHandle,
    open: bool,
    opened_at: chrono::DateTime<chrono::Utc>,
    _subs: Vec<Subscription>,
}

impl ExportDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let from = cx.new(|_| Scrub::new("From", 0.1, 2).unit("s").range(0.0, f64::MAX));
        let to = cx.new(|_| Scrub::new("To", 0.1, 2).unit("s").range(0.0, f64::MAX));
        let mut subs = vec![cx.observe_in(&store, window, |this: &mut Self, store, window, cx| {
            let open = matches!(store.read(cx).dialog, Some(Dialog::Export));
            if open && !this.open {
                this.on_open(window, cx);
            } else if open && this.job.is_none() && !this.preparing {
                // An export another client started while the dialog is up.
                let since = this.opened_at;
                this.job = store.read(cx).exports.iter().filter(|e| !e.done && e.started_at >= since).max_by_key(|e| e.started_at).map(|e| e.id.clone());
            }
            this.open = open;
            cx.notify();
        })];
        subs.push(cx.subscribe(&from, |_, _, _: &ScrubChange, cx| cx.notify()));
        subs.push(cx.subscribe(&to, |_, _, _: &ScrubChange, cx| cx.notify()));
        cx.bind_keys([KeyBinding::new("enter", Confirm, Some("ExportDialog"))]);
        Self {
            store,
            preset: None,
            format: 0,
            quality: "standard",
            encoder: "auto",
            encoders: None,
            captions: "burn",
            size: Size::Project,
            fps: None,
            range: Range::Whole,
            from,
            to,
            sound: Default::default(),
            job: None,
            preparing: false,
            focus: cx.focus_handle(),
            open: false,
            opened_at: chrono::Utc::now(),
            _subs: subs,
        }
    }

    fn on_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.opened_at = chrono::Utc::now();
        let s = self.store.read(cx);
        // Follow an export that is still running (started here earlier, or by another client).
        self.job = s.exports.iter().filter(|e| !e.done).max_by_key(|e| e.started_at).map(|e| e.id.clone());
        self.preparing = false;
        let d = s.duration();
        let (a, b) = self.selection_span(cx).unwrap_or((0.0, d));
        self.from.update(cx, |sc, _| sc.set_value(a));
        self.to.update(cx, |sc, _| sc.set_value(b));
        if self.range == Range::Selection && self.selection_span(cx).is_none() {
            self.range = Range::Whole;
        }
        window.focus(&self.focus, cx);
        if self.encoders.is_none() {
            // The first time, this tries each hardware encoder on a few frames (about a second).
            let task = self.store.update(cx, |s, cx| s.call("export.encoders", json!({}), cx));
            cx.spawn(async move |this, cx| {
                let found = task.await.ok();
                this.update(cx, |this, cx| {
                    this.encoders = found;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// Whether the format has a video encoder to choose (not sound only, not GIF).
    fn has_encoder_choice(&self) -> bool {
        !matches!(FORMATS[self.format].0, "audio" | "gif")
    }

    /// `{ id, label }` of the encoder `choice` gives the chosen format; `Some(Null)` when there is
    /// none, `None` while `export.encoders` hasn't answered.
    fn encoder_for(&self, choice: &str) -> Option<Value> {
        let formats = self.encoders.as_ref()?["formats"].as_array()?;
        let id = FORMATS[self.format].0;
        Some(formats.iter().find(|f| f["format"] == id).map(|f| f[choice].clone()).unwrap_or(Value::Null))
    }

    /// Start and end of the selected clips.
    fn selection_span(&self, cx: &App) -> Option<(f64, f64)> {
        let clips = self.store.read(cx).selected_clips();
        if clips.is_empty() {
            return None;
        }
        let a = clips.iter().map(|c| c.start).fold(f64::MAX, f64::min);
        let b = clips.iter().map(|c| c.start + c.duration).fold(0.0, f64::max);
        Some((a, b))
    }

    fn status<'a>(&self, store: &'a Store) -> Option<&'a ExportStatus> {
        let id = self.job.as_ref()?;
        store.exports.iter().find(|e| &e.id == id)
    }

    /// Output size for the chosen preset (even numbers, the short side at the preset).
    fn dims(&self, cx: &App) -> (u32, u32) {
        let Some(p) = self.store.read(cx).project.clone() else { return (1920, 1080) };
        let (w, h) = (p.settings.width as f64, p.settings.height as f64);
        let short = match self.size {
            Size::Project => {
                let fitted = self.preset.and_then(|id| kimchi_control::commands::export::preset(id).ok()).and_then(|pr| kimchi_control::commands::export::preset_size(pr, &p.settings));
                return fitted.unwrap_or((p.settings.width, p.settings.height));
            }
            Size::P720 => 720.0,
            Size::P1080 => 1080.0,
            Size::P2160 => 2160.0,
        };
        let k = short / if w >= h { h } else { w };
        let even = |v: f64| ((v * k / 2.0).round() * 2.0) as u32;
        (even(w), even(h))
    }

    /// The range to render, `None` for the whole project.
    fn span(&self, cx: &App) -> Option<(f64, f64)> {
        match self.range {
            Range::Whole => None,
            Range::Selection => self.selection_span(cx),
            Range::Custom => Some((self.from.read(cx).value(), self.to.read(cx).value())),
        }
    }

    fn can_start(&self, cx: &App) -> bool {
        let d = self.store.read(cx).duration();
        let range_ok = self.span(cx).is_none_or(|(a, b)| b > a + 0.01);
        let encoder_ok = !self.has_encoder_choice() || self.encoder_for(self.encoder).is_none_or(|e| !e.is_null());
        !self.preparing && d > 0.0 && range_ok && encoder_ok
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let Some(project) = self.store.read(cx).project.clone() else { return };
        let (id, _, ext, _) = FORMATS[self.format];
        let (w, h) = self.dims(cx);
        let mut params = json!({ "format": id, "quality": self.quality });
        if let Some(p) = self.preset {
            params["preset"] = json!(p);
        }
        // Sound only: its file type (and stems go into a folder, without an extension).
        let ext = if id == "audio" { self.sound.extension().unwrap_or("") } else { ext };
        if id == "audio"
            && let (Some(o), Some(extra)) = (params.as_object_mut(), self.sound.params().as_object().cloned())
        {
            o.extend(extra);
        }
        if !project.captions().is_empty() {
            params["captions"] = json!(self.captions);
        }
        if self.has_encoder_choice() {
            params["encoder"] = json!(self.encoder);
        }
        if self.size != Size::Project && id != "audio" {
            params["width"] = json!(w);
            params["height"] = json!(h);
        }
        if let Some(f) = self.fps.filter(|_| id != "audio") {
            params["fps"] = json!(f);
        }
        if let Some((a, b)) = self.span(cx) {
            params["from"] = json!(a);
            params["to"] = json!(b);
        }
        let name = format!("{}.{ext}", sanitize(&project.name));
        let dir = default_dir();
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        self.preparing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().and_then(Result::ok).flatten();
            this.update(cx, |this, cx| {
                let Some(mut path) = picked else {
                    this.preparing = false;
                    cx.notify();
                    return;
                };
                if !ext.is_empty() && path.extension().is_none_or(|e| !e.eq_ignore_ascii_case(ext)) {
                    path.set_extension(ext);
                }
                params["path"] = json!(path.to_string_lossy());
                let weak = cx.entity().downgrade();
                let task = this.store.update(cx, |s, cx| s.call("export.start", params, cx));
                cx.spawn(async move |_, cx| {
                    let r = task.await;
                    weak
                        .update(cx, |this, cx| {
                            this.preparing = false;
                            match r {
                                Ok(v) => this.job = v["exportId"].as_str().map(str::to_string),
                                Err(e) => this.store.update(cx, |s, cx| s.error(e, cx)),
                            }
                            cx.notify();
                        })
                        .ok();
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    fn cancel_export(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.job.clone() {
            self.store.update(cx, |s, cx| s.run("export.cancel", json!({ "exportId": id }), cx));
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let done = self.status(self.store.read(cx)).is_none_or(|e| e.done);
        if done {
            self.job = None;
        }
        self.store.update(cx, |s, cx| s.close_dialog(cx));
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        match self.status(self.store.read(cx)).map(|e| e.done) {
            None => self.start(cx),
            Some(true) => self.close(cx),
            Some(false) => {}
        }
    }

    // ---- pieces ---------------------------------------------------------------

    fn options(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        // A sound-only export's file type, rate, depth or bitrate, and stems.
        let sound_rows = if FORMATS[self.format].0 == "audio" { crate::views::mixer::export::rows(&self.sound, cx.entity().downgrade(), |d: &mut Self| &mut d.sound, cx) } else { vec![] };
        let s = self.store.read(cx);
        let p = s.project.clone();
        let audio = FORMATS[self.format].0 == "audio";
        let has_selection = !s.selection.is_empty();
        let (pw, ph, pfps) = p.as_ref().map(|p| (p.settings.width, p.settings.height, p.settings.fps)).unwrap_or((1920, 1080, 30.0));
        let weak = cx.entity().downgrade();
        let set = move |f: fn(&mut Self, &Value), v: Value| {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| {
                    f(this, &v);
                    cx.notify();
                })
                .ok();
            }
        };
        let cards = FORMATS.iter().enumerate().map(|(i, (_, name, _, desc))| {
            let on = self.format == i;
            div()
                .id(("export-format", i))
                .flex()
                .flex_col()
                .gap(px(3.))
                .p(px(12.))
                .rounded(px(sz::R_MD))
                .border_1()
                .cursor_pointer()
                .role(gpui::Role::RadioButton)
                .aria_label(SharedString::from(format!("{name}: {desc}")))
                .when(on, |d| d.bg(t.accent_soft).border_color(t.accent))
                .when(!on, |d| d.bg(t.hover).border_color(t.line).hover(|s| s.bg(t.pressed)))
                .on_click(set(
                    |this, v| {
                        this.format = v.as_u64().unwrap_or(0) as usize;
                        this.preset = None;
                    },
                    json!(i),
                ))
                .child(div().font_weight(FontWeight::BOLD).when(on, |d| d.text_color(t.accent_text)).child(*name))
                .child(div().text_size(px(sz::XS)).text_color(t.text_2).child(*desc))
        });
        let row = |label: &str, control: AnyElement| div().flex().items_center().gap(px(12.)).child(div().w(px(100.)).flex_none().child(caps(label.to_string(), cx))).child(div().flex_1().child(control));
        let weak = cx.entity().downgrade();
        let w2 = weak.clone();
        let w3 = weak.clone();
        let w4 = weak.clone();
        let w5 = weak.clone();
        let size = segmented(
            "export-size",
            vec![(Size::Project, format!("{pw}×{ph}").into()), (Size::P720, "720p".into()), (Size::P1080, "1080p".into()), (Size::P2160, "4K".into())],
            self.size,
            move |v, _, cx| {
                weak.update(cx, |this, cx| {
                    this.size = *v;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let fps = segmented(
            "export-fps",
            vec![(None, format!("{} fps", trim_fps(pfps)).into()), (Some(24), "24".into()), (Some(25), "25".into()), (Some(30), "30".into()), (Some(60), "60".into())],
            self.fps,
            move |v, _, cx| {
                w2.update(cx, |this, cx| {
                    this.fps = *v;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let mut ranges = vec![(Range::Whole, SharedString::from(format!("Whole cut · {}", short(s.duration()))))];
        if has_selection {
            ranges.push((Range::Selection, "Selected clips".into()));
        }
        ranges.push((Range::Custom, "Custom".into()));
        let range = segmented(
            "export-range",
            ranges,
            self.range,
            move |v, _, cx| {
                w3.update(cx, |this, cx| {
                    this.range = *v;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let quality = segmented(
            "export-quality",
            vec![("draft", "Draft".into()), ("standard", "Standard".into()), ("high", "High".into())],
            self.quality,
            move |v, _, cx| {
                w4.update(cx, |this, cx| {
                    this.quality = v;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let encoder = segmented(
            "export-encoder",
            vec![("auto", "Auto".into()), ("hardware", "GPU".into()), ("software", "CPU".into())],
            self.encoder,
            move |v, _, cx| {
                w5.update(cx, |this, cx| {
                    this.encoder = v;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let has_captions = self.store.read(cx).project.as_ref().is_some_and(|p| !p.captions().is_empty());
        let w6 = cx.entity().downgrade();
        let captions = segmented(
            "export-captions",
            vec![("burn", "In the picture".into()), ("file", ".srt file".into()), ("both", "Both".into()), ("none", "Off".into())],
            self.captions,
            move |v, _, cx| {
                w6.update(cx, |this, cx| {
                    this.captions = v;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        // What the choice means on this computer.
        let (note, warn) = match self.encoder_for(self.encoder) {
            None => ("Checking this computer's encoders…".to_string(), false),
            Some(Value::Null) if self.encoder == "hardware" => (format!("No GPU encoder for {} on this computer.", FORMATS[self.format].1), true),
            Some(Value::Null) => ("No encoder for this format in this ffmpeg build.".to_string(), true),
            Some(e) => {
                let label = e["label"].as_str().unwrap_or_default();
                let on_gpu = self.encoder_for("hardware").is_some_and(|h| h["id"] == e["id"]);
                match (self.encoder, on_gpu) {
                    ("auto", true) => (format!("{label}, on the GPU · the CPU takes over if it fails"), false),
                    ("auto", false) => (format!("{label} · no GPU encoder for this format here"), false),
                    _ => (label.to_string(), false),
                }
            }
        };
        let bad_range = self.range == Range::Custom && self.to.read(cx).value() <= self.from.read(cx).value() + 0.01;
        let presets = self.presets(cx);
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(presets)
            .child(div().grid().grid_cols(3).gap(px(8.)).children(cards))
            .when(!audio, |d| d.child(row("Resolution", size.into_any_element())).child(row("Frame rate", fps.into_any_element())))
            .child(row("Range", range.into_any_element()))
            .when(self.range == Range::Custom, |d| {
                d.child(
                    div()
                        .pl(px(112.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(div().flex().gap(px(8.)).child(div().flex_1().child(self.from.clone())).child(div().flex_1().child(self.to.clone())))
                        .when(bad_range, |d| d.child(div().text_size(px(sz::XS)).text_color(t.warning).child("The end must come after the start."))),
                )
            })
            .when(self.range == Range::Selection, |d| {
                let (a, b) = self.selection_span(cx).unwrap_or((0.0, 0.0));
                d.child(div().pl(px(112.)).font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(format!("{} → {} ({})", crate::ui::timecode(a), crate::ui::timecode(b), short(b - a))))
            })
            .child(row("Quality", quality.into_any_element()))
            .when(self.has_encoder_choice(), |d| {
                d.child(row("Encoder", encoder.into_any_element())).child(
                    div()
                        .id("export-encoder-note")
                        .pl(px(112.))
                        .text_size(px(sz::XS))
                        .text_color(if warn { t.warning } else { t.text_2 })
                        .child(note),
                )
            })
            .when(has_captions && !audio, |d| d.child(row("Captions", captions.into_any_element())))
            .children(sound_rows.into_iter().map(|(label, el)| row(label, el)))
            .child(row("Loudness", crate::views::mixer::export_loudness(cx)))
            .into_any_element()
    }

    /// Picks an export preset: its format, quality and sound file type fill the choices below.
    fn pick_preset(&mut self, id: Option<&'static str>) {
        self.preset = id;
        let Some(p) = id.and_then(|id| kimchi_control::commands::export::preset(id).ok()) else { return };
        let format = if p.format == "wav" { "audio" } else { p.format };
        if let Some(i) = FORMATS.iter().position(|f| f.0 == format) {
            self.format = i;
        }
        self.quality = match p.quality {
            "draft" => "draft",
            "high" => "high",
            _ => "standard",
        };
        self.size = Size::Project;
        self.fps = None;
        if format == "audio" {
            self.sound.format = match (p.format, p.audio_format) {
                ("wav", _) => "wav",
                (_, Some("mp3")) => "mp3",
                (_, Some("flac")) => "flac",
                _ => "aac",
            };
        }
    }

    /// The preset chips by group, with what the picked one does.
    fn presets(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let weak = cx.entity().downgrade();
        let chip = |id: Option<&'static str>, label: &'static str, tip: &'static str, on: bool| {
            let weak = weak.clone();
            Button::new(SharedString::from(format!("export-preset-{}", id.unwrap_or("none"))), label).small().selected(on).tooltip(tip).on_click(move |_, _, cx| {
                weak.update(cx, |this, cx| {
                    this.pick_preset(id);
                    cx.notify();
                })
                .ok();
            })
        };
        let mut groups: Vec<(&'static str, Vec<&'static kimchi_control::commands::export::Preset>)> = vec![];
        for p in kimchi_control::commands::export::PRESETS {
            match groups.iter_mut().find(|(g, _)| *g == p.group) {
                Some((_, v)) => v.push(p),
                None => groups.push((p.group, vec![p])),
            }
        }
        let picked = self.preset.and_then(|id| kimchi_control::commands::export::preset(id).ok());
        let project = self.store.read(cx).project.clone();
        let warning = match (picked, project) {
            (Some(p), Some(project)) => match (p.size, kimchi_control::commands::export::preset_size(p, &project.settings)) {
                (kimchi_control::commands::export::PresetSize::Fit(bw, bh), Some((w, h))) if (w, h) != (bw, bh) => {
                    Some(format!("This project is {}×{}, so it comes out {w}×{h}. A {bw}×{bh} project fills the frame.", project.settings.width, project.settings.height))
                }
                _ => None,
            },
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(caps("Preset".to_string(), cx))
            .child(div().flex().flex_wrap().gap(px(4.)).child(chip(None, "Custom", "Choose everything below yourself", picked.is_none())).children(groups.into_iter().flat_map(|(_, ps)| {
                ps.into_iter().map(|p| chip(Some(p.id), p.label, p.note, self.preset == Some(p.id))).collect::<Vec<_>>()
            })))
            .children(picked.map(|p| div().text_size(px(sz::XS)).text_color(t.text_2).child(p.note)))
            .children(warning.map(|w| div().text_size(px(sz::XS)).text_color(t.warning).child(w)))
            .into_any_element()
    }

    fn progress(&self, e: &ExportStatus, cx: &App) -> AnyElement {
        let t = cx.theme().clone();
        let base = div().flex().flex_col().items_center().gap(px(12.)).pt(px(16.)).pb(px(8.)).text_center();
        let path = div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(e.path.clone());
        if let Some(err) = &e.error {
            let cancelled = err.to_lowercase().contains("cancel");
            return base
                .child(
                    div()
                        .size(px(44.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if cancelled { t.hover } else { t.danger.opacity(0.14) })
                        .child(icon(if cancelled { "circle-stop" } else { "circle-alert" }).size(px(20.)).text_color(if cancelled { t.text_2 } else { t.danger })),
                )
                .child(div().font_weight(FontWeight::SEMIBOLD).child(if cancelled { "Export cancelled" } else { "The export failed" }))
                .when(!cancelled, |d| {
                    d.child(
                        div()
                            .id("export-error")
                            .w_full()
                            .max_h(px(180.))
                            .overflow_y_scroll()
                            .p(px(10.))
                            .rounded(px(sz::R_SM))
                            .bg(t.bg_sunken)
                            .text_left()
                            .font_family(MONO)
                            .text_size(px(sz::XS))
                            .text_color(t.danger)
                            .child(err.clone()),
                    )
                })
                .into_any_element();
        }
        if e.done {
            return base
                .child(div().size(px(44.)).flex().items_center().justify_center().bg(t.success.opacity(0.16)).child(icon("check").size(px(20.)).text_color(t.success)))
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Exported"))
                .child(path)
                .into_any_element();
        }
        let pct = (e.progress.clamp(0.0, 1.0) * 100.0).round();
        let encoder = e.encoder.as_deref().map(|id| {
            div().text_size(px(sz::XS)).text_color(t.text_2).child(format!("Encoding with {}", kimchi_media::accel::label(id)))
        });
        base.child(
            div()
                .flex()
                .items_baseline()
                .font_family(MONO)
                .child(div().text_size(px(48.)).font_weight(FontWeight::LIGHT).child(format!("{pct}")))
                .child(div().text_size(px(sz::XL)).text_color(t.text_2).child("%")),
        )
        .child(
            div()
                .id("export-progress")
                .w_full()
                .h(px(6.))
                .bg(t.line)
                .overflow_hidden()
                .role(gpui::Role::ProgressIndicator)
                .aria_label(SharedString::from(format!("Exporting, {pct}%")))
                .child(div().h_full().w(relative(e.progress.clamp(0.0, 1.0) as f32)).bg(t.accent)),
        )
        .child(path)
        .children(encoder)
        .into_any_element()
    }

    fn footer(&self, status: Option<&ExportStatus>, cx: &mut Context<Self>) -> AnyElement {
        let row = div().flex().justify_end().gap(px(8.));
        match status {
            Some(e) if e.done => {
                let path = PathBuf::from(&e.path);
                let ok = e.error.is_none();
                row.when(ok, |d| d.child(Button::new("export-reveal", crate::ui::reveal_label()).with_icon("folder-search").on_click(move |_, _, cx| cx.reveal_path(&path))))
                    .when(!ok, |d| {
                        d.child(Button::new("export-back", "Back").ghost().on_click(cx.listener(|this, _, _, cx| {
                            this.job = None;
                            cx.notify();
                        })))
                    })
                    .child(Button::new("export-done", "Done").primary().on_click(cx.listener(|this, _, _, cx| this.close(cx))))
                    .into_any_element()
            }
            Some(_) => row
                .child(Button::new("export-hide", "Hide").ghost().tooltip("The export keeps going").on_click(cx.listener(|this, _, _, cx| this.close(cx))))
                .child(Button::new("export-cancel", "Cancel export").danger().on_click(cx.listener(|this, _, _, cx| this.cancel_export(cx))))
                .into_any_element(),
            None => {
                let audio = FORMATS[self.format].0 == "audio";
                let (w, h) = self.dims(cx);
                let label = if self.preparing {
                    "Choosing where…".to_string()
                } else if audio {
                    "Export audio".into()
                } else {
                    format!("Export {w}×{h}")
                };
                row.child(Button::new("export-close", "Cancel").ghost().on_click(cx.listener(|this, _, _, cx| this.close(cx))))
                    .child(
                        Button::new("export-start", label)
                            .primary()
                            .with_icon(if self.preparing { "loader-circle" } else { "share" })
                            .disabled(!self.can_start(cx))
                            .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for ExportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let store = self.store.read(cx);
        let subtitle = store.project.as_ref().map(|p| format!("{} · {}", p.name, short(p.duration()))).unwrap_or_default();
        let empty = store.duration() <= 0.0;
        let status = self.status(store).cloned();
        let body = match &status {
            Some(e) => self.progress(e, cx),
            None => self.options(cx),
        };
        let footer = self.footer(status.as_ref(), cx);
        div()
            .id("export-dialog")
            .key_context("ExportDialog")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::confirm))
            .role(gpui::Role::Dialog)
            .aria_label("Export")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .px(px(20.))
                    .pt(px(18.))
                    .pb(px(14.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(sz::LG)).font_weight(FontWeight::SEMIBOLD).child("Export"))
                            .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(subtitle)),
                    )
                    .child(Button::icon("export-x", "x", "Close (Esc)").on_click(cx.listener(|this, _, _, cx| this.close(cx)))),
            )
            .child(
                div()
                    .id("export-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(20.))
                    .pb(px(16.))
                    .child(body)
                    .when(empty && status.is_none(), |d| d.child(div().mt(px(12.)).text_size(px(sz::SM)).text_color(t.warning).child("The timeline is empty: add something to export."))),
            )
            .child(div().flex_none().debug_selector(|| "export-footer".into()).px(px(20.)).py(px(14.)).border_t_1().border_color(t.line).child(footer))
    }
}

/// Where the save panel opens: ~/Movies, else home.
/// Movies (macOS), Videos (Windows, Linux), else the home folder.
pub fn default_dir() -> PathBuf {
    dirs::video_dir().filter(|d| d.is_dir()).or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir)
}

/// A file name from a project name.
fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c }).collect();
    let s = s.trim();
    if s.is_empty() { "kimchi export".into() } else { s.to_string() }
}

fn trim_fps(f: f64) -> String {
    if (f - f.round()).abs() < 0.001 { format!("{}", f.round()) } else { format!("{f:.2}") }
}
