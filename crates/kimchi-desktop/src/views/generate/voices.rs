//! The Sound mode's own settings: the voice picker for speech (with the provider's samples when
//! it has them), length chips for music and sound effects, instrumental and lyrics for music.
//! Voices come from `generate.voices`; a sample is fetched and played through the speakers.

use gpui::{AnyElement, App, Context, ElementId, FontWeight, SharedString, anchored, deferred, div, prelude::*, px, relative};
use kimchi_gen::{ModelInfo, Task, Voice};
use serde_json::json;

use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{GlassExt, icon, switch};
use crate::views::generate::{bounds_probe, chip, eyebrow, model_key};
use crate::views::generate_panel::GeneratePanel;

/// A speech model's voices, as far as they are known.
#[derive(Clone, Debug)]
pub enum VoiceState {
    Loading,
    Ready(Vec<Voice>),
    Failed(String),
}

impl GeneratePanel {
    /// Asks for a speech model's voices once (the harness caches them too).
    pub(crate) fn load_voices(&mut self, m: &ModelInfo, cx: &mut Context<Self>) {
        let key = model_key(m);
        if matches!(self.voices.get(&key), Some(VoiceState::Loading | VoiceState::Ready(_))) {
            return;
        }
        self.voices.insert(key.clone(), VoiceState::Loading);
        let task = self.store.update(cx, |s, cx| s.call("generate.voices", json!({ "provider": m.provider, "model": m.id }), cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                let state = match r {
                    Ok(v) => VoiceState::Ready(serde_json::from_value(v["voices"].clone()).unwrap_or_default()),
                    Err(e) => VoiceState::Failed(e),
                };
                this.voices.insert(key, state);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn voice_list(&self, cx: &App) -> Vec<Voice> {
        let Some(m) = self.current_model(cx) else { return vec![] };
        match self.voices.get(&model_key(&m)) {
            Some(VoiceState::Ready(v)) => v.clone(),
            _ => vec![],
        }
    }

    /// The voice the request will use: the one chosen, else the model's default.
    pub(crate) fn current_voice(&self, m: &ModelInfo, cx: &App) -> Option<Voice> {
        let id = self.draft.voice.clone().or_else(|| m.default_voice.clone())?;
        Some(self.voice_list(cx).into_iter().find(|v| v.id == id).unwrap_or_else(|| Voice::new(id.clone(), id)))
    }

    /// Voices matching the search, the person's own first.
    pub(crate) fn voice_matches(&self, cx: &App) -> Vec<Voice> {
        let q = self.voice_query.read(cx).text().trim().to_lowercase();
        self.voice_list(cx)
            .into_iter()
            .filter(|v| {
                q.is_empty()
                    || [Some(&v.name), v.description.as_ref(), v.language.as_ref(), v.gender.as_ref(), Some(&v.id)]
                        .into_iter()
                        .flatten()
                        .any(|s| s.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub(crate) fn pick_voice(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.draft.voice = id;
        self.voice_open = false;
        self.voice_query.update(cx, |i, cx| i.set_text("", cx));
        cx.notify();
    }

    /// Plays a voice's sample (a second click stops waiting for it).
    fn play_sample(&mut self, v: &Voice, cx: &mut Context<Self>) {
        let Some(url) = v.preview_url.clone() else { return };
        if self.sample.as_deref() == Some(v.id.as_str()) {
            self.sample = None;
            crate::views::mixer::scrub::stop(cx);
            cx.notify();
            return;
        }
        self.sample = Some(v.id.clone());
        cx.notify();
        let id = v.id.clone();
        let task = crate::views::mixer::scrub::play_url(url, cx);
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                if this.sample.as_deref() == Some(id.as_str()) {
                    this.sample = None;
                }
                if let Err(e) = r {
                    this.store.update(cx, |s, cx| s.error(format!("The sample couldn't play: {e}"), cx));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Speech: the voice; music: length, instrumental, lyrics; sound effects: length.
    pub(crate) fn sound_settings(&mut self, m: &ModelInfo, cx: &mut Context<Self>) -> AnyElement {
        let task = self.draft.task();
        let section = |name: &str, body: AnyElement, cx: &App| div().flex().flex_col().gap(px(6.)).child(eyebrow(name, cx)).child(body).into_any_element();
        let mut out: Vec<AnyElement> = vec![];
        if task == Task::TextToSpeech && m.voices {
            out.push(section("Voice", self.voice_picker(m, cx), cx));
        }
        if matches!(task, Task::TextToMusic | Task::TextToSound) {
            let lengths = sound_lengths(task, m);
            if !lengths.is_empty() {
                let current = self.draft.duration;
                let auto_label = if m.duration_range.is_some() && task == Task::TextToSound { "Auto" } else { "Model's" };
                let row = div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .child(chip("sound-len-auto", auto_label, current.is_none(), cx).on_click(cx.listener(|this, _, _, cx| {
                        this.draft.duration = None;
                        cx.notify();
                    })))
                    .children(lengths.into_iter().enumerate().map(|(i, secs)| {
                        chip(("sound-len", i), crate::views::generate::short_duration(secs).replace(".0s", "s"), current == Some(secs), cx).on_click(cx.listener(move |this, _, _, cx| {
                            this.draft.duration = Some(secs);
                            cx.notify();
                        }))
                    }));
                out.push(section("Length", row.into_any_element(), cx));
            }
        }
        if task == Task::TextToMusic {
            if m.instrumental {
                let this = cx.entity().downgrade();
                out.push(
                    switch(
                        "gen-instrumental",
                        "Instrumental",
                        self.draft.instrumental,
                        move |on, _, cx| {
                            this.update(cx, |p, cx| {
                                p.draft.instrumental = on;
                                cx.notify();
                            })
                            .ok();
                        },
                        cx,
                    )
                    .into_any_element(),
                );
            }
            if !(self.draft.instrumental && m.instrumental) {
                let help = if m.lyrics { "Lyrics" } else { "Lyrics (sent with the prompt)" };
                out.push(section(help, self.lyrics.clone().into_any_element(), cx));
            }
        }
        div().flex().flex_col().gap(px(12.)).children(out).into_any_element()
    }

    fn voice_picker(&mut self, m: &ModelInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let state = self.voices.get(&model_key(m)).cloned();
        let current = self.current_voice(m, cx);
        let (title, sub): (SharedString, Option<String>) = match (&current, &state) {
            (_, Some(VoiceState::Loading)) => ("Loading voices…".into(), None),
            (Some(v), _) => (v.name.clone().into(), voice_line(v)),
            (None, _) => ("The model's voice".into(), None),
        };
        let trigger = div()
            .id("voice-picker")
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(36.))
            .px(px(10.))
            .rounded(px(sz::R_MD))
            .bg(t.bg_sunken.opacity(0.5))
            .border_1()
            .border_color(if self.voice_open { t.accent_ring } else { t.line })
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .tooltip(|_, cx| crate::ui::tooltip("Choose a voice".into(), cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.voice_open = !this.voice_open;
                if this.voice_open {
                    crate::ui::input::focus(&this.voice_query, window, cx);
                }
                cx.notify();
            }))
            .child(icon("mic").text_color(t.text_2))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(title))
                    .when_some(sub, |d, s| d.child(div().truncate().text_size(px(sz::XS)).text_color(t.text_2).child(s))),
            )
            .child(icon("chevron-down").text_color(t.text_2));
        let failed = match &state {
            Some(VoiceState::Failed(e)) => Some(e.clone()),
            _ => None,
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(bounds_probe(self.voice_anchor.clone()))
            .child(trigger)
            .when_some(failed, |d, e| d.child(div().text_size(px(sz::XS)).text_color(t.warning).child(format!("Voices couldn't be listed: {e}"))))
            .when(self.voice_open, |d| {
                d.child(div().absolute().left_0().top(relative(1.)).child(
                    deferred(anchored().snap_to_window_with_margin(px(8.)).child(crate::ui::motion::enter(div().relative().child(self.voice_popover(cx)), "voices-in", crate::ui::motion::FAST, (0., -4.)))).with_priority(3),
                ))
            })
            .into_any_element()
    }

    fn voice_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let anchor = self.voice_anchor.clone();
        let current = self.draft.voice.clone();
        let voices = self.voice_matches(cx);
        let playing = self.sample.clone();
        let rows: Vec<AnyElement> = voices
            .into_iter()
            .map(|v| {
                let on = current.as_deref() == Some(v.id.as_str());
                let id = v.id.clone();
                let line = voice_line(&v);
                let has_sample = v.preview_url.is_some();
                let is_playing = playing.as_deref() == Some(v.id.as_str());
                let v2 = v.clone();
                div()
                    .id(ElementId::Name(format!("voice-{}", v.id).into()))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(sz::R_SM))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.accent_soft))
                    .when(!on, |d| d.hover(|s| s.bg(t.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_voice(Some(id.clone()), cx)))
                    .when(has_sample, |d| {
                        d.child(
                            div()
                                .id(ElementId::Name(format!("voice-play-{}", v.id).into()))
                                .flex_none()
                                .size(px(24.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(if is_playing { t.accent } else { t.hover })
                                .text_color(if is_playing { t.text_on_accent } else { t.text })
                                .tooltip(move |_, cx| crate::ui::tooltip(if is_playing { "Stop".into() } else { "Listen".into() }, cx))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.play_sample(&v2, cx);
                                }))
                                .child(icon(if is_playing { "square" } else { "play" }).size(px(11.))),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.text)
                                    .child(div().truncate().child(v.name.clone()))
                                    .when(v.custom, |d| d.child(div().px(px(4.)).rounded(px(sz::R_XS)).bg(t.hover).text_size(px(9.5)).text_color(t.text_2).child("yours"))),
                            )
                            .when_some(line, |d, l| d.child(div().truncate().text_size(px(sz::XS)).text_color(t.text_2).child(l))),
                    )
                    .when(on, |d| d.child(icon("check").text_color(t.accent_text)))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .id("voice-popover")
            .occlude()
            .mt(px(6.))
            .w(px(340.))
            .max_h(px(380.))
            .flex()
            .flex_col()
            .rounded(px(sz::R_LG))
            .glass(t.glass2)
            .bg(crate::views::generate::popover_fill(cx))
            .shadow(t.glass_shadow())
            .overflow_hidden()
            .text_size(px(sz::BASE))
            .on_mouse_down_out(cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                if anchor.get().is_some_and(|b| b.contains(&e.position)) {
                    return;
                }
                this.voice_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(12.))
                    .py(px(4.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_color(t.text_2)
                    .child(icon("search"))
                    .child(div().flex_1().child(self.voice_query.clone())),
            )
            .child(
                div()
                    .id("voice-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(6.))
                    .when(empty, |d| d.child(div().py(px(14.)).text_center().text_color(t.text_2).child("No voice matches.")))
                    .children(rows),
            )
            .into_any_element()
    }
}

/// Lengths offered for music and sound effects, kept to what the model makes.
fn sound_lengths(task: Task, m: &ModelInfo) -> Vec<f64> {
    if !m.durations.is_empty() {
        return m.durations.clone();
    }
    let Some((lo, hi)) = m.duration_range else { return vec![] };
    let wanted: &[f64] = if task == Task::TextToMusic { &[15.0, 30.0, 60.0, 120.0, 180.0] } else { &[2.0, 5.0, 10.0, 20.0] };
    wanted.iter().copied().filter(|d| *d >= lo && *d <= hi).collect()
}

/// "Warm, British · female · en-GB"
fn voice_line(v: &Voice) -> Option<String> {
    let parts: Vec<&str> = [v.description.as_deref(), v.gender.as_deref(), v.language.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_lines() {
        let v = Voice::new("x", "X").described("Warm").gender("female");
        assert_eq!(voice_line(&v).as_deref(), Some("Warm · female"));
        assert_eq!(voice_line(&Voice::new("x", "X")), None);
    }
}
