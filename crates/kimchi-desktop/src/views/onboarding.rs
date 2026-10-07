//! The first-run setup: shown over the whole window the first time kimchi starts (before Home),
//! and from Help › Set up kimchi… or `ui.showPanel onboarding`. A few calm steps, each one
//! skippable: the appearance, the editor the person comes from (its keys, projects, looks),
//! generative AI (and connecting a provider), the built-in assistant, then a start.
//!
//! It reads what is on this computer with `app.onboarding` and changes things only through
//! commands: `app.setSetting`, `project.importFrom`, `looks.import`, `generate.setKey`,
//! `generate.check`, `app.setAgentKey`, `agent.setProvider`, and `app.finishOnboarding` at the
//! end (or on "Skip setup") with the answers.
//!
//! Keys: Enter goes on, Esc goes back, Tab picks the next choice of a step.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, FontWeight, KeyBinding, MouseButton, PathPromptOptions, Render, ScrollHandle, SharedString,
    Subscription, Window, actions, div, prelude::*, px,
};
use kimchi_control::Settings;
use serde_json::{Value, json};

use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::TextInput;
use crate::ui::{Button, GlassExt, caps, icon, layout, motion, segmented, switch};

actions!(onboarding, [Next, Back, NextChoice]);

/// Whether the setup opens by itself at start: never set up (a settings file from before 0.9
/// counts as set up), and not turned off for this run (`KIMCHI_NO_ONBOARDING`, for scripts and
/// screenshots).
pub fn should_show(settings: &Settings) -> bool {
    !settings.onboarding.is_done() && std::env::var("KIMCHI_NO_ONBOARDING").is_err()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
    ComingFrom,
    /// Bring your work from the app chosen.
    Bring,
    Generative,
    /// Connect your provider (after "Yes" to generative AI).
    Connect,
    Agent,
    Done,
}

impl Step {
    pub fn id(self) -> &'static str {
        match self {
            Step::Welcome => "welcome",
            Step::ComingFrom => "comingFrom",
            Step::Bring => "bring",
            Step::Generative => "generativeAi",
            Step::Connect => "connect",
            Step::Agent => "agent",
            Step::Done => "done",
        }
    }

    fn parse(s: &str) -> Option<Step> {
        [Step::Welcome, Step::ComingFrom, Step::Bring, Step::Generative, Step::Connect, Step::Agent, Step::Done].into_iter().find(|st| st.id().eq_ignore_ascii_case(s.trim()))
    }

    /// Its progress dot (the follow-up steps share their question's).
    fn dot(self) -> usize {
        match self {
            Step::Welcome => 0,
            Step::ComingFrom | Step::Bring => 1,
            Step::Generative | Step::Connect => 2,
            Step::Agent => 3,
            Step::Done => 4,
        }
    }
}

const DOTS: usize = 5;

/// What "Where are you coming from?" offers besides the apps.
const NOT_AN_APP: [(&str, &str, &str); 2] = [("new", "New to editing", "Start with kimchi's own keys."), ("other", "Something else", "Another editor, or a mix.")];

/// The agent's API providers offered with a key field.
const AGENT_KEYS: [(&str, &str, &str); 4] = [
    ("anthropic", "Anthropic (Claude)", "https://console.anthropic.com/settings/keys"),
    ("openai", "OpenAI", "https://platform.openai.com/api-keys"),
    ("gemini", "Google Gemini", "https://aistudio.google.com/apikey"),
    ("openrouter", "OpenRouter", "https://openrouter.ai/keys"),
];

pub struct Onboarding {
    store: Entity<Store>,
    focus: FocusHandle,
    open: bool,
    pub step: Step,
    /// `app.onboarding`'s answer.
    info: Option<Value>,
    pub coming_from: Option<String>,
    /// Use the app's keyboard layout (on by default).
    pub use_keys: bool,
    pub generative: Option<bool>,
    /// Some(false): no assistant; Some(true): one was chosen.
    pub agent: Option<bool>,
    agent_choice: Option<String>,
    /// Key fields by `gen:<id>` / `agent:<id>`.
    keys: HashMap<String, Entity<TextInput>>,
    /// Results of checks: (ok, what to say).
    checks: HashMap<String, (bool, String)>,
    busy: HashSet<String>,
    imported: Option<String>,
    looks_note: Option<String>,
    show_all: bool,
    copied: bool,
    scroll: ScrollHandle,
    /// lsuite AI's account card (the first choice of the assistant step).
    lsuite: Entity<crate::views::lsuite::LsuiteCard>,
    _subs: Vec<Subscription>,
}

impl Onboarding {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let subs = vec![cx.observe_in(&store, window, |this: &mut Self, store, window, cx| {
            let asked = store.read(cx).setup.clone();
            match asked {
                Some(step) if !this.open => {
                    this.open = true;
                    this.reset(Step::parse(&step).unwrap_or(Step::Welcome), cx);
                    window.focus(&this.focus, cx);
                }
                Some(step) => {
                    // Asked for another step while open (`ui.showPanel onboarding section`).
                    if let Some(st) = Step::parse(&step).filter(|st| *st != this.step && !step.is_empty()) {
                        this.go(st, cx);
                        store.update(cx, |s, _| s.setup = Some(String::new()));
                    }
                }
                None => this.open = false,
            }
            cx.notify();
        })];
        cx.bind_keys([
            KeyBinding::new("enter", Next, Some("Onboarding && !TextInput")),
            KeyBinding::new("escape", Back, Some("Onboarding && !TextInput")),
            KeyBinding::new("tab", NextChoice, Some("Onboarding && !TextInput")),
        ]);
        Self {
            store,
            focus: cx.focus_handle(),
            open: false,
            step: Step::Welcome,
            info: None,
            coming_from: None,
            use_keys: true,
            generative: None,
            agent: None,
            agent_choice: None,
            keys: HashMap::new(),
            checks: HashMap::new(),
            busy: HashSet::new(),
            imported: None,
            looks_note: None,
            show_all: false,
            copied: false,
            scroll: ScrollHandle::new(),
            lsuite: cx.new(|cx| crate::views::lsuite::LsuiteCard::new(window, cx)),
            _subs: subs,
        }
    }

    fn reset(&mut self, step: Step, cx: &mut Context<Self>) {
        let s = self.store.read(cx).settings.clone();
        self.step = step;
        self.coming_from = Some(s.onboarding.coming_from.clone()).filter(|c| !c.is_empty());
        self.use_keys = true;
        self.generative = None;
        self.agent = None;
        self.agent_choice = None;
        self.checks.clear();
        self.imported = None;
        self.looks_note = None;
        self.show_all = false;
        self.load(cx);
    }

    /// Reads what is on this computer (`app.onboarding`).
    fn load(&mut self, cx: &mut Context<Self>) {
        let task = self.store.update(cx, |s, cx| s.call("app.onboarding", json!({}), cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                match r {
                    Ok(v) => this.info = Some(v),
                    Err(e) => this.store.update(cx, |s, cx| s.error(e, cx)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    #[cfg(test)]
    pub fn info(&self) -> Option<&Value> {
        self.info.as_ref()
    }

    /// Runs a command; `then` gets the result (errors become toasts and also reach `then`).
    fn run(&self, name: &str, params: Value, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Result<Value, String>, &mut Context<Self>) + 'static) {
        let task = self.store.update(cx, |s, cx| s.call(name, params, cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                if let Err(e) = &r {
                    this.store.update(cx, |s, cx| s.error(e.clone(), cx));
                }
                then(this, r, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn app(&self) -> Option<Value> {
        let id = self.coming_from.as_deref()?;
        self.info.as_ref()?["apps"].as_array()?.iter().find(|a| a["id"] == id).cloned()
    }

    pub fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.step = step;
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        cx.notify();
    }

    /// The step after this one, given the answers.
    fn after(&self) -> Option<Step> {
        Some(match self.step {
            Step::Welcome => Step::ComingFrom,
            Step::ComingFrom if self.app().is_some() => Step::Bring,
            Step::ComingFrom | Step::Bring => Step::Generative,
            Step::Generative if self.generative == Some(true) => Step::Connect,
            Step::Generative | Step::Connect => Step::Agent,
            Step::Agent => Step::Done,
            Step::Done => return None,
        })
    }

    fn before(&self) -> Option<Step> {
        Some(match self.step {
            Step::Welcome => return None,
            Step::ComingFrom => Step::Welcome,
            Step::Bring => Step::ComingFrom,
            Step::Generative if self.app().is_some() => Step::Bring,
            Step::Generative => Step::ComingFrom,
            Step::Connect => Step::Generative,
            Step::Agent if self.generative == Some(true) => Step::Connect,
            Step::Agent => Step::Generative,
            Step::Done => Step::Agent,
        })
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        match self.after() {
            Some(st) => self.go(st, cx),
            None => self.finish(false, true, cx),
        }
    }

    fn on_next(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        self.next(cx);
    }

    fn on_back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(st) = self.before() {
            self.go(st, cx);
        }
    }

    /// Tab: the next choice of a question step.
    fn on_next_choice(&mut self, _: &NextChoice, _: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::ComingFrom => {
                let mut ids: Vec<String> = self.editors().iter().filter_map(|a| a["id"].as_str().map(str::to_string)).collect();
                ids.extend(NOT_AN_APP.iter().map(|(id, _, _)| id.to_string()));
                let at = self.coming_from.as_ref().and_then(|c| ids.iter().position(|i| i == c)).map(|i| i + 1).unwrap_or(0);
                self.coming_from = ids.get(at % ids.len().max(1)).cloned();
            }
            Step::Generative => self.generative = Some(self.generative != Some(true)),
            _ => {}
        }
        cx.notify();
    }

    /// The answers: `app.finishOnboarding`, then the setup closes (`new`: into a new project).
    pub fn finish(&mut self, skipped: bool, new_project: bool, cx: &mut Context<Self>) {
        let mut params = json!({ "skipped": skipped });
        if let Some(c) = &self.coming_from {
            params["comingFrom"] = json!(c);
            if let Some(app) = self.app()
                && let Some(k) = app["keymap"].as_str()
            {
                params["keymap"] = json!(if self.use_keys { k } else { "kimchi" });
            }
        }
        if let Some(on) = self.generative {
            params["generativeAi"] = json!(on);
        }
        if let Some(on) = self.agent {
            params["agent"] = json!(on);
        }
        self.run("app.finishOnboarding", params, cx, move |this, r, cx| {
            if r.is_err() {
                return;
            }
            this.store.update(cx, |s, cx| {
                s.close_setup(cx);
                if new_project && s.project.is_none() {
                    s.run("project.create", json!({ "name": "Untitled", "width": 1920, "height": 1080, "fps": 30 }), cx);
                }
            });
        });
    }

    fn editors(&self) -> Vec<Value> {
        self.info.as_ref().and_then(|v| v["apps"].as_array()).map(|a| a.iter().filter(|a| a["kind"] == "editor").cloned().collect()).unwrap_or_default()
    }

    fn key_field(&mut self, id: &str, hint: &str, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(f) = self.keys.get(id) {
            return f.clone();
        }
        let hint = hint.to_string();
        let f = cx.new(|cx| {
            let mut i = TextInput::new(cx).placeholder(if hint.is_empty() { "Paste the key".to_string() } else { hint });
            i.mono = true;
            i
        });
        self.keys.insert(id.to_string(), f.clone());
        f
    }

    // ---- actions of the steps ---------------------------------------------------------

    fn import_project(&mut self, app: Value, cx: &mut Context<Self>) {
        let name = app["name"].as_str().unwrap_or("the app").to_string();
        let folders = app["opens"].as_array().is_some_and(|o| o.iter().any(|f| f == "capcut"));
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: folders, multiple: false, prompt: Some(format!("Open from {name}").into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update(cx, |this, cx| {
                this.busy.insert("import".into());
                cx.notify();
                this.run("project.importFrom", json!({ "path": path }), cx, move |this, r, _| {
                    this.busy.remove("import");
                    if let Ok(v) = r {
                        let project = v["name"].as_str().or_else(|| v["project"]["name"].as_str()).map(str::to_string);
                        let missing = v["report"]["missingMedia"].as_array().or_else(|| v["missingMedia"].as_array()).map(Vec::len).unwrap_or(0);
                        let mut note = format!("Opened “{}”.", project.clone().unwrap_or_else(|| "the project".into()));
                        if missing > 0 {
                            note.push_str(&format!(" {missing} media file{} aren't where it says: find them from the media panel.", if missing == 1 { "" } else { "s" }));
                        }
                        this.imported = Some(project.unwrap_or_else(|| "the imported project".into()));
                        this.checks.insert("import".into(), (true, note));
                    }
                });
            })
            .ok();
        })
        .detach();
    }

    fn import_looks(&mut self, app: &Value, cx: &mut Context<Self>) {
        let paths: Vec<Value> = app["lookFolders"].as_array().into_iter().flatten().map(|f| f["path"].clone()).collect();
        let folder = app["name"].as_str().unwrap_or("Imported").to_string();
        self.busy.insert("looks".into());
        self.run("looks.import", json!({ "paths": paths, "folder": folder }), cx, |this, r, _| {
            this.busy.remove("looks");
            if let Ok(v) = r {
                let n = v["added"].as_array().map(Vec::len).unwrap_or(0);
                this.looks_note = Some(format!("Added {n} look{} to the library: use them from the inspector's Colour section.", if n == 1 { "" } else { "s" }));
            }
        });
    }

    /// Saves a provider's key (when one was typed) and checks that the provider answers.
    fn check_provider(&mut self, id: String, cx: &mut Context<Self>) {
        let key = self.keys.get(&format!("gen:{id}")).map(|f| f.read(cx).text().trim().to_string()).unwrap_or_default();
        self.busy.insert(format!("gen:{id}"));
        cx.notify();
        if key.is_empty() {
            self.check_only(id, cx);
            return;
        }
        let pid = id.clone();
        self.run("generate.setKey", json!({ "provider": id, "key": key }), cx, move |this, r, cx| {
            if r.is_ok() {
                this.check_only(pid, cx);
            } else {
                this.busy.remove(&format!("gen:{pid}"));
            }
        });
    }

    fn check_only(&mut self, id: String, cx: &mut Context<Self>) {
        self.run("generate.check", json!({ "provider": &id }), cx, move |this, r, _| {
            this.busy.remove(&format!("gen:{id}"));
            let said = match r {
                Ok(v) => (v["ok"].as_bool().unwrap_or(true), v["message"].as_str().unwrap_or("It answers: ready to generate.").to_string()),
                Err(e) => (false, e),
            };
            this.checks.insert(format!("gen:{id}"), said);
        });
    }

    /// Runs the assistant on `provider`, saving its key first when one is given.
    fn use_agent(&mut self, provider: String, key: Option<String>, cx: &mut Context<Self>) {
        match key.filter(|k| !k.trim().is_empty()) {
            Some(k) => {
                let p = provider.clone();
                self.run("app.setAgentKey", json!({ "provider": provider, "key": k }), cx, move |this, r, cx| {
                    if r.is_ok() {
                        this.set_agent(p, cx);
                    }
                });
            }
            None => self.set_agent(provider, cx),
        }
    }

    fn set_agent(&mut self, provider: String, cx: &mut Context<Self>) {
        self.run("agent.setProvider", json!({ "provider": &provider }), cx, move |this, r, _| {
            if r.is_ok() {
                this.agent = Some(true);
                this.agent_choice = Some(provider);
            }
        });
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = true;
        cx.notify();
    }
}

// ---- drawing ---------------------------------------------------------------------------

/// A step's heading and one line under it.
fn heading(title: impl Into<SharedString>, sub: impl Into<SharedString>, cx: &App) -> AnyElement {
    let t = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_size(px(sz::XL)).font_weight(FontWeight::BOLD).text_color(t.text).child(title.into()))
        .child(div().text_size(px(sz::BASE)).text_color(t.text_2).child(sub.into()))
        .into_any_element()
}

/// An app's initials on a quiet tile, for the apps kimchi has no logo of (it only shows the logos
/// of apps it really opens projects, looks or keys from).
fn monogram(name: &str, cx: &App) -> AnyElement {
    let t = cx.theme();
    let letters: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
    div()
        .flex_none()
        .size(px(34.))
        .rounded(px(sz::R_MD))
        .bg(t.bg_sunken)
        .border_1()
        .border_color(t.line)
        .flex()
        .items_center()
        .justify_center()
        .font_family(MONO)
        .text_size(px(sz::SM))
        .text_color(t.text_2)
        .child(letters)
        .into_any_element()
}

/// A small coloured label ("Found on this computer").
fn badge(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let t = cx.theme();
    div().flex_none().px(px(6.)).py(px(1.)).rounded(px(sz::R_XS)).bg(t.accent_soft).text_color(t.accent_text).text_size(px(sz::XS)).child(text.into()).into_any_element()
}

/// A note in the step's colour for what happened.
fn said(ok: bool, text: &str, cx: &App) -> AnyElement {
    let t = cx.theme();
    let c = if ok { t.success } else { t.danger };
    div()
        .flex()
        .items_start()
        .gap(px(6.))
        .text_size(px(sz::SM))
        .text_color(c)
        .child(icon(if ok { "circle-check" } else { "circle-alert" }).mt(px(1.)).text_color(c))
        .child(div().flex_1().min_w_0().child(text.to_string()))
        .into_any_element()
}

/// A bordered block in a step.
fn block(cx: &App) -> gpui::Div {
    let t = cx.theme();
    div().flex().flex_col().gap(px(10.)).p(px(14.)).rounded(px(sz::R_MD)).bg(t.bg_sunken.opacity(0.5)).border_1().border_color(t.line)
}

impl Onboarding {
    #[allow(clippy::too_many_arguments)]
    /// A card to pick: a lead (monogram, logo, icon), a title, one line, an optional badge.
    fn choice(&self, id: SharedString, selected: bool, lead: AnyElement, title: String, sub: String, tag: Option<String>, on: impl Fn(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        div()
            .id(gpui::ElementId::Name(id))
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(10.))
            .rounded(px(sz::R_MD))
            .border_1()
            .cursor_pointer()
            .role(gpui::Role::RadioButton)
            .aria_label(SharedString::from(title.clone()))
            .when(selected, |d| d.bg(t.accent_soft).border_color(t.accent_ring))
            .when(!selected, |d| d.border_color(t.line).hover(|s| s.bg(t.hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                on(this, cx);
                cx.notify();
            }))
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().flex().items_center().gap(px(6.)).child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).text_color(if selected { t.accent_text } else { t.text }).child(title)).children(tag.map(|g| badge(g, cx))))
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(sub)),
            )
            .into_any_element()
    }

    fn welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.store.read(cx).settings.appearance.mode.clone();
        let appearance = segmented(
            "setup-appearance",
            vec![("system".to_string(), "System".into()), ("dark".to_string(), "Dark".into()), ("light".to_string(), "Light".into())],
            mode,
            |v, _, cx| {
                let v = v.clone();
                cx.store().update(cx, |s, cx| s.run("app.setSetting", json!({ "key": "appearance.mode", "value": v }), cx));
            },
            cx,
        );
        div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .child(crate::views::home::mark(56., cx))
            .child(heading("Welcome to kimchi", "A video editor for cutting, colour, sound, titles and 3D, with generative AI in the cut when you want it. A few questions and you're editing.", cx))
            .child(div().flex().flex_col().gap(px(8.)).child(caps("Appearance", cx)).child(div().w(px(300.)).max_w_full().child(appearance)))
            .into_any_element()
    }

    fn coming_from(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let mut cards: Vec<AnyElement> = vec![];
        for app in self.editors() {
            let id = app["id"].as_str().unwrap_or("").to_string();
            let name = app["name"].as_str().unwrap_or("").to_string();
            let found = app["installed"] == true;
            let sel = self.coming_from.as_deref() == Some(id.as_str());
            // The editor's own logo where kimchi really works with it; its initials otherwise.
            let lead = if crate::ui::logos::logo_file(&id).is_some() { div().flex_none().size(px(34.)).flex().items_center().justify_center().child(crate::ui::logo(&id, px(30.))).into_any_element() } else { monogram(&name, cx) };
            let pick = id.clone();
            cards.push(self.choice(
                format!("from-{id}").into(),
                sel,
                lead,
                name,
                app["vendor"].as_str().unwrap_or("").to_string(),
                found.then(|| "Found on this computer".to_string()),
                move |this, _| {
                    this.coming_from = Some(pick.clone());
                    this.use_keys = true;
                },
                cx,
            ));
        }
        for (id, name, sub) in NOT_AN_APP {
            let sel = self.coming_from.as_deref() == Some(id);
            let lead = div().flex_none().size(px(34.)).flex().items_center().justify_center().child(icon(if id == "new" { "sparkles" } else { "circle-help" })).into_any_element();
            cards.push(self.choice(format!("from-{id}").into(), sel, lead, name.into(), sub.into(), None, move |this, _| this.coming_from = Some(id.to_string()), cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(heading("Where are you coming from?", "kimchi can take its keyboard shortcuts and open its projects and looks.", cx))
            .child(if self.info.is_none() {
                div().text_color(cx.theme().text_2).child("Looking at this computer…").into_any_element()
            } else {
                div().grid().grid_cols(if narrow { 2 } else { 3 }).gap(px(8.)).children(cards).into_any_element()
            })
            .into_any_element()
    }

    fn bring(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let Some(app) = self.app() else { return div().into_any_element() };
        let name = app["name"].as_str().unwrap_or("").to_string();
        let app_id = app["id"].as_str().unwrap_or("").to_string();
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .when(crate::ui::logos::logo_file(&app_id).is_some(), |d| d.child(crate::ui::logo(&app_id, px(44.))))
            .child(heading(format!("Bring your work from {name}"), "Its keys, its projects and its looks. All of it can wait: kimchi keeps these in its menus.", cx));
        if let Some(layout) = app["keymap"].as_str().and_then(|k| kimchi_control::keymaps::layout(k).ok()) {
            let on = self.use_keys;
            let weak = cx.entity().downgrade();
            col = col.child(
                block(cx)
                    .child(switch("setup-keys", format!("Use {}'s shortcuts", layout.name), on, move |v, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.use_keys = v;
                            cx.notify();
                        })
                        .ok();
                    }, cx))
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(format!("{} kimchi's own keys stay for the rest. Change it in Settings, or see them all with ?.", layout.notes))),
            );
        }
        let can_open = app["extensions"].as_array().is_some_and(|e| !e.is_empty()) || app["opens"].as_array().is_some_and(|o| !o.is_empty());
        let mut projects = block(cx).child(caps("Projects", cx)).child(div().text_size(px(sz::SM)).text_color(t.text_2).child(app["bring"].as_str().unwrap_or("").to_string()));
        if can_open {
            let a2 = app.clone();
            let busy = self.busy.contains("import");
            projects = projects.child(
                div().flex().child(
                    Button::new("setup-import", if busy { "Opening…".to_string() } else { format!("Open a project from {name}…") })
                        .with_icon("folder-open")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.import_project(a2.clone(), cx))),
                ),
            );
        }
        if let Some((ok, note)) = self.checks.get("import") {
            projects = projects.child(said(*ok, note, cx));
        }
        col = col.child(projects);
        let looks_found: usize = app["lookFolders"].as_array().into_iter().flatten().filter_map(|f| f["looks"].as_u64()).sum::<u64>() as usize;
        let mut looks = block(cx).child(caps("Looks", cx)).child(div().text_size(px(sz::SM)).text_color(t.text_2).child(app["looks"].as_str().unwrap_or("").to_string()));
        if looks_found > 0 {
            let a2 = app.clone();
            let busy = self.busy.contains("looks");
            looks = looks.child(
                div().flex().child(
                    Button::new("setup-looks", if busy { "Importing…".to_string() } else { format!("Import {looks_found} look{} from {name}'s folder", if looks_found == 1 { "" } else { "s" }) })
                        .with_icon("palette")
                        .disabled(busy || self.looks_note.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| this.import_looks(&a2, cx))),
                ),
            );
        }
        if let Some(n) = &self.looks_note {
            looks = looks.child(said(true, n, cx));
        }
        col = col.child(looks);
        if let Some(p) = app["plugins"].as_str().filter(|p| !p.is_empty()) {
            col = col.child(block(cx).child(caps("Plugins", cx)).child(div().text_size(px(sz::SM)).text_color(t.text_2).child(p.to_string())));
        }
        col.into_any_element()
    }

    fn generative_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let yes = self.generative == Some(true);
        let no = self.generative == Some(false);
        let lead = |ic: &'static str| div().flex_none().size(px(34.)).flex().items_center().justify_center().child(icon(ic).size(px(18.))).into_any_element();
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(heading(
                "Do you want to use generative AI?",
                "Make shots, stills, voices and music from a prompt, right on the timeline: animate a frame, extend a clip, bridge two shots. You connect a provider with your own key, or a model running on this computer.",
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(self.choice("gen-yes".into(), yes, lead("sparkles"), "Yes, connect a provider".into(), "Next: pick a provider and paste its key.".into(), None, |this, _| this.generative = Some(true), cx))
                    .child(self.choice("gen-no".into(), no, lead("circle-slash"), "Not now".into(), "The Generate tab and the AI actions stay hidden. Settings › Models turns them on.".into(), None, |this, _| this.generative = Some(false), cx)),
            )
            .into_any_element()
    }

    fn connect(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let providers: Vec<Value> = self.info.as_ref().and_then(|v| v["generation"]["providers"].as_array().cloned()).unwrap_or_default();
        let shown = if self.show_all { providers.len() } else { providers.len().min(6) };
        let mut rows: Vec<AnyElement> = vec![];
        for p in providers.iter().take(shown) {
            let id = p["id"].as_str().unwrap_or("").to_string();
            let name = p["name"].as_str().unwrap_or("").to_string();
            let needs_key = p["needsKey"] == true;
            let field = needs_key.then(|| self.key_field(&format!("gen:{id}"), p["keyHint"].as_str().unwrap_or(""), cx));
            let busy = self.busy.contains(&format!("gen:{id}"));
            let tag = if p["ready"] == true && p["hasKey"] == true {
                Some("Connected")
            } else if p["running"] == true {
                Some("Running on this computer")
            } else if p["quickStart"] == true {
                Some("Quick to start")
            } else {
                None
            };
            let key_url = p["keyUrl"].as_str().map(str::to_string);
            let check_id = id.clone();
            let mut row = block(cx).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(crate::ui::logo(&id, px(26.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().flex().items_center().gap(px(6.)).child(div().font_weight(FontWeight::SEMIBOLD).child(name)).children(tag.map(|g| badge(g, cx))))
                            .child(div().text_size(px(sz::SM)).text_color(t.text_2).truncate().child(p["tagline"].as_str().unwrap_or("").to_string())),
                    )
                    .children(key_url.map(|u| Button::new(SharedString::from(format!("key-url-{id}")), "Get a key").small().ghost().icon_after("arrow-up-right").on_click(move |_, _, cx| cx.open_url(&u)))),
            );
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(field.map(|f| div().flex_1().min_w_0().child(f)))
                    .child(
                        Button::new(SharedString::from(format!("check-{id}")), if busy { "Checking…" } else if needs_key { "Save and check" } else { "Check" })
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.check_provider(check_id.clone(), cx))),
                    ),
            );
            if let Some((ok, note)) = self.checks.get(&format!("gen:{id}")) {
                row = row.child(said(*ok, note, cx));
            }
            rows.push(row.into_any_element());
        }
        let more = providers.len() > shown;
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(heading("Connect your provider", format!("The quickest to start first: one key reaches many models. Keys stay in your {} and only go to their provider. You can connect more than one.", crate::ui::keychain_name()), cx))
            .children(rows)
            .when(more, |d| d.child(div().flex().child(Button::new("setup-all-providers", "Show every provider").small().ghost().on_click(cx.listener(|this, _, _, cx| {
                this.show_all = true;
                cx.notify();
            })))))
            .into_any_element()
    }

    fn agent_step(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let info = self.info.clone().unwrap_or(Value::Null);
        let chosen = self.agent_choice.clone();
        let mut found: Vec<AnyElement> = vec![];
        for tool in info["codingTools"].as_array().into_iter().flatten().filter(|c| c["found"] == true) {
            let id = tool["id"].as_str().unwrap_or("").to_string();
            let name = tool["name"].as_str().unwrap_or("").to_string();
            let using = chosen.as_deref() == Some(id.as_str());
            let pick = id.clone();
            found.push(self.choice(format!("agent-{id}").into(), using, crate::ui::logo(&id, px(26.)).into_any_element(), format!("{name} is installed"), if using { "The Agent panel runs it.".into() } else { "Use it: your own subscription, nothing to paste.".into() }, None, move |this, cx| this.use_agent(pick.clone(), None, cx), cx));
        }
        for srv in info["localServers"].as_array().into_iter().flatten().filter(|s| s["answering"] == true && matches!(s["id"].as_str(), Some("ollama" | "lmstudio"))) {
            let id = srv["id"].as_str().unwrap_or("").to_string();
            let name = srv["name"].as_str().unwrap_or("").to_string();
            let using = chosen.as_deref() == Some(id.as_str());
            let pick = id.clone();
            found.push(self.choice(format!("agent-{id}").into(), using, crate::ui::logo(&id, px(26.)).into_any_element(), format!("{name} is running"), "A model on this computer: private, and free.".into(), None, move |this, cx| this.use_agent(pick.clone(), None, cx), cx));
        }
        let mut keys: Vec<AnyElement> = vec![];
        for (id, name, url) in AGENT_KEYS {
            let field = self.key_field(&format!("agent:{id}"), "", cx);
            let using = chosen.as_deref() == Some(id);
            let f2 = field.clone();
            keys.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(crate::ui::logo(id, px(20.)))
                    .child(div().w(px(140.)).flex_none().truncate().text_size(px(sz::SM)).child(name))
                    .child(div().flex_1().min_w_0().child(field))
                    .child(Button::new(SharedString::from(format!("agent-key-{id}")), if using { "In use" } else { "Save and use" }).small().selected(using).on_click(cx.listener(move |this, _, _, cx| {
                        let k = f2.read(cx).text().to_string();
                        this.use_agent(id.to_string(), Some(k), cx)
                    })))
                    .child(Button::icon(SharedString::from(format!("agent-url-{id}")), "arrow-up-right", "Get a key").on_click(move |_, _, cx| cx.open_url(url)))
                    .into_any_element(),
            );
        }
        let data_dir = self.store.read(cx).session.data_dir.clone();
        let mcp = kimchi_control::discovery::entry(&data_dir, None).mcp.map(|p| p.display().to_string()).unwrap_or_else(|| crate::ui::mcp_fallback().into());
        let line = format!("claude mcp add kimchi -- {} --live", crate::ui::shell_quote(&mcp));
        let copy_line = line.clone();
        let none = self.agent == Some(false);
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(heading(
                "An AI assistant in kimchi?",
                format!("The Agent panel ({}) edits with you: a rough cut from your clips, captions, titles, a colour pass. You see every change and can take a whole run back.", crate::actions::hint(&crate::actions::ToggleAgent).unwrap_or_default()),
                cx,
            ))
            .child({
                let using = chosen.as_deref() == Some("lsuite");
                let ready = self.store.read(cx).account.as_ref().is_some_and(|a| a.signed_in && a.can_run && !a.expired);
                block(cx)
                    .when(using, |d| d.border_color(t.accent_ring))
                    .child(div().flex().items_center().child(div().flex_1().child(caps("No setup", cx))).child({
                        let b = Button::new("agent-lsuite", if using { "In use" } else { "Use lsuite AI" }).small().selected(using);
                        let b = if ready && !using { b.primary() } else { b };
                        b.on_click(cx.listener(|this, _, _, cx| this.use_agent("lsuite".into(), None, cx)))
                    }))
                    .child(self.lsuite.clone())
            })
            .when(!found.is_empty(), |d| d.child(div().flex().flex_col().gap(px(8.)).child(caps("On this computer", cx)).children(found)))
            .child(block(cx).child(caps("With an API key", cx)).children(keys))
            .child(self.choice("agent-none".into(), none, div().flex_none().size(px(26.)).flex().items_center().justify_center().child(icon("circle-slash")).into_any_element(), "No assistant".into(), "The Agent panel stays hidden. Settings › Agent brings it back.".into(), None, |this, _| {
                this.agent = Some(false);
                this.agent_choice = None;
            }, cx))
            .child(
                block(cx)
                    .child(caps("Let other AI apps control kimchi", cx))
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Every command is a tool for Claude Code, Codex, Cursor or any MCP client. In a terminal:"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pl(px(10.))
                            .pr(px(4.))
                            .py(px(4.))
                            .rounded(px(sz::R_SM))
                            .bg(t.bg_sunken)
                            .border_1()
                            .border_color(t.line)
                            .child(div().flex_1().min_w_0().font_family(MONO).text_size(px(sz::XS)).child(line))
                            .child(Button::new("setup-copy-mcp", if self.copied { "Copied" } else { "Copy" }).small().ghost().with_icon(if self.copied { "check" } else { "copy" }).on_click(cx.listener(move |this, _, _, cx| this.copy(copy_line.clone(), cx)))),
                    ),
            )
            .into_any_element()
    }

    fn done(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let keys = match self.app().and_then(|a| a["keymap"].as_str().map(str::to_string)) {
            Some(k) if self.use_keys => kimchi_control::keymaps::layout(&k).map(|l| format!("{}'s keys, kimchi's for the rest", l.name)).unwrap_or_default(),
            _ => "kimchi's own keys".to_string(),
        };
        let lines = [
            ("keyboard", format!("Keyboard: {keys}.")),
            ("sparkles", match self.generative {
                Some(false) => "Generative AI: hidden.".to_string(),
                _ => "Generative AI: on.".to_string(),
            }),
            ("bot", match (self.agent, &self.agent_choice) {
                (Some(false), _) => "Assistant: none.".to_string(),
                (_, Some(p)) => format!("Assistant: {p}."),
                _ => "Assistant: set it up in Settings › Agent when you want.".to_string(),
            }),
        ];
        let imported = self.imported.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(heading("You're set", "Here is what kimchi will use. Everything can change in Settings.", cx))
            .child(block(cx).children(lines.into_iter().map(|(ic, l)| div().flex().items_center().gap(px(8.)).text_size(px(sz::SM)).child(icon(ic).text_color(t.text_2)).child(l).into_any_element())))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .when_some(imported, |d, name| d.child(Button::new("setup-open-imported", format!("Open “{name}”")).primary().with_icon("folder-open").on_click(cx.listener(|this, _, _, cx| this.finish(false, false, cx)))))
                    .when(self.imported.is_none(), |d| d.child(Button::new("setup-new-project", "Start a new project").primary().with_icon("plus").on_click(cx.listener(|this, _, _, cx| this.finish(false, true, cx)))))
                    .child(Button::new("setup-projects", "Go to my projects").on_click(cx.listener(|this, _, _, cx| this.finish(false, false, cx)))),
            )
            .child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Find this again in Help › Set up kimchi…"))
            .into_any_element()
    }

    fn dots(&self, cx: &App) -> AnyElement {
        let t = cx.theme();
        let at = self.step.dot();
        div()
            .id("setup-dots")
            .flex()
            .items_center()
            .gap(px(6.))
            .aria_label(SharedString::from(format!("Step {} of {DOTS}", at + 1)))
            .children((0..DOTS).map(|i| div().h(px(6.)).w(px(if i == at { 18. } else { 6. })).rounded(px(sz::R_XS)).bg(if i <= at { t.accent } else { t.line_strong }).into_any_element()))
            .into_any_element()
    }
}

impl Focusable for Onboarding {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let viewport = window.viewport_size();
        let narrow = layout::breakpoint(f32::from(viewport.width)) == layout::Breakpoint::Compact;
        let (w, max_h) = layout::dialog_size(760., 900., viewport);
        let body = match self.step {
            Step::Welcome => self.welcome(cx),
            Step::ComingFrom => self.coming_from(narrow, cx),
            Step::Bring => self.bring(cx),
            Step::Generative => self.generative_step(cx),
            Step::Connect => self.connect(cx),
            Step::Agent => self.agent_step(cx),
            Step::Done => self.done(cx),
        };
        let step = self.step;
        let next_label = match step {
            Step::Welcome => "Set up kimchi",
            Step::Generative if self.generative.is_none() => "Skip",
            Step::ComingFrom if self.coming_from.is_none() => "Skip",
            Step::Agent if self.agent.is_none() => "Skip",
            _ => "Next",
        };
        let controls = crate::ui::window_controls(window, cx);
        let fullscreen = window.is_fullscreen();
        let panel = div()
            .id("setup-panel")
            .relative()
            .w(px(w))
            .max_h(px(max_h - crate::views::editor::TOPBAR_H))
            .flex()
            .flex_col()
            .rounded(px(sz::R_LG))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .overflow_hidden()
            .child(
                div()
                    .id("setup-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(if narrow { 20. } else { 32. }))
                    .py(px(28.))
                    .child(motion::enter(div().relative().child(body), ("setup-step", step.dot() * 10 + step as usize), motion::BASE, (0., 8.))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .px(px(if narrow { 20. } else { 32. }))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(self.dots(cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when(self.before().is_some(), |d| d.child(Button::new("setup-back", "Back").ghost().tooltip("Back (Esc)").on_click(cx.listener(|this, _, _, cx| {
                                if let Some(st) = this.before() {
                                    this.go(st, cx);
                                }
                            }))))
                            .when(step != Step::Done, |d| d.child(Button::new("setup-next", next_label).primary().icon_after("arrow-right").tooltip("Next (Enter)").on_click(cx.listener(|this, _, _, cx| this.next(cx))))),
                    ),
            );
        div()
            .id("onboarding")
            .key_context("Onboarding")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_back))
            .on_action(cx.listener(Self::on_next_choice))
            .role(gpui::Role::Dialog)
            .aria_label("Set up kimchi")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("setup-top")
                    .h(px(crate::views::editor::TOPBAR_H))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(if fullscreen || !cfg!(target_os = "macos") { 18. } else { 92. }))
                    .when(controls.is_none(), |d| d.pr(px(14.)))
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |e, window, _| {
                        if e.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(div().flex().items_center().gap(px(8.)).child(crate::views::home::mark(22., cx)).child(div().text_size(px(sz::MD)).font_weight(FontWeight::BOLD).child("Set up kimchi")))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(Button::new("setup-skip", "Skip setup").small().ghost().tooltip("Keep kimchi as it is. Help › Set up kimchi… comes back here.").on_click(cx.listener(|this, _, _, cx| this.finish(true, false, cx)))),
                    )
                    .children(controls),
            )
            .child(div().flex_1().min_h_0().flex().justify_center().items_start().pt(px(if narrow { 4. } else { 16. })).px(px(layout::DIALOG_MARGIN)).child(motion::fade(panel, "setup-panel-in", motion::BASE)))
    }
}

