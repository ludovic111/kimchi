//! Plugins (lsuite `PLUGINS.md`): one titled area, four parts.
//!
//! - **Stock**: what ships in kimchi, by kind, each with a line on what it does.
//! - **Installed**: lsuite plugins, frei0r filters and sound plugins found on this computer, each
//!   with its format's logo, vendor and version, a switch always in view, and Remove for lsuite
//!   plugins; Rescan.
//! - **Formats**: what kimchi loads, with each format's logo, and where it looks.
//! - **Build with your agent**: one field; sending it starts the Agent panel on the plugin
//!   recipe (`plugin.guide`). Whether Rust is installed, and how to get it.
//!
//! Everything goes through `plugin.*` commands; the lists are `plugin.list` and
//! `plugin.toolchain`'s answers.

use gpui::{AnyElement, App, ClipboardItem, Context, Entity, FontWeight, Render, ScrollHandle, SharedString, Subscription, Window, div, prelude::*, px};
use serde_json::{Value, json};

use crate::store::{Dialog, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, caps, icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Stock,
    Installed,
    Formats,
    Build,
}

impl Part {
    const ALL: [Part; 4] = [Part::Stock, Part::Installed, Part::Formats, Part::Build];

    pub fn parse(s: Option<&str>) -> Option<Part> {
        Some(match s?.to_ascii_lowercase().as_str() {
            "stock" => Part::Stock,
            "installed" | "installed plugins" => Part::Installed,
            "formats" => Part::Formats,
            "build" | "agent" | "new" => Part::Build,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Part::Stock => "Stock",
            Part::Installed => "Installed",
            Part::Formats => "Formats",
            Part::Build => "Build with your agent",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Part::Stock => "package",
            Part::Installed => "plug-zap",
            Part::Formats => "layers",
            Part::Build => "sparkles",
        }
    }
}

pub struct PluginsDialog {
    store: Entity<Store>,
    part: Part,
    open: bool,
    asked: Option<String>,
    list: Option<Value>,
    toolchain: Option<Value>,
    loading: bool,
    prompt: Entity<TextInput>,
    body_scroll: ScrollHandle,
    copied: bool,
    _subs: Vec<Subscription>,
}

/// The prompt "Build with your agent" sends: the person's words and kimchi's recipe.
pub fn recipe_prompt(wish: &str) -> String {
    format!(
        "Make me a kimchi video plugin: {wish}\n\nFollow kimchi's plugin recipe: read plugin.guide and check plugin.toolchain (if Rust is missing, stop and tell me how to install it); plugin.new with a short name and the right kind; write the code with plugin.writeSource; plugin.build until it is green, fixing the errors it reports; plugin.publishLocal; then try it on the selected clip (or a new solid for a generator), render a frame with project.renderFrame, look at it and adjust. Tell me its name when it works."
    )
}

impl PluginsDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let prompt = cx.new(|cx| TextInput::new(cx).multiline(3).placeholder("Describe the plugin you want: “a VHS look with wobbly lines and colour bleed”"));
        let subs = vec![
            cx.observe_in(&store, window, |this: &mut Self, store, _, cx| {
                let dialog = store.read(cx).dialog.clone();
                let (open, asked) = match &dialog {
                    Some(Dialog::Plugins { part }) => (true, part.clone()),
                    _ => (false, None),
                };
                if open && (!this.open || (asked.is_some() && asked != this.asked)) {
                    if let Some(p) = Part::parse(asked.as_deref()) {
                        this.part = p;
                    }
                    this.load(cx);
                }
                this.open = open;
                this.asked = if open { asked } else { None };
                cx.notify();
            }),
            cx.subscribe(&prompt, |this, _, e: &InputEvent, cx| match e {
                InputEvent::Submit => this.build_with_agent(cx),
                _ => cx.notify(),
            }),
        ];
        Self { store, part: Part::Installed, open: false, asked: None, list: None, toolchain: None, loading: false, prompt, body_scroll: ScrollHandle::new(), copied: false, _subs: subs }
    }

    /// Reads the plugins and the toolchain again.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let list = self.store.update(cx, |s, cx| s.call("plugin.list", json!({}), cx));
        let tools = self.store.update(cx, |s, cx| s.call("plugin.toolchain", json!({}), cx));
        cx.spawn(async move |this, cx| {
            let (l, t) = futures::join!(list, tools);
            this.update(cx, |this, cx| {
                this.loading = false;
                if let Ok(v) = l {
                    this.list = Some(v);
                }
                if let Ok(v) = t {
                    this.toolchain = Some(v);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn run(&mut self, name: &str, params: Value, cx: &mut Context<Self>) {
        let task = self.store.update(cx, |s, cx| s.call(name, params, cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                if let Err(e) = r {
                    this.store.update(cx, |s, cx| s.error(e, cx));
                }
                this.load(cx);
            })
            .ok();
        })
        .detach();
    }

    fn rescan(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.run("plugin.rescan", json!({}), cx);
    }

    /// Starts the Agent panel on the plugin recipe, with the person's words. Sending it is the
    /// person's go-ahead, so the plugins permission is switched on first when it is off.
    fn build_with_agent(&mut self, cx: &mut Context<Self>) {
        let wish = self.prompt.read(cx).text().trim().to_string();
        if wish.is_empty() {
            return;
        }
        let allowed = self.store.read(cx).settings.agent.permissions.plugins;
        let prompt = recipe_prompt(&wish);
        self.prompt.update(cx, |i, cx| i.set_text("", cx));
        self.store.update(cx, |s, cx| {
            if !allowed {
                s.run("app.setSetting", json!({ "key": "agent.permissions.plugins", "value": true }), cx);
            }
            s.close_dialog(cx);
            s.set_agent_open(true, cx);
            s.run("agent.send", json!({ "prompt": prompt }), cx);
        });
    }

    fn plugins(&self) -> Vec<Value> {
        self.list.as_ref().and_then(|l| l["plugins"].as_array().cloned()).unwrap_or_default()
    }
}

/// The logo id for a plugin format as `plugin.list` names it.
fn format_logo(format: &str) -> &'static str {
    match format.to_ascii_lowercase().as_str() {
        "lsuite" | "kimchi" => "lsuite",
        "frei0r" => "frei0r",
        "clap" => "clap",
        "vst3" => "vst3",
        "au" => "au",
        "native" | "ryolune" => "ryolune",
        _ => "lut",
    }
}

fn format_label(format: &str) -> String {
    match format.to_ascii_lowercase().as_str() {
        "native" => "ryolune".into(),
        "kimchi" => "kimchi".into(),
        "au" => "Audio Unit".into(),
        f => f.to_string(),
    }
}

/// A switch, boxed, lit when on (always in view, never cut).
fn switch_box(id: SharedString, on: bool, label: String, on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
    let t = cx.theme();
    div()
        .id(id)
        .flex_none()
        .w(px(30.))
        .h(px(18.))
        .p(px(2.))
        .cursor_pointer()
        .role(gpui::Role::Switch)
        .aria_label(SharedString::from(label))
        .bg(if on { t.accent } else { t.line_strong })
        .on_click(move |_, w, cx| on_toggle(!on, w, cx))
        .child(div().size(px(14.)).bg(if on { t.text_on_accent } else { t.text }).when(on, |d| d.ml(px(12.))))
        .into_any_element()
}

/// A section heading in caps running into a hairline, like a drawing.
fn heading(text: &str, count: Option<usize>, cx: &App) -> AnyElement {
    let t = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .pt(px(6.))
        .child(caps(text.to_string(), cx))
        .when_some(count, |d, n| d.child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(n.to_string())))
        .child(div().flex_1().h(px(1.)).bg(t.line))
        .into_any_element()
}

impl PluginsDialog {
    fn row(&self, i: usize, p: &Value, switchable: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let id = p["id"].as_str().unwrap_or("").to_string();
        let name = p["name"].as_str().unwrap_or("").to_string();
        let format = p["format"].as_str().unwrap_or("").to_string();
        let on = p["enabled"].as_bool().unwrap_or(true);
        let failed = p["failed"].as_str().map(str::to_string);
        let removable = p["source"] == "installed" && format == "lsuite";
        let mut meta: Vec<String> = vec![];
        for k in ["vendor", "version"] {
            if let Some(v) = p[k].as_str().filter(|v| !v.is_empty()) {
                meta.push(v.to_string());
            }
        }
        meta.push(format_label(&format));
        meta.push(p["kind"].as_str().unwrap_or("").to_string());
        let desc = p["description"].as_str().unwrap_or("").to_string();
        let toggle_id = id.clone();
        let weak = cx.entity().downgrade();
        div()
            .id(("plugin-row", i))
            .flex()
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .border_b_1()
            .border_color(t.line)
            .when(!on, |d| d.opacity(0.6))
            .child(div().mt(px(1.)).child(crate::ui::logo(format_logo(&format), px(22.))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(name.clone()))
                    .child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(meta.join(" · ")))
                    .when(!desc.is_empty(), |d| d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child(desc)))
                    .when_some(failed, |d, f| d.child(div().flex().gap(px(5.)).text_size(px(sz::SM)).text_color(t.danger).child(icon("circle-alert").mt(px(1.))).child(div().flex_1().min_w_0().child(format!("Switched off after it failed: {f}"))))),
            )
            .when(removable, |d| {
                let rid = id.clone();
                d.child(Button::new(("plugin-remove", i), "Remove").small().ghost().color(t.danger).on_click(cx.listener(move |this, _, _, cx| this.run("plugin.remove", json!({ "id": rid }), cx))))
            })
            .when(switchable, |d| {
                d.child(div().mt(px(2.)).child(switch_box(
                    SharedString::from(format!("plugin-switch-{i}")),
                    on,
                    format!("{name} on"),
                    move |v, _, cx| {
                        let cmd = if v { "plugin.enable" } else { "plugin.disable" };
                        let id = toggle_id.clone();
                        weak.update(cx, |this, cx| this.run(cmd, json!({ "id": id }), cx)).ok();
                    },
                    cx,
                )))
            })
            .into_any_element()
    }

    fn stock(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let all: Vec<Value> = self.plugins().into_iter().filter(|p| p["source"] == "stock").collect();
        let mut col = div().flex().flex_col().gap(px(10.)).child(div().text_size(px(sz::SM)).text_color(t.text_2).child(
            "What ships in kimchi. The effects, transitions and looks are in the inspector; the plugins made with kimchi's SDK show how one is built.",
        ));
        let groups: [(&str, &str); 6] = [("Effects", "effect"), ("Generators", "generator"), ("Transitions", "transition"), ("Looks", "look"), ("Sound effects", "sound"), ("", "")];
        let mut n = 0usize;
        for (title, kind) in groups.iter().filter(|(t, _)| !t.is_empty()) {
            let items: Vec<&Value> = all.iter().filter(|p| p["kind"] == *kind).collect();
            if items.is_empty() {
                continue;
            }
            col = col.child(heading(title, Some(items.len()), cx));
            let rows: Vec<AnyElement> = items
                .iter()
                .map(|p| {
                    n += 1;
                    let sdk = p["id"].as_str().is_some_and(|i| i.starts_with("kimchi:"));
                    if sdk {
                        return self.row(10_000 + n, p, true, cx);
                    }
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(5.))
                        .child(div().w(px(150.)).flex_none().text_size(px(sz::BASE)).text_color(t.text).child(p["name"].as_str().unwrap_or("").to_string()))
                        .child(div().flex_1().min_w_0().text_size(px(sz::SM)).text_color(t.text_2).child(p["description"].as_str().unwrap_or("").to_string()))
                        .into_any_element()
                })
                .collect();
            col = col.child(div().flex().flex_col().children(rows));
        }
        col.into_any_element()
    }

    fn installed(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let all: Vec<Value> = self.plugins().into_iter().filter(|p| p["source"] == "installed").collect();
        let video: Vec<&Value> = all.iter().filter(|p| p["kind"] != "sound").collect();
        let sound: Vec<&Value> = all.iter().filter(|p| p["kind"] == "sound").collect();
        let folder = self.list.as_ref().and_then(|l| l["folder"].as_str()).unwrap_or("~/.lsuite/plugins/kimchi").to_string();
        let failed = self.list.as_ref().and_then(|l| l["failed"].as_array().cloned()).unwrap_or_default();
        let mut col = div().flex().flex_col().gap(px(10.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().min_w_0().text_size(px(sz::SM)).text_color(t.text_2).child("Plugins found on this computer. A switched-off plugin stays installed; clips that use it are drawn without it."))
                .child(
                    Button::new("plugins-rescan", if self.loading { "Looking…" } else { "Rescan" })
                        .small()
                        .with_icon(if self.loading { "loader-circle" } else { "refresh-cw" })
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.rescan(cx))),
                ),
        );
        let mut i = 0usize;
        col = col.child(heading("Video", Some(video.len()), cx));
        if video.is_empty() {
            col = col.child(div().px(px(10.)).text_size(px(sz::SM)).text_color(t.text_2).child(format!("None yet. Build one with your agent, or install frei0r (the frei0r-plugins package). lsuite plugins go in {folder}.")));
        }
        let rows: Vec<AnyElement> = video.iter().map(|p| {
            i += 1;
            self.row(i, p, true, cx)
        }).collect();
        col = col.child(div().flex().flex_col().children(rows));
        if !sound.is_empty() {
            col = col.child(heading("Sound", Some(sound.len()), cx));
            let rows: Vec<AnyElement> = sound.iter().map(|p| {
                i += 1;
                self.row(i, p, true, cx)
            }).collect();
            col = col.child(div().flex().flex_col().children(rows));
        }
        if !failed.is_empty() {
            col = col.child(heading("Didn't load", Some(failed.len()), cx));
            for f in failed.iter().take(12) {
                let path = f["path"].as_str().unwrap_or("").to_string();
                col = col.child(
                    div()
                        .px(px(10.))
                        .flex()
                        .flex_col()
                        .child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(path))
                        .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(f["error"].as_str().unwrap_or("").to_string())),
                );
            }
        }
        col.into_any_element()
    }

    fn formats(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let formats = self.list.as_ref().and_then(|l| l["formats"].as_array().cloned()).unwrap_or_default();
        let cards: Vec<AnyElement> = formats
            .iter()
            .map(|f| {
                let id = f["id"].as_str().unwrap_or("").to_string();
                let found = f["found"].as_u64();
                let folders: Vec<(String, bool)> = f["folders"].as_array().into_iter().flatten().map(|x| (x["path"].as_str().unwrap_or("").to_string(), x["exists"] == true)).collect();
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .p(px(12.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.bg_raised)
                    .child(crate::ui::logo(format_logo(&id), px(30.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child(f["name"].as_str().unwrap_or("").to_string()))
                                    .child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(f["kind"].as_str().unwrap_or("").to_uppercase()))
                                    .child(div().flex_1())
                                    .when_some(found, |d, n| d.child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(format!("{n} found")))),
                            )
                            .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(f["description"].as_str().unwrap_or("").to_string()))
                            .children(folders.into_iter().take(6).map(|(p, exists)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .font_family(MONO)
                                    .text_size(px(sz::XS))
                                    .text_color(if exists { t.text_2 } else { t.text_3 })
                                    .child(icon(if exists { "folder-open" } else { "folder" }).size(px(11.)))
                                    .child(div().min_w_0().truncate().child(p))
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_size(px(sz::SM)).text_color(t.text_2).child("What kimchi loads, and where it looks. Nothing else is listed: these are the formats kimchi really runs."))
            .children(cards)
            .into_any_element()
    }

    fn build(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let allowed = s.settings.agent.permissions.plugins;
        let kind = kimchi_agent::AgentConfig::from_settings(&s.settings.agent).provider;
        let tool = self.toolchain.clone().unwrap_or(Value::Null);
        let rust_ok = tool["ok"] == true;
        let empty = self.prompt.read(cx).text().trim().is_empty();
        let hint = tool["installHint"].as_str().unwrap_or("").to_string();
        let weak = cx.entity().downgrade();
        let rust_line = if self.toolchain.is_none() {
            div().text_size(px(sz::SM)).text_color(t.text_2).child("Checking for Rust…").into_any_element()
        } else if rust_ok {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(sz::SM))
                .child(icon("circle-check").text_color(t.success))
                .child(div().font_family(MONO).text_size(px(sz::XS)).child(tool["version"].as_str().unwrap_or("Rust").to_string()))
                .into_any_element()
        } else {
            let copy = hint.clone();
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().flex().gap(px(6.)).text_size(px(sz::SM)).child(icon("circle-alert").mt(px(1.)).text_color(t.warning)).child("Rust isn't installed. The agent needs it to build plugins: install it with rustup (free, a few minutes), then check again."))
                .child(div().px(px(8.)).py(px(4.)).bg(t.bg_sunken).border_1().border_color(t.line).font_family(MONO).text_size(px(sz::XS)).child(hint))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(Button::new("rust-copy", if self.copied { "Copied" } else { "Copy the command" }).small().with_icon("copy").on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            this.copied = true;
                            cx.notify();
                        })))
                        .child(Button::new("rust-site", "rustup.rs").small().ghost().icon_after("arrow-up-right").on_click(|_, _, cx| cx.open_url("https://rustup.rs")))
                        .child(Button::new("rust-check", "Check again").small().ghost().with_icon("refresh-cw").on_click(cx.listener(|this, _, _, cx| this.load(cx)))),
                )
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(
                "Say what you want. Your agent writes it in Rust with kimchi's plugin SDK, builds it, installs it and tries it on the selected clip: you follow along in the Agent panel, and the plugin shows up in Installed without a restart.",
            ))
            .child(div().p(px(10.)).border_1().border_color(t.line_strong).bg(t.bg_sunken).child(self.prompt.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(crate::ui::logo(kind.id(), px(18.)))
                    .child(div().flex_1().min_w_0().text_size(px(sz::SM)).text_color(t.text_2).child(format!("Runs on {}", kind.label())))
                    .child(Button::new("plugins-agent-settings", "Change").small().ghost().on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("agent".into()) }, cx))))
                    .child(
                        Button::new("plugins-build", if allowed { "Build with your agent" } else { "Allow and build" })
                            .primary()
                            .with_icon("sparkles")
                            .disabled(empty)
                            .tooltip(if allowed { "Starts the Agent panel on kimchi's plugin recipe" } else { "Turns on the agent's Plugins permission, then starts it on kimchi's plugin recipe" })
                            .on_click(cx.listener(|this, _, _, cx| this.build_with_agent(cx))),
                    ),
            )
            .child(heading("Rust", None, cx))
            .child(rust_line)
            .child(heading("Permission", None, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(div().font_weight(FontWeight::MEDIUM).child("Let agents build and install plugins")).child(div().text_size(px(sz::SM)).text_color(t.text_2).child("The agent's Plugins permission (Settings › Agent). Sending a request here turns it on.")))
                    .child(switch_box("plugins-permission".into(), allowed, "Let agents build and install plugins".into(), move |v, _, cx| {
                        weak.update(cx, |this, cx| this.run("app.setSetting", json!({ "key": "agent.permissions.plugins", "value": v }), cx)).ok();
                    }, cx)),
            )
            .into_any_element()
    }
}

impl Render for PluginsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let all = self.plugins();
        let installed = all.iter().filter(|p| p["source"] == "installed").count();
        let stock = all.iter().filter(|p| p["source"] == "stock").count();
        let meta = if self.list.is_none() { "…".to_string() } else { format!("{installed} installed · {stock} stock") };
        let nav: Vec<AnyElement> = Part::ALL
            .iter()
            .map(|&p| {
                let selected = self.part == p;
                div()
                    .id(SharedString::from(format!("plugins-part-{}", p.label())))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .h(px(30.))
                    .px(px(9.))
                    .text_size(px(sz::BASE))
                    .cursor_pointer()
                    .role(gpui::Role::Tab)
                    .aria_label(p.label())
                    .when(selected, |d| d.bg(t.accent).text_color(t.text_on_accent).font_weight(FontWeight::SEMIBOLD))
                    .when(!selected, |d| d.text_color(t.text_2).hover(|s| s.bg(t.hover).text_color(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.part = p;
                        cx.notify();
                    }))
                    .child(icon(p.icon()))
                    .child(div().flex_1().min_w_0().child(p.label()))
                    .into_any_element()
            })
            .collect();
        let body = match self.part {
            Part::Stock => self.stock(cx),
            Part::Installed => self.installed(cx),
            Part::Formats => self.formats(cx),
            Part::Build => self.build(cx),
        };
        div()
            .id("plugins-dialog")
            .key_context("PluginsDialog")
            .role(gpui::Role::Dialog)
            .aria_label("Plugins")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                // Titled like every area: the name, what it holds in mono, its actions.
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(20.))
                    .py(px(14.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(icon("plug-zap"))
                    .child(div().text_size(px(sz::LG)).font_weight(FontWeight::SEMIBOLD).child("Plugins"))
                    .child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(meta))
                    .child(div().flex_1())
                    .child(Button::icon("plugins-x", "x", "Close (Esc)").on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))),
            )
            .child(
                div()
                    .h(px(560.))
                    .min_h_0()
                    .flex_shrink_1()
                    .flex()
                    .child(div().w(px(196.)).flex_none().flex().flex_col().gap(px(2.)).p(px(10.)).border_r_1().border_color(t.line).children(nav))
                    .child(div().id("plugins-body").flex_1().min_w_0().h_full().overflow_y_scroll().track_scroll(&self.body_scroll).p(px(20.)).child(body)),
            )
    }
}
