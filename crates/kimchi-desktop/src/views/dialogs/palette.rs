//! The command palette (⌘K): a fuzzy filter over the window's actions and the menus' actions,
//! recent projects, and, for anything typed, "generate this". Up/Down move, Enter runs, Escape
//! closes. Actions are dispatched as GPUI actions, so they run exactly as the shortcuts do.

use std::rc::Rc;

use gpui::{Action, App, Context, Entity, FontWeight, KeyBinding, Render, ScrollHandle, SharedString, Subscription, Window, actions, div, prelude::*, px};
use serde_json::json;

use crate::actions as act;
use crate::store::{ComposeRequest, Dialog, Store, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::input::{self, InputEvent, TextInput};
use crate::ui::{icon, kbd};

actions!(palette, [SelectPrev, SelectNext]);

type Run = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
struct Cmd {
    label: SharedString,
    /// Extra words that match (not shown).
    keywords: &'static str,
    hint: Option<&'static str>,
    icon: &'static str,
    /// An AI action: drawn with the accent.
    ai: bool,
    run: Run,
}

impl Cmd {
    fn new(label: impl Into<SharedString>, icon: &'static str, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self { label: label.into(), keywords: "", hint: None, icon, ai: false, run: Rc::new(run) }
    }
    fn hint(mut self, h: &'static str) -> Self {
        self.hint = Some(h);
        self
    }
    fn keywords(mut self, k: &'static str) -> Self {
        self.keywords = k;
        self
    }
    fn ai(mut self) -> Self {
        self.ai = true;
        self
    }
    /// Runs a GPUI action, as its shortcut or menu item would.
    fn action(label: &'static str, icon: &'static str, a: impl Action + Clone) -> Self {
        Self::new(label, icon, move |window, cx| window.dispatch_action(a.boxed_clone(), cx))
    }
}

pub struct Palette {
    store: Entity<Store>,
    input: Entity<TextInput>,
    query: String,
    index: usize,
    open: bool,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl Palette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Type a command — or describe something to generate"));
        let mut subs = vec![cx.observe_in(&store, window, |this: &mut Self, store, window, cx| {
            let open = matches!(store.read(cx).dialog, Some(Dialog::Palette));
            if open && !this.open {
                this.query.clear();
                this.index = 0;
                this.input.update(cx, |i, cx| i.set_text("", cx));
                if let Some(q) = std::env::var("KIMCHI_TEMP_QUERY").ok().and_then(|f| std::fs::read_to_string(f).ok()).map(|q| q.trim().to_string()).filter(|q| !q.is_empty()) { this.query = q.clone(); this.input.update(cx, |i, cx| i.set_text(q, cx)); } // TEMP-TEST
                input::focus(&this.input, window, cx);
            }
            this.open = open;
        })];
        subs.push(cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| match e {
            InputEvent::Changed(q) => {
                this.query = q.clone();
                this.index = 0;
                this.scroll.scroll_to_item(0);
                cx.notify();
            }
            InputEvent::Submit(_) => this.run_selected(window, cx),
            InputEvent::Cancel => this.store.update(cx, |s, cx| s.close_dialog(cx)),
            InputEvent::Blur => {}
        }));
        // Up/Down beat the text field's own bindings (same depth, bound later).
        cx.bind_keys([
            KeyBinding::new("up", SelectPrev, Some("Palette > TextInput")),
            KeyBinding::new("down", SelectNext, Some("Palette > TextInput")),
            KeyBinding::new("ctrl-p", SelectPrev, Some("Palette > TextInput")),
            KeyBinding::new("ctrl-n", SelectNext, Some("Palette > TextInput")),
        ]);
        Self { store, input, query: String::new(), index: 0, open: false, scroll: ScrollHandle::new(), _subs: subs }
    }

    /// Everything the palette offers right now (before filtering).
    fn commands(&self, cx: &App) -> Vec<Cmd> {
        let s = self.store.read(cx);
        let has_project = s.project.is_some();
        let has_selection = !s.selection.is_empty();
        let mut v = vec![];
        if has_project {
            v.extend([
                Cmd::new("Generate video…", "film", |_, cx| compose(true, None, cx)).keywords("ai make clip").ai(),
                Cmd::new("Generate image…", "image", |_, cx| compose(false, None, cx)).keywords("ai make picture still").ai(),
                Cmd::action("Import media", "import", act::Import).hint("⌘I").keywords("add files video audio"),
                Cmd::action("Add text", "type", act::AddText).hint("T").keywords("title caption"),
                Cmd::action("Split at playhead", "scissors", act::Split).hint("S").keywords("cut razor"),
                Cmd::action("Undo", "undo-2", act::Undo).hint("⌘Z"),
                Cmd::action("Redo", "redo-2", act::Redo).hint("⇧⌘Z"),
                Cmd::action("Select all clips", "layers", act::SelectAll).hint("⌘A"),
                Cmd::action("Play / pause", "play", act::PlayPause).hint("Space"),
                Cmd::action("Go to start", "skip-back", act::GoToStart).hint("Home"),
                Cmd::action("Go to end", "skip-forward", act::GoToEnd).hint("End"),
                Cmd::action("Add marker at playhead", "map-pin", act::AddMarker).hint("M"),
                Cmd::action("Toggle snapping", "magnet", act::ToggleSnap).hint("N"),
                Cmd::action("Zoom in", "zoom-in", act::ZoomIn).hint("="),
                Cmd::action("Zoom out", "zoom-out", act::ZoomOut).hint("-"),
                Cmd::action("Zoom to fit", "maximize-2", act::ZoomFit).hint("⇧Z"),
                Cmd::new("Add video track", "film", |_, cx| run("track.add", json!({ "kind": "video" }), cx)),
                Cmd::new("Add audio track", "audio-lines", |_, cx| run("track.add", json!({ "kind": "audio" }), cx)),
                Cmd::action("Export…", "share", act::Export).hint("⌘E").keywords("render save file"),
                Cmd::new("Send the cut to ryolune to score", "music", |_, cx| {
                    cx.store().update(cx, |s, cx| s.run_then("handoff.toRyolune", json!({}), cx, |s, _, cx| s.info("Sent to ryolune.", cx)))
                })
                .keywords("music soundtrack lsuite"),
                Cmd::action("Show generation jobs", "sparkles", act::ToggleJobs),
                Cmd::action("Focus the prompt", "wand-sparkles", act::FocusGenerate).hint("⌘G").ai(),
                Cmd::action("Back to projects", "arrow-left", act::CloseProject).hint("⌘W").keywords("home close"),
            ]);
            if has_selection {
                v.extend([
                    Cmd::action("Duplicate selection", "copy", act::Duplicate).hint("⌘D"),
                    Cmd::action("Delete selection", "trash", act::Delete).hint("⌫").keywords("remove"),
                    Cmd::action("Ripple delete selection", "trash", act::RippleDelete).hint("⇧⌫").keywords("remove close gap"),
                ]);
            }
            v.push(Cmd::action("Agent", "bot", act::ToggleAgent).hint("⌘J").keywords("assistant chat ai").ai());
        }
        v.extend([
            Cmd::action("New project", "file-plus", act::NewProject).hint("⌘N"),
            Cmd::action("Settings", "settings", act::OpenSettings).hint("⌘,").keywords("preferences"),
            Cmd::new("Models & keys", "key-round", |_, cx| settings("models", cx)).keywords("api key provider"),
            Cmd::new("Agent settings and permissions", "bot", |_, cx| settings("agent", cx)),
            Cmd::new("Appearance", "sun", |_, cx| settings("appearance", cx)).keywords("theme dark light transparency"),
            Cmd::action("Toggle light / dark", "moon", act::ToggleTheme).keywords("theme appearance"),
            Cmd::action("Check for updates", "refresh-cw", act::CheckUpdates).keywords("version upgrade"),
            Cmd::new("Connect Claude Code or Codex (MCP)", "waypoints", |_, cx| settings("about", cx)).keywords("ai control mcp cli"),
            Cmd::action("Driving kimchi from AI and scripts", "info", act::OpenHelp).keywords("help docs cli mcp"),
            Cmd::action("Support kimchi", "heart", act::OpenSupport).keywords("help sponsor donate"),
            Cmd::action("About kimchi", "info", act::About).keywords("version"),
            Cmd::action("Quit kimchi", "x", act::Quit).hint("⌘Q").keywords("exit"),
        ]);
        // Projects, most recent first (the open one is already on screen).
        let open = s.project.as_ref().map(|p| p.id);
        for p in s.library.iter().filter(|p| Some(p.id) != open) {
            let id = p.id;
            v.push(Cmd::new(format!("Open “{}”", p.name), "folder-open", move |_, cx| run("project.open", json!({ "projectId": id }), cx)).keywords("project recent"));
        }
        v
    }

    /// Matching commands, best first; anything typed can also become a prompt.
    fn results(&self, cx: &App) -> Vec<Cmd> {
        let q = self.query.trim();
        let all = self.commands(cx);
        if q.is_empty() {
            // Projects beyond the five most recent stay reachable by typing.
            let mut projects = 0;
            return all
                .into_iter()
                .filter(|c| {
                    if c.icon != "folder-open" {
                        return true;
                    }
                    projects += 1;
                    projects <= 5
                })
                .collect();
        }
        let mut scored: Vec<(i32, usize, Cmd)> = all
            .into_iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let a = fuzzy(q, &c.label);
                let b = fuzzy(q, c.keywords).map(|s| s - 40);
                a.max(b).map(|s| (s, i, c))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut out: Vec<Cmd> = scored.into_iter().map(|(_, _, c)| c).collect();
        let prompt = q.to_string();
        if self.store.read(cx).project.is_some() {
            let (p1, p2) = (prompt.clone(), prompt.clone());
            out.push(Cmd::new(format!("Generate video: “{prompt}”"), "sparkles", move |_, cx| generate(true, &p1, cx)).ai());
            out.push(Cmd::new(format!("Generate image: “{prompt}”"), "sparkles", move |_, cx| generate(false, &p2, cx)).ai());
        } else {
            out.push(Cmd::new(format!("New project from “{prompt}”"), "sparkles", move |_, cx| new_project_from(&prompt, cx)).ai());
        }
        out
    }

    fn run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let results = self.results(cx);
        if let Some(c) = results.get(self.index.min(results.len().saturating_sub(1))) {
            self.run_cmd(c.run.clone(), window, cx);
        }
    }

    /// Runs a command, then closes. Actions are dispatched first, while the palette's
    /// field still has focus inside the workspace, so they reach its handlers.
    fn run_cmd(&mut self, run: Run, window: &mut Window, cx: &mut Context<Self>) {
        run(window, cx);
        self.query.clear();
        self.input.update(cx, |i, cx| i.set_text("", cx));
        self.store.update(cx, |s, cx| if s.dialog == Some(Dialog::Palette) { s.close_dialog(cx) });
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.index = self.index.saturating_sub(1);
        self.scroll.scroll_to_item(self.index);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.results(cx).len();
        self.index = (self.index + 1).min(n.saturating_sub(1));
        self.scroll.scroll_to_item(self.index);
        cx.notify();
    }
}

impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let results = self.results(cx);
        let index = self.index.min(results.len().saturating_sub(1));
        let rows: Vec<_> = results
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let on = i == index;
                let color = if c.ai { t.accent_text } else if on { t.text } else { t.text_2 };
                let run = c.run.clone();
                div()
                    .id(("palette-row", i))
                    .flex()
                    .items_center()
                    .gap(px(11.))
                    .h(px(36.))
                    .px(px(10.))
                    .rounded(px(sz::R_SM))
                    .cursor_pointer()
                    .role(gpui::Role::ListBoxOption)
                    .aria_label(c.label.clone())
                    .text_color(if on { t.text } else { t.text_2 })
                    .when(on, |d| d.bg(t.accent_soft))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.index != i {
                            this.index = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.run_cmd(run.clone(), window, cx)))
                    .child(icon(c.icon).size(px(15.)).text_color(color))
                    .child(div().flex_1().min_w_0().truncate().when(c.ai && on, |d| d.font_weight(FontWeight::SEMIBOLD)).child(c.label))
                    .when_some(c.hint, |d, h| d.child(kbd(keys(h), cx)))
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .id("palette")
            .key_context("Palette")
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .role(gpui::Role::Dialog)
            .aria_label("Command palette")
            .flex()
            .flex_col()
            .max_h(px(440.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_color(t.text_2)
                    .child(icon("search").size(px(16.)).text_color(t.text_2))
                    .child(div().flex_1().text_size(px(sz::MD)).child(self.input.clone()))
                    .child(kbd("esc", cx)),
            )
            .child(
                div()
                    .id("palette-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(px(6.))
                    .role(gpui::Role::ListBox)
                    .children(rows)
                    .when(empty, |d| d.child(div().p(px(12.)).text_color(t.text_2).child("Nothing matches."))),
            )
    }
}

/// Fuzzy match: every character of `q` in order. Higher is better: contiguous runs, word
/// starts and an early first match score more; `None` when it doesn't match.
fn fuzzy(q: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = q.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let lower = text.to_lowercase();
    // A plain substring match beats any scattered one.
    let needle: String = q.iter().collect();
    if let Some(pos) = lower.replace(' ', "").find(&needle).or_else(|| lower.find(&needle)) {
        return Some(1000 - pos as i32);
    }
    let (mut qi, mut score, mut prev) = (0, 0, None::<usize>);
    for (i, c) in t.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            let word_start = i == 0 || !t[i - 1].is_alphanumeric();
            score += 10 + if word_start { 15 } else { 0 } + if prev == Some(i.wrapping_sub(1)) { 12 } else { 0 };
            if qi == 0 {
                score -= i as i32;
            }
            prev = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score)
}

/// Shortcut hints as this platform writes them.
fn keys(h: &'static str) -> SharedString {
    if cfg!(target_os = "macos") {
        return h.into();
    }
    h.replace("⇧⌘", "Ctrl+Shift+").replace('⌘', "Ctrl+").replace('⇧', "Shift+").replace('⌫', "Backspace").into()
}

fn run(name: &'static str, params: serde_json::Value, cx: &mut App) {
    cx.store().update(cx, |s, cx| s.run(name, params, cx));
}

fn settings(section: &str, cx: &mut App) {
    let section = section.to_string();
    // After the palette closes.
    cx.defer(move |cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some(section) }, cx)));
}

fn compose(video: bool, prompt: Option<String>, cx: &mut App) {
    cx.store().update(cx, |s, cx| s.compose(ComposeRequest { video, prompt, ..Default::default() }, cx));
}

/// Generates right away when a provider is ready (at the playhead, the model from the
/// settings or the first featured one); otherwise opens the composer with the prompt.
fn generate(video: bool, prompt: &str, cx: &mut App) {
    let ready = cx.store().read(cx).providers.iter().any(|p| p.ready);
    if !ready {
        compose(video, Some(prompt.to_string()), cx);
        return;
    }
    let params = json!({ "prompt": prompt, "video": video });
    let prompt = prompt.to_string();
    cx.store().update(cx, |s, cx| {
        let task = s.call("generate.submit", params, cx);
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |s, cx| match r {
                Ok(_) => s.info(if video { "Generating a video…" } else { "Generating an image…" }, cx),
                // No model can do it as asked: hand the prompt to the composer instead.
                Err(e) => {
                    s.error(e, cx);
                    s.compose(ComposeRequest { video, prompt: Some(prompt), ..Default::default() }, cx);
                }
            })
            .ok();
        })
        .detach();
    });
}

/// From home: a project named after the prompt, with the prompt in the composer.
fn new_project_from(prompt: &str, cx: &mut App) {
    let name = crate::views::home::title_from(prompt);
    let prompt = prompt.to_string();
    cx.store().update(cx, |s, cx| {
        s.run_then("project.create", json!({ "name": name }), cx, move |s, _, cx| {
            s.compose(ComposeRequest { video: true, prompt: Some(prompt), ..Default::default() }, cx);
        })
    });
}
