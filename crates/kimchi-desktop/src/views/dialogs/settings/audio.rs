//! Settings › Audio: the speakers and microphone, the loudness clips are normalised to, the
//! timeline's sound habits (hear while scrubbing, snap to beats, the voice-over count-in),
//! plugin folders and a rescan (with how many effects kimchi knows), and the link with
//! ryolune (installed, running, its version, songs refreshed when saved). Every change is
//! `app.setSetting` (`audio.*`) or `audio.rescanPlugins`; devices and apps are read with
//! `audio.devices` and `handoff.apps`.

use gpui::{AnyElement, Context, FontWeight, PathPromptOptions, SharedString, div, prelude::*, px};
use serde_json::{Value, json};

use super::{SettingsDialog, group, note, toggle_row};
use crate::store::{MenuItem, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, icon, segmented};

#[derive(Default)]
pub(super) struct AudioState {
    devices: Option<Value>,
    ryolune: Option<Value>,
    effects: Option<usize>,
    scanning: bool,
}

impl SettingsDialog {
    /// Reads the devices, the effect count and ryolune's entry (on showing the section).
    pub(super) fn load_audio(&mut self, cx: &mut Context<Self>) {
        let settings = self.store.read(cx).settings.clone();
        self.audio.devices = Some(crate::views::mixer::devices_json(&settings));
        self.audio.effects = Some(kimchi_audio::plugins::effects(None).len());
        self.run("handoff.apps", json!({}), cx, |this, v, cx| {
            this.audio.ryolune = v.as_array().and_then(|a| a.iter().find(|x| x["app"] == "ryolune").cloned()).or(Some(Value::Null));
            cx.notify();
        });
        cx.notify();
    }

    fn device_button(&self, id: &'static str, key: &'static str, current: &str, list: Vec<String>, default: Option<String>, _cx: &mut Context<Self>) -> AnyElement {
        let label = if current.is_empty() { format!("System default{}", default.map(|d| format!(" ({d})")).unwrap_or_default()) } else { current.to_string() };
        Button::new(id, label)
            .small()
            .icon_after("chevron-down")
            .on_click(move |e, _, cx| {
                let mut entries = vec![MenuItem::new("System default", move |_, cx| crate::views::mixer::run_cmd(cx, "app.setSetting", json!({ "key": key, "value": "" }))).entry()];
                for name in &list {
                    let n = name.clone();
                    entries.push(MenuItem::new(name.clone(), move |_, cx| crate::views::mixer::run_cmd(cx, "app.setSetting", json!({ "key": key, "value": n.clone() }))).entry());
                }
                let pos = e.position();
                cx.store().update(cx, |s, cx| s.open_menu(pos, entries, cx));
            })
            .into_any_element()
    }

    fn set_folders(&self, folders: Vec<String>, cx: &mut Context<Self>) {
        self.set_setting("audio.pluginFolders", json!(folders), cx);
    }

    pub(super) fn audio_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let a = self.store.read(cx).settings.audio.clone();
        let names = |key: &str, v: &Option<Value>| -> Vec<String> { v.as_ref().and_then(|d| d[key].as_array()).map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() };
        let outputs = names("outputs", &self.audio.devices);
        let inputs = names("inputs", &self.audio.devices);
        let default_out = self.audio.devices.as_ref().and_then(|d| d["defaultOutput"].as_str().map(str::to_string));
        let default_in = self.audio.devices.as_ref().and_then(|d| d["defaultInput"].as_str().map(str::to_string));
        let out_btn = self.device_button("audio-output", "audio.outputDevice", &a.output_device, outputs, default_out, cx);
        let in_btn = self.device_button("audio-input", "audio.inputDevice", &a.input_device, inputs, default_in, cx);
        let weak = cx.entity().downgrade();
        let loudness = segmented(
            "default-loudness",
            vec![(-14i64, "−14 YouTube".into()), (-16i64, "−16 Podcast".into()), (-23i64, "−23 Broadcast".into())],
            a.default_loudness.round() as i64,
            move |v, _, cx| {
                let v = *v;
                weak.update(cx, |this, cx| this.set_setting("audio.defaultLoudness", json!(v), cx)).ok();
            },
            cx,
        );
        let weak = cx.entity().downgrade();
        let count_in = segmented(
            "count-in",
            vec![(0i64, "None".into()), (1i64, "1 s".into()), (2i64, "2 s".into()), (3i64, "3 s".into()), (4i64, "4 s".into())],
            a.count_in.round() as i64,
            move |v, _, cx| {
                let v = *v;
                weak.update(cx, |this, cx| this.set_setting("audio.countIn", json!(v), cx)).ok();
            },
            cx,
        );
        let toggle = |id: &'static str, key: &'static str, title: &str, desc: &str, on: bool, cx: &mut Context<Self>| {
            let weak = cx.entity().downgrade();
            toggle_row(id, title, desc, on, true, move |v, _, cx| {
                weak.update(cx, |this, cx| this.set_setting(key, json!(v), cx)).ok();
            }, cx)
        };
        // Plugin folders.
        let folders = a.plugin_folders.clone();
        let rows = folders.iter().enumerate().map(|(i, f)| {
            let rest: Vec<String> = folders.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, x)| x.clone()).collect();
            let weak = cx.entity().downgrade();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(8.))
                .rounded(px(sz::R_SM))
                .bg(t.bg_sunken)
                .child(icon("folder").text_color(t.text_3))
                .child(div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(sz::SM)).child(f.clone()))
                .child(Button::icon(SharedString::from(format!("remove-folder-{i}")), "x", "Remove").small().on_click(move |_, _, cx| {
                    let rest = rest.clone();
                    weak.update(cx, |this, cx| this.set_folders(rest, cx)).ok();
                }))
        }).collect::<Vec<_>>();
        let weak = cx.entity().downgrade();
        let add_folder = Button::new("add-plugin-folder", "Add a folder…").small().with_icon("plus").on_click(move |_, _, cx| {
            let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some("Add".into()) });
            let weak = weak.clone();
            cx.spawn(async move |cx| {
                let Ok(Ok(Some(paths))) = rx.await else { return };
                weak.update(cx, |this, cx| {
                    let mut folders = this.store.read(cx).settings.audio.plugin_folders.clone();
                    for p in paths {
                        let p = p.to_string_lossy().into_owned();
                        if !folders.contains(&p) {
                            folders.push(p);
                        }
                    }
                    this.set_folders(folders, cx);
                })
                .ok();
            })
            .detach();
        });
        let scanning = self.audio.scanning;
        let rescan = Button::new("rescan-plugins-settings", if scanning { "Scanning…" } else { "Rescan plugins" }).small().with_icon("refresh-cw").disabled(scanning).on_click(cx.listener(|this, _, _, cx| {
            this.audio.scanning = true;
            cx.notify();
            this.run("audio.rescanPlugins", json!({}), cx, |this, v, cx| {
                this.audio.scanning = false;
                this.audio.effects = v["effects"].as_u64().map(|n| n as usize);
                cx.notify();
            });
        }));
        // ryolune.
        let ryolune: AnyElement = match &self.audio.ryolune {
            None => div().text_size(px(sz::SM)).text_color(t.text_3).child("Looking for ryolune…").into_any_element(),
            Some(Value::Null) => note("circle-alert", "ryolune isn't installed on this computer. Songs still import (kimchi plays them with ryolune's own engine); opening one in ryolune needs the app: lsuite.xyz/ryolune.", t.text_2, cx),
            Some(v) => {
                let running = v["isRunning"].as_bool().unwrap_or(false);
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(sz::R_MD))
                    .bg(t.bg_sunken)
                    .border_1()
                    .border_color(t.line)
                    .child(crate::views::mixer::ryolune_mark(22., cx))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("ryolune {}", v["version"].as_str().unwrap_or(""))))
                            .child(div().text_size(px(sz::SM)).text_color(if running { t.success } else { t.text_2 }).child(if running { "Running: hand-offs and \u{201c}Open in ryolune\u{201d} go straight to it." } else { "Installed, not running: kimchi starts it when you open a song." })),
                    )
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(group("Output", Some("Where the preview plays."), div().flex().child(out_btn).into_any_element(), cx))
            .child(group("Input", Some("The microphone voice-overs are recorded from."), div().flex().child(in_btn).into_any_element(), cx))
            .child(group("Loudness", Some("What Normalize brings clips to, in LUFS (speech: −16)."), div().max_w(px(380.)).child(loudness).into_any_element(), cx))
            .child(toggle("audio-scrub", "audio.scrub", "Hear while scrubbing", "Dragging the playhead plays a little of the sound under it.", a.scrub, cx))
            .child(toggle("audio-snap-beats", "audio.snapToBeats", "Snap to beats", "Clips, edges and the playhead also stick to the beats of music on the timeline.", a.snap_to_beats, cx))
            .child(group("Voice-over count-in", Some("Seconds counted down before a take starts recording."), div().max_w(px(320.)).child(count_in).into_any_element(), cx))
            .child(group(
                "Plugins",
                Some("kimchi uses ryolune's stock effects and the CLAP, VST3, Audio Unit and ryolune plugins in the standard folders, plus these."),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(rows)
                    .child(div().flex().items_center().gap(px(8.)).child(add_folder).child(rescan).child(div().text_size(px(sz::SM)).text_color(t.text_2).child(self.audio.effects.map(|n| format!("{n} effects")).unwrap_or_default())))
                    .into_any_element(),
                cx,
            ))
            .child(group("ryolune", None, ryolune, cx))
            .child(toggle("audio-refresh-songs", "audio.refreshSongs", "Keep songs up to date", "When kimchi comes back to the front, songs saved in ryolune since are rendered again.", a.refresh_songs, cx))
            .into_any_element()
    }
}
