//! The agent panel, docked on the right. It runs the model the person already
//! has (Claude Code, Codex, Gemini CLI, an API key, a local server) through `kimchi-agent`, which
//! acts only through the command registry, so permissions and the one undo
//! history are the same as for MCP and the CLI. It shows one card per command
//! (from its own runs, and from MCP clients and the CLI driving kimchi), the
//! changes with "Revert this run", and how to connect an outside agent.
//!
//! The conversation itself is `kimchi_agent::Host`'s: the panel sends, stops and reverts with the
//! `agent.*` commands, as `kimchi-cli` and MCP clients can, and draws the host's snapshot.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{AnyElement, ClipboardItem, Context, Entity, FontWeight, Render, ScrollHandle, Subscription, Task, Window, div, prelude::*, px};
use kimchi_agent::{AgentConfig, Entry, ProviderKind, ProviderStatus, Snapshot};
use serde_json::json;

use crate::store::{Dialog, MenuItem, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, GlassExt, caps, icon, segmented};
use crate::views::agent::cards::Ending;
use crate::views::agent::{History, Tab};
use crate::views::generate::spinner_icon;

const EXAMPLES: [&str; 4] = [
    "Add a title \"Day one\" over the first three seconds",
    "Trim every clip to at most 4 seconds and close the gaps",
    "Put a marker at each cut",
    "Generate a 5 s shot of rain on a window after the last clip",
];

pub struct AgentPanel {
    pub(crate) store: Entity<Store>,
    tab: Tab,
    composer: Entity<TextInput>,
    model_input: Entity<TextInput>,
    title_input: Entity<TextInput>,
    memory_input: Entity<TextInput>,
    memory_open: bool,
    model_seen: String,
    /// The host's conversation and runs, as last drawn.
    pub(crate) snap: Snapshot,
    _pump: Task<()>,
    _ticker: Option<Task<()>>,
    statuses: Vec<ProviderStatus>,
    checking: bool,
    pub(crate) history: Option<History>,
    /// Terminal sessions (by checkpoint) reverted from the Changes tab.
    pub(crate) reverted_sessions: std::collections::HashSet<u64>,
    project_seen: Option<usize>,
    pub(crate) expanded: HashSet<u64>,
    scroll: ScrollHandle,
    changes_scroll: ScrollHandle,
    mcp: Option<PathBuf>,
    was_open: bool,
    provider_seen: String,
    /// lsuite AI's sign-in, shown instead of the warning while it isn't ready.
    lsuite: Entity<crate::views::lsuite::LsuiteCard>,
    _subs: Vec<Subscription>,
}

impl AgentPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let lsuite = cx.new(|cx| crate::views::lsuite::LsuiteCard::new(window, cx).compact());
        let composer = cx.new(|cx| {
            let mut i = TextInput::new(cx).multiline(2).placeholder("Ask the agent to edit the cut…");
            i.bare = true;
            i
        });
        let model_input = cx.new(|cx| TextInput::new(cx).placeholder("Provider default / model id"));
        let title_input = cx.new(|cx| TextInput::new(cx).placeholder("Conversation title"));
        let memory_input = cx.new(|cx| TextInput::new(cx).multiline(4).placeholder("Project preferences, decisions and context for every conversation…"));
        let mut subs = vec![
            cx.observe(&store, |this, _, cx| this.on_store_changed(cx)),
            cx.subscribe(&composer, |this, _, e: &InputEvent, cx| match e {
                InputEvent::Submit => this.send(cx),
                InputEvent::Changed(_) => cx.notify(),
                _ => {}
            }),
        ];
        subs.push(cx.subscribe(&model_input, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Submit | InputEvent::Blur) {
                let model = this.model_input.read(cx).text().trim().to_string();
                if model != this.store.read(cx).settings.agent.model {
                    this.store.update(cx, |s, cx| s.run("app.setSetting", json!({"key":"agent.model", "value": model}), cx));
                }
            }
        }));
        subs.push(cx.subscribe(&title_input, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Submit | InputEvent::Blur) {
                let title = this.title_input.read(cx).text().trim().to_string();
                if !title.is_empty() && title != this.snap.conversation.title {
                    this.store.update(cx, |s, cx| s.run("agent.renameConversation", json!({"title":title}), cx));
                }
            }
        }));
        let provider_seen = store.read(cx).settings.agent.provider.clone();
        // Redraw whenever the host's conversation changes, whoever changed it.
        let host = store.read(cx).agent.clone();
        let mut changes = host.subscribe();
        let pump = cx.spawn(async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |p, cx| p.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            store,
            tab: Tab::Conversation,
            composer,
            model_input, title_input, memory_input, memory_open: false, model_seen: String::new(),
            snap: host.snapshot(),
            _pump: pump,
            _ticker: None,
            statuses: vec![],
            checking: false,
            history: None,
            reverted_sessions: Default::default(),
            project_seen: None,
            expanded: HashSet::new(),
            scroll: ScrollHandle::new(),
            changes_scroll: ScrollHandle::new(),
            mcp: kimchi_agent::mcp_executable(),
            was_open: false,
            provider_seen,
            lsuite,
            _subs: subs,
        };
        this.title_input.update(cx, |i, cx| i.set_text(this.snap.conversation.title.clone(), cx));
        this.on_store_changed(cx);
        this
    }

    fn provider(&self, cx: &gpui::App) -> ProviderKind {
        AgentConfig::from_settings(&self.store.read(cx).settings.agent).provider
    }

    fn status_of(&self, kind: ProviderKind) -> Option<&ProviderStatus> {
        self.statuses.iter().find(|s| s.provider == kind)
    }

    // ---- store and session -------------------------------------------------

    fn on_store_changed(&mut self, cx: &mut Context<Self>) {
        let model = self.store.read(cx).settings.agent.model.clone();
        if self.model_seen != model {
            self.model_seen = model.clone();
            self.model_input.update(cx, |i, cx| i.set_text(model, cx));
        }
        let s = self.store.read(cx);
        let open = s.agent_open;
        let provider = s.settings.agent.provider.clone();
        let project = s.project.as_ref().map(|p| std::sync::Arc::as_ptr(p) as usize);
        if open && (!self.was_open || provider != self.provider_seen) {
            self.refresh_statuses(cx);
        }
        if open && (!self.was_open || project != self.project_seen) {
            self.project_seen = project;
            self.refresh_history(cx);
        }
        self.was_open = open;
        self.provider_seen = provider;
        cx.notify();
    }

    fn refresh_statuses(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        let session = self.store.read(cx).session.clone();
        // Probes the CLIs and Ollama: never on the UI thread.
        let task = gpui_tokio::Tokio::spawn(cx, async move { kimchi_agent::provider_status(&session).await });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |p, cx| {
                p.checking = false;
                if let Ok(list) = r {
                    p.statuses = list;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn refresh_history(&mut self, cx: &mut Context<Self>) {
        if self.store.read(cx).project.is_none() {
            self.history = None;
            return;
        }
        let task = self.store.update(cx, |s, cx| s.call("history.list", json!({}), cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |p, cx| {
                p.history = r.ok().and_then(|v| serde_json::from_value(v).ok());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes the host's latest snapshot.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let host = self.store.read(cx).agent.clone();
        let snap = host.snapshot();
        let grew = snap.entries.len() != self.snap.entries.len() || snap.entries.last().map(entry_len) != self.snap.entries.last().map(entry_len);
        let ended = self.snap.running.is_some() && snap.running.is_none();
        let started = snap.running.is_some() && self._ticker.is_none();
        if snap.entries.is_empty() {
            self.expanded.clear();
        }
        if self.snap.conversation.id != snap.conversation.id || self.snap.conversation.title != snap.conversation.title {
            self.title_input.update(cx, |i, cx| i.set_text(snap.conversation.title.clone(), cx));
        }
        if self.snap.conversation.id != snap.conversation.id {
            self.composer.update(cx, |i, cx| i.set_text("", cx));
            self.memory_open = false;
            self.memory_input.update(cx, |i, cx| i.set_text(snap.memory.clone(), cx));
        }
        self.snap = snap;
        if grew {
            self.scroll.scroll_to_bottom();
        }
        if ended {
            self._ticker = None;
            self.refresh_history(cx);
        }
        if started {
            // Keeps the elapsed time moving while the model thinks.
            self._ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let going = this.update(cx, |p, cx| {
                        cx.notify();
                        p.snap.running.is_some()
                    });
                    if !matches!(going, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    // ---- runs ----------------------------------------------------------------

    fn send(&mut self, cx: &mut Context<Self>) {
        let prompt = self.composer.read(cx).text().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        self.composer.update(cx, |i, cx| i.set_text("", cx));
        let command = if self.snap.running.is_some() { "agent.steer" } else { "agent.send" };
        self.store.update(cx, |s, cx| s.run(command, json!({ "prompt": prompt }), cx));
        self.tab = Tab::Conversation;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.run("agent.stop", json!({}), cx));
    }

    /// "Revert this run": back to the checkpoint taken before its first change, as one undo step.
    pub(crate) fn revert_run(&mut self, run: u64, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        self.store.update(cx, |s, cx| {
            s.run_then("agent.revert", json!({ "run": run }), cx, move |s, _, cx| {
                s.info("Reverted the run. Undo brings it back.", cx);
                this.update(cx, |p, cx| p.refresh_history(cx)).ok();
            })
        });
    }

    fn new_conversation(&mut self, cx: &mut Context<Self>) {
        if self.snap.running.is_none() {
            self.store.update(cx, |s, cx| s.run("agent.newConversation", json!({}), cx));
        }
    }

    fn open_agent_settings(cx: &mut gpui::App) {
        cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("agent".into()) }, cx));
    }

    fn provider_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let current = self.provider(cx);
        let this = cx.entity().downgrade();
        let mut entries: Vec<crate::store::MenuEntry> = vec![];
        for group in kimchi_agent::Group::ALL {
            if !entries.is_empty() {
                entries.push(crate::store::MenuEntry::Separator);
            }
            entries.extend(ProviderKind::ALL.into_iter().filter(|k| k.group() == group).map(|kind| {
                let ready = self.status_of(kind).map(|s| s.ready);
                let mut item = MenuItem::new(kind.label(), move |_, cx| {
                    cx.store().update(cx, |s, cx| s.run("agent.setProvider", json!({ "provider": kind.id() }), cx));
                })
.logo(kind.id())
                .shortcut(match ready {
                    Some(true) => "ready",
                    Some(false) => "set up",
                    None => "…",
                });
                if kind == current {
                    item = item.icon("check");
                }
                item.entry()
            }));
        }
        entries.push(crate::store::MenuEntry::Separator);
        entries.push(
            MenuItem::new("Check again", move |_, cx| {
                this.update(cx, |p, cx| p.refresh_statuses(cx)).ok();
            })
            .icon("refresh-cw")
            .entry(),
        );
        entries.push(MenuItem::new("Agent settings and permissions…", |_, cx| Self::open_agent_settings(cx)).icon("shield-check").entry());
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    // ---- rendering -------------------------------------------------------------

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let kind = self.provider(cx);
        let status = self.status_of(kind);
        let dot = match status.map(|s| s.ready) {
            Some(true) => t.success,
            Some(false) => t.warning,
            None => t.text_3,
        };
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(44.))
            .px(px(10.))
            .border_b_1()
            .border_color(t.line)
            .child(icon("bot").text_color(t.accent_text))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Agent"))
            .child(
                div()
                    .id("agent-provider")
                    .ml(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(sz::R_SM))
                    .border_1()
                    .border_color(t.line)
                    .text_size(px(sz::SM))
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover).text_color(t.text))
                    .tooltip(|_, cx| crate::ui::tooltip("Which model runs the agent".into(), cx))
                    .on_click(cx.listener(|this, e: &gpui::ClickEvent, _, cx| this.provider_menu(e.position(), cx)))
                    .child(div().flex_none().size(px(7.)).bg(dot))
                    .child(crate::ui::logo(kind.id(), px(14.)))
                    .child(div().truncate().child(kind.label()))
                    .child(icon("chevron-down").size(px(12.))),
            )
            .child(div().flex_1())
            .child(Button::icon("agent-new", "plus", "New conversation").disabled(self.snap.running.is_some()).on_click(cx.listener(|this, _, _, cx| this.new_conversation(cx))))
            .child(Button::icon("agent-settings", "shield-check", "Agent settings and permissions").on_click(|_, _, cx| Self::open_agent_settings(cx)))
            .child(Button::icon("agent-close", "x", crate::actions::tip("Close", &crate::actions::ToggleAgent)).on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.set_agent_open(false, cx))))
    }

    fn conversations_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let current = self.snap.conversation.id;
        let entries = self.snap.conversations.iter().map(|c| {
            let id = c.id;
            let mut item = MenuItem::new(c.title.clone(), move |_, cx| {
                cx.store().update(cx, |s, cx| s.run("agent.selectConversation", json!({"id": id}), cx));
            });
            if id == current { item = item.icon("check"); }
            item.entry()
        }).collect();
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    fn model_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let mut models = self.status_of(self.provider(cx)).map(|s| s.models.clone()).unwrap_or_default();
        let default = self.provider(cx).default_model();
        if !default.is_empty() && !models.iter().any(|m| m == default) { models.insert(0, default.into()); }
        let current = self.store.read(cx).settings.agent.model.clone();
        if !current.is_empty() && !models.contains(&current) { models.insert(0, current); }
        models.insert(0, String::new());
        let entries = models.into_iter().map(|model| {
            let label = if model.is_empty() { "Provider default".to_string() } else { model.clone() };
            MenuItem::new(label, move |_, cx| cx.store().update(cx, |s, cx| s.run("app.setSetting", json!({"key":"agent.model", "value":model}), cx))).entry()
        }).collect();
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    fn conversation_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        div().flex().flex_col().gap(px(6.)).p(px(10.)).border_b_1().border_color(t.line)
            .child(div().flex().items_center().gap(px(6.))
                .child(div().flex_1().min_w_0().child(self.title_input.clone()))
                .child(Button::icon("agent-conversations", "message-square", "Conversations in this project")
                    .disabled(self.snap.running.is_some()).on_click(cx.listener(|this, e: &gpui::ClickEvent, _, cx| this.conversations_menu(e.position(), cx)))))
            .child(div().flex().items_center().gap(px(6.))
                .child(div().text_size(px(sz::XS)).text_color(t.text_2).child("Model"))
                .child(div().flex_1().min_w_0().child(self.model_input.clone()))
                .child(Button::icon("agent-models", "chevron-down", "Choose an agent model")
                    .on_click(cx.listener(|this, e: &gpui::ClickEvent, _, cx| this.model_menu(e.position(), cx)))))
            .child(Button::new("agent-memory", if self.memory_open { "Close project memory" } else { "Project memory" }).small().ghost().with_icon("file-text")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.memory_open = !this.memory_open;
                    if this.memory_open { this.memory_input.update(cx, |i, cx| i.set_text(this.snap.memory.clone(), cx)); }
                    cx.notify();
                })))
            .when(self.memory_open, |d| d.child(self.memory_input.clone())
                .child(Button::new("agent-memory-save", "Save memory").small().on_click(cx.listener(|this, _, _, cx| {
                    let text = this.memory_input.read(cx).text().to_string();
                    this.store.update(cx, |s, cx| s.run("agent.setMemory", json!({"text":text}), cx));
                    this.memory_open = false; cx.notify();
                }))))
            .when_some(self.snap.storage_error.clone(), |d, e| d.child(div().text_size(px(sz::XS)).text_color(t.danger).child(e)))
            .into_any_element()
    }

    /// Warns when the chosen provider can't run, or agents are turned off.
    fn notices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let kind = self.provider(cx);
        let (text, action): (String, Option<&'static str>) = if !s.settings.agent.permissions.enabled {
            ("Agents are turned off: the agent, MCP clients and `kimchi-cli --agent` are refused.".into(), Some("Turn on in Settings › Agent"))
        } else if kind == ProviderKind::Lsuite {
            // lsuite AI: its own card (sign in, or the plan), only while it isn't ready.
            self.status_of(kind).filter(|st| !st.ready)?;
            return Some(div().m(px(10.)).mb_0().p(px(12.)).border_1().border_color(t.line_strong).bg(t.bg_raised).child(self.lsuite.clone()).into_any_element());
        } else {
            (self.status_of(kind).filter(|st| !st.ready)?.message.clone(), None)
        };
        Some(
            div()
                .m(px(10.))
                .mb_0()
                .p(px(10.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(sz::R_MD))
                .bg(t.warning.opacity(0.1))
                .border_1()
                .border_color(t.warning.opacity(0.4))
                .text_size(px(sz::SM))
                .child(div().flex().gap(px(8.)).child(icon("circle-alert").mt(px(2.)).text_color(t.warning)).child(div().flex_1().min_w_0().line_height(px(18.)).child(text)))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            Button::new("notice-check", if self.checking { "Checking…" } else { "Check again" })
                                .small()
                                .with_icon("refresh-cw")
                                .disabled(self.checking)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_statuses(cx))),
                        )
                        .child(Button::new("notice-settings", action.unwrap_or("Settings › Agent")).small().ghost().on_click(|_, _, cx| Self::open_agent_settings(cx))),
                )
                .into_any_element(),
        )
    }

    fn mcp_line(&self, i: usize, client: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let path = self.mcp.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| crate::ui::mcp_fallback().into());
        let line = format!("{client} mcp add kimchi -- {} --live", crate::ui::shell_quote(&path));
        let copy = line.clone();
        div()
            .id(("mcp-line", i))
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(8.))
            .pr(px(2.))
            .py(px(2.))
            .rounded(px(sz::R_SM))
            .bg(t.bg_sunken.opacity(0.7))
            .border_1()
            .border_color(t.line)
            .child(crate::ui::logo(client, px(14.)))
            .child(div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(10.5)).child(line))
            .child(Button::icon(("mcp-copy", i), "copy", "Copy").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                cx.store().update(cx, |s, cx| s.info("Copied. Paste it in a terminal.", cx));
            }))
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let kind = self.provider(cx);
        // When it isn't ready, the notice above says why.
        let status = self.status_of(kind).filter(|s| s.ready).map(|s| s.message.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(14.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().size(px(32.)).rounded(px(sz::R_MD)).bg(t.accent_soft).flex().items_center().justify_center().child(icon("bot").size(px(17.)).text_color(t.accent_text)))
                    .child(div().text_size(px(sz::MD)).font_weight(FontWeight::SEMIBOLD).child("Edit by asking"))
                    .child(div().text_size(px(sz::SM)).line_height(px(18.)).text_color(t.text_2).child(
                        "The agent cuts, trims, titles, arranges and generates with the same commands as the CLI and MCP. It uses the model you already have: Claude Code, Codex, an API key or a model on this computer.",
                    ))
                    .child(div().text_size(px(sz::SM)).line_height(px(18.)).text_color(t.text_2).child(
                        "Every command shows up here as a card, every change lands in the one undo history, and a whole run can be reverted.",
                    ))
                    .when_some(status, |d, m| {
                        d.child(div().flex().gap(px(6.)).text_size(px(sz::XS)).text_color(t.text_2).child(icon("info").size(px(12.)).mt(px(1.))).child(div().flex_1().min_w_0().child(m)))
                    }),
            )
            .child(
                div().flex().flex_col().gap(px(6.)).child(caps("Try", cx)).children(EXAMPLES.iter().enumerate().map(|(i, ex)| {
                    let text = ex.to_string();
                    div()
                        .id(("example", i))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(sz::R_MD))
                        .border_1()
                        .border_color(t.line)
                        .text_size(px(sz::SM))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).border_color(t.line_strong))
                        .child(ex.to_string())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let text = text.clone();
                            this.composer.update(cx, |i, cx| i.set_text(text, cx));
                            crate::ui::input::focus(&this.composer, window, cx);
                            cx.notify();
                        }))
                })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(caps("Permissions", cx))
                    .child(div().text_size(px(sz::SM)).line_height(px(18.)).text_color(t.text_2).child(
                        "Editing is always allowed and always undoable. Files, projects, generation (which spends credits), settings and quitting are each a switch, the same for this agent and MCP clients.",
                    ))
                    .child(div().flex().child(Button::new("perm-open", "Settings › Agent").small().with_icon("shield-check").on_click(|_, _, cx| Self::open_agent_settings(cx)))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(caps("Use your own agent", cx))
                    .child(div().text_size(px(sz::SM)).line_height(px(18.)).text_color(t.text_2).child(
                        "Connect Claude Code or Codex to this window over MCP; what they do shows up here too.",
                    ))
                    .child(self.mcp_line(0, "claude", cx))
                    .child(self.mcp_line(1, "codex", cx)),
            )
    }

    fn running_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = cx.theme().clone();
        let a = self.snap.running.and_then(|id| self.snap.run(id))?;
        let secs = a.seconds() as u64;
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(sz::SM))
                .text_color(t.text_2)
                .child(div().text_color(t.accent_text).child(spinner_icon("loader-circle", true, "agent-spin", 13.)))
                .child(div().flex_1().min_w_0().truncate().child(a.activity.clone().unwrap_or_else(|| "Working…".into())))
                .child(div().font_family(MONO).text_size(px(sz::XS)).child(format!("{}:{:02}", secs / 60, secs % 60)))
                .into_any_element(),
        )
    }

    fn conversation_view(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.snap.entries.is_empty() && self.snap.running.is_none() {
            return div().id("agent-scroll").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(self.empty_state(cx)).into_any_element();
        }
        let items: Vec<AnyElement> = self
            .snap
            .entries
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                Entry::User { text, source, .. } => self.user_bubble(i, text, *source, cx),
                Entry::Assistant { text, .. } => self.assistant_text(text, cx),
                Entry::Command { record, result, .. } => self.command_card(record, result.as_ref(), cx),
                Entry::Outcome { run, state, error, changes, seconds, tokens } => {
                    self.outcome_row(i, Ending { run: *run, state: *state, error: error.as_deref(), changes: *changes, seconds: *seconds, tokens: *tokens }, cx)
                }
            })
            .collect();
        let running = self.running_row(cx);
        div()
            .id("agent-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(div().flex().flex_col().gap(px(8.)).p(px(12.)).children(items).children(running))
            .into_any_element()
    }

    fn composer_view(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let focused = self.composer.read(cx).is_focused(window);
        let running = self.snap.running.is_some();
        let empty = self.composer.read(cx).text().trim().is_empty();
        // What the agent is told about the window with the request.
        let glance = kimchi_agent::glance(&self.store.read(cx).session);
        let detail: gpui::SharedString = format!("The agent is told what you see as you send:\n{}", glance.lines.iter().skip(1).cloned().collect::<Vec<_>>().join("\n")).into();
        div().p(px(10.)).border_t_1().border_color(t.line).child(
            div()
                .rounded(px(sz::R_LG))
                .bg(t.bg_sunken.opacity(if t.is_dark() { 0.55 } else { 0.7 }))
                .border_1()
                .border_color(if focused { t.accent_ring } else { t.line_strong })
                .child(
                    div()
                        .id("agent-glance")
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(10.))
                        .pt(px(7.))
                        .text_size(px(sz::XS))
                        .text_color(t.text_2)
                        .tooltip(move |_, cx| crate::ui::tooltip(detail.clone(), cx))
                        .child(icon("scan-eye").size(px(12.)))
                        .child(div().flex_1().min_w_0().truncate().child(glance.short)),
                )
                .child(div().max_h(px(200.)).child(self.composer.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .pb(px(6.))
                        .child(div().flex_1().text_size(px(sz::XS)).text_color(t.text_2).child(if running { "Send to steer the current run.".to_string() } else { format!("{} to send", crate::actions::keys_label("M-enter")) }))
                        .when(running, |d| d.child(Button::icon("agent-stop", "square", "Stop").color(t.danger).on_click(cx.listener(|this, _, _, cx| this.stop(cx)))))
                        .child(Button::new("agent-send", if running { "Steer" } else { "Send" }).small().primary().disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx)))),
                ),
        )
    }
}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let agent_steps = self.history.as_ref().map(|h| h.undo.iter().filter(|s| s.source != "window").count()).unwrap_or(0);
        let this = cx.entity().downgrade();
        let header = self.header(cx).into_any_element();
        let notices = self.notices(cx);
        let body = match self.tab {
            Tab::Conversation => self.conversation_view(cx),
            Tab::Changes => div().id("agent-changes").size_full().overflow_y_scroll().track_scroll(&self.changes_scroll).child(self.changes_tab(cx)).into_any_element(),
        };
        let composer = self.composer_view(window, cx).into_any_element();
        div()
            .size_full()
            .flex()
            .flex_col()
            .glass(t.glass1)
            .border_0()
            .border_l_1()
            .border_color(t.line)
            .text_size(px(sz::BASE))
            .child(header)
            .child(div().id("agent-options").flex_none().max_h(px((f32::from(window.viewport_size().height) * 0.4).max(120.)))
                .overflow_y_scroll().child(self.conversation_controls(cx)).children(notices))
            .child(
                div().px(px(10.)).pt(px(10.)).child(segmented(
                    "agent-tab",
                    vec![(Tab::Conversation, "Conversation".into()), (Tab::Changes, if agent_steps > 0 { format!("Changes · {agent_steps}").into() } else { "Changes".into() })],
                    self.tab,
                    move |tab, _, cx| {
                        let tab = *tab;
                        this.update(cx, |p, cx| {
                            p.tab = tab;
                            if tab == Tab::Changes {
                                p.refresh_history(cx);
                            }
                            cx.notify();
                        })
                        .ok();
                    },
                    cx,
                )),
            )
            .child(div().flex_1().min_h_0().child(body))
            .when(self.tab == Tab::Conversation, |d| d.child(composer))
    }
}

/// How much an entry holds, to notice the last one growing (streamed text).
fn entry_len(e: &Entry) -> usize {
    match e {
        Entry::Assistant { text, .. } => text.len(),
        Entry::Command { result, .. } => usize::from(result.is_some()),
        _ => 0,
    }
}
