//! The agent panel, docked on the right. It runs the model the person already
//! has (Claude Code, Codex, an API key, Ollama) through `kimchi-agent`, which
//! acts only through the command registry, so permissions and the one undo
//! history are the same as for MCP and the CLI. It shows one card per command
//! (from its own runs, and from MCP clients and the CLI driving kimchi), the
//! changes with "Revert this run", and how to connect an outside agent.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui::{AnyElement, ClipboardItem, Context, Entity, FontWeight, Render, ScrollHandle, Subscription, Task, Window, div, prelude::*, px};
use kimchi_agent::{Agent, AgentConfig, AgentEvent, Conversation, ProviderKind, ProviderStatus};
use kimchi_control::CommandRecord;
use serde_json::{Value, json};

use crate::store::{Dialog, MenuItem, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, GlassExt, caps, icon, segmented};
use crate::views::agent::{Active, History, Item, Outcome, OutcomeKind, RunSummary, Tab};
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
    pub(crate) items: Vec<Item>,
    /// Command seq → index in `items`, so a command reported twice (by the run
    /// and by the session's stream) shows once.
    seen: HashMap<u64, usize>,
    last_seq: u64,
    conversation: Conversation,
    pub(crate) runs: Vec<RunSummary>,
    active: Option<Active>,
    _pump: Option<Task<()>>,
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
    _subs: Vec<Subscription>,
}

impl AgentPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let composer = cx.new(|cx| {
            let mut i = TextInput::new(cx).multiline(2).placeholder("Ask the agent to edit the cut…");
            i.bare = true;
            i
        });
        let subs = vec![
            cx.observe(&store, |this, _, cx| this.on_store_changed(cx)),
            cx.subscribe(&composer, |this, _, e: &InputEvent, cx| match e {
                InputEvent::Submit => this.send(cx),
                InputEvent::Changed(_) => cx.notify(),
                _ => {}
            }),
        ];
        let provider_seen = store.read(cx).settings.agent.provider.clone();
        let mut this = Self {
            store,
            tab: Tab::Conversation,
            composer,
            items: vec![],
            seen: HashMap::new(),
            last_seq: 0,
            conversation: Conversation::new(),
            runs: vec![],
            active: None,
            _pump: None,
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
            _subs: subs,
        };
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
        let s = self.store.read(cx);
        let open = s.agent_open;
        let fresh: Vec<CommandRecord> = s.commands.iter().filter(|r| r.seq > self.last_seq).cloned().collect();
        let provider = s.settings.agent.provider.clone();
        let project = s.project.as_ref().map(|p| Arc::as_ptr(p) as usize);
        for r in fresh {
            self.last_seq = self.last_seq.max(r.seq);
            self.add_command(r, None);
        }
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

    fn add_command(&mut self, record: CommandRecord, result: Option<Value>) {
        if let Some(&i) = self.seen.get(&record.seq) {
            if let (Some(r), Some(Item::Command { result: slot, .. })) = (result, self.items.get_mut(i)) {
                *slot = Some(r);
            }
            return;
        }
        self.seen.insert(record.seq, self.items.len());
        self.items.push(Item::Command { record: Box::new(record), result });
        self.scroll.scroll_to_bottom();
    }

    // ---- runs ----------------------------------------------------------------

    fn send(&mut self, cx: &mut Context<Self>) {
        let prompt = self.composer.read(cx).text().trim().to_string();
        if prompt.is_empty() || self.active.is_some() {
            return;
        }
        self.composer.update(cx, |i, cx| i.set_text("", cx));
        let s = self.store.read(cx);
        let session = s.session.clone();
        let config = AgentConfig::from_settings(&s.settings.agent);
        let provider = config.provider;
        self.items.push(Item::User { text: prompt.clone() });
        self.runs.push(RunSummary { prompt: prompt.clone(), provider, checkpoint: None, changes: 0, finished: false, reverted: false, at: chrono::Local::now() });
        let run_ix = self.runs.len() - 1;
        // The run itself lives on the session's Tokio runtime; this only reads its events.
        let mut run = Agent::start(&session, config, prompt, self.conversation.clone());
        let handle = run.handle();
        let mut events = run.take_events().expect("fresh run");
        self.active = Some(Active { handle, run: run_ix, status: Some(format!("Starting {}…", provider.label())), started: Instant::now(), tokens: (0, 0), streamed: false });
        self._pump = Some(cx.spawn(async move |this, cx| {
            while let Some(ev) = events.next().await {
                if this.update(cx, |p, cx| p.on_agent_event(ev, cx)).is_err() {
                    break;
                }
            }
        }));
        // Keeps the elapsed time moving while the model thinks.
        self._ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let going = this.update(cx, |p, cx| {
                    cx.notify();
                    p.active.is_some()
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        }));
        self.tab = Tab::Conversation;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn on_agent_event(&mut self, ev: AgentEvent, cx: &mut Context<Self>) {
        match ev {
            AgentEvent::Status { message } => {
                if let Some(a) = &mut self.active {
                    a.status = Some(message);
                }
            }
            AgentEvent::Text { delta } => {
                if let Some(a) = &mut self.active {
                    a.streamed = true;
                }
                match self.items.last_mut() {
                    Some(Item::Assistant { text }) => text.push_str(&delta),
                    _ => self.items.push(Item::Assistant { text: delta.trim_start().to_string() }),
                }
                self.scroll.scroll_to_bottom();
            }
            AgentEvent::Command { record, result } => {
                self.last_seq = self.last_seq.max(record.seq);
                self.add_command(*record, result);
            }
            AgentEvent::Usage { input_tokens, output_tokens } => {
                if let Some(a) = &mut self.active {
                    a.tokens.0 += input_tokens;
                    a.tokens.1 += output_tokens;
                }
            }
            AgentEvent::Done { summary, checkpoint, changes, conversation } => {
                self.conversation = conversation;
                if self.active.as_ref().is_some_and(|a| !a.streamed) && !summary.trim().is_empty() {
                    self.items.push(Item::Assistant { text: summary });
                }
                self.finish(OutcomeKind::Done, None, checkpoint, changes, cx);
            }
            AgentEvent::Error { message, checkpoint, changes } => {
                if let Some(a) = &self.active {
                    self.conversation = a.handle.conversation();
                }
                self.finish(OutcomeKind::Error, Some(message), checkpoint, changes, cx);
            }
            AgentEvent::Cancelled { checkpoint, changes } => {
                if let Some(a) = &self.active {
                    self.conversation = a.handle.conversation();
                }
                self.finish(OutcomeKind::Cancelled, None, checkpoint, changes, cx);
            }
        }
        cx.notify();
    }

    fn finish(&mut self, kind: OutcomeKind, message: Option<String>, checkpoint: Option<u64>, changes: usize, cx: &mut Context<Self>) {
        let Some(a) = self.active.take() else { return };
        if let Some(r) = self.runs.get_mut(a.run) {
            r.checkpoint = checkpoint;
            r.changes = changes;
            r.finished = true;
        }
        self.items.push(Item::Outcome(Outcome { run: a.run, kind, message, changes, tokens: a.tokens, secs: a.started.elapsed().as_secs_f32() }));
        self.scroll.scroll_to_bottom();
        self.refresh_history(cx);
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(a) = &mut self.active {
            a.handle.cancel();
            a.status = Some("Stopping…".into());
        }
        cx.notify();
    }

    /// "Revert this run": back to the checkpoint taken before its first change, as one undo step.
    pub(crate) fn revert_run(&mut self, run: usize, cx: &mut Context<Self>) {
        let Some(cp) = self.runs.get(run).and_then(|r| r.checkpoint) else { return };
        let task = self.store.update(cx, |s, cx| s.call("history.revertTo", json!({ "checkpoint": cp }), cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |p, cx| {
                match r {
                    Ok(_) => {
                        if let Some(run) = p.runs.get_mut(run) {
                            run.reverted = true;
                        }
                        p.store.update(cx, |s, cx| s.info("Reverted the run. Undo brings it back.", cx));
                    }
                    Err(e) => p.store.update(cx, |s, cx| s.error(e, cx)),
                }
                p.refresh_history(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn new_conversation(&mut self, cx: &mut Context<Self>) {
        if self.active.is_some() {
            return;
        }
        self.items.clear();
        self.seen.clear();
        self.expanded.clear();
        self.conversation = Conversation::new();
        cx.notify();
    }

    fn open_agent_settings(cx: &mut gpui::App) {
        cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("agent".into()) }, cx));
    }

    fn provider_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let current = self.provider(cx);
        let this = cx.entity().downgrade();
        let mut entries: Vec<crate::store::MenuEntry> = ProviderKind::ALL
            .iter()
            .map(|&kind| {
                let ready = self.status_of(kind).map(|s| s.ready);
                let mut item = MenuItem::new(kind.label(), move |_, cx| {
                    cx.store().update(cx, |s, cx| s.run("app.setSetting", json!({ "key": "agent.provider", "value": kind.id() }), cx));
                })
                .shortcut(match ready {
                    Some(true) => "ready",
                    Some(false) => "set up",
                    None => "…",
                });
                if kind == current {
                    item = item.icon("check");
                }
                item.entry()
            })
            .collect();
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
                    .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
                    .child(div().truncate().child(kind.label()))
                    .child(icon("chevron-down").size(px(12.))),
            )
            .child(div().flex_1())
            .child(Button::icon("agent-new", "plus", "New conversation").disabled(self.active.is_some() || self.items.is_empty()).on_click(cx.listener(|this, _, _, cx| this.new_conversation(cx))))
            .child(Button::icon("agent-settings", "shield-check", "Agent settings and permissions").on_click(|_, _, cx| Self::open_agent_settings(cx)))
            .child(Button::icon("agent-close", "x", crate::actions::tip("Close", &crate::actions::ToggleAgent)).on_click(|_, _, cx| {
                cx.store().update(cx, |s, cx| {
                    s.agent_open = false;
                    s.sync_ui(cx);
                    cx.notify();
                })
            }))
    }

    /// Warns when the chosen provider can't run, or agents are turned off.
    fn notices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let kind = self.provider(cx);
        let (text, action): (String, Option<&'static str>) = if !s.settings.agent.permissions.enabled {
            ("Agents are turned off: the agent, MCP clients and `kimchi-cli --agent` are refused.".into(), Some("Turn on in Settings › Agent"))
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
        let path = self.mcp.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "/Applications/kimchi.app/Contents/MacOS/kimchi-mcp".into());
        let line = format!("{client} mcp add kimchi -- {path} --live");
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
            .child(icon("terminal").size(px(12.)).text_color(t.text_2))
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
        let a = self.active.as_ref()?;
        let secs = a.started.elapsed().as_secs();
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(sz::SM))
                .text_color(t.text_2)
                .child(div().text_color(t.accent_text).child(spinner_icon("loader-circle", true, "agent-spin", 13.)))
                .child(div().flex_1().min_w_0().truncate().child(a.status.clone().unwrap_or_else(|| "Working…".into())))
                .child(div().font_family(MONO).text_size(px(sz::XS)).child(format!("{}:{:02}", secs / 60, secs % 60)))
                .into_any_element(),
        )
    }

    fn conversation_view(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.items.is_empty() && self.active.is_none() {
            return div().id("agent-scroll").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(self.empty_state(cx)).into_any_element();
        }
        let items: Vec<AnyElement> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                Item::User { text } => self.user_bubble(i, text, cx),
                Item::Assistant { text } => self.assistant_text(text, cx),
                Item::Command { record, result } => self.command_card(record, result.as_ref(), cx),
                Item::Outcome(o) => self.outcome_row(i, o, cx),
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
        let running = self.active.is_some();
        let empty = self.composer.read(cx).text().trim().is_empty();
        div().p(px(10.)).border_t_1().border_color(t.line).child(
            div()
                .rounded(px(sz::R_LG))
                .bg(t.bg_sunken.opacity(if t.is_dark() { 0.55 } else { 0.7 }))
                .border_1()
                .border_color(if focused { t.accent_ring } else { t.line_strong })
                .child(div().max_h(px(200.)).child(self.composer.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .pb(px(6.))
                        .child(div().flex_1().text_size(px(sz::XS)).text_color(t.text_2).child(if running { "Running. Stop keeps finished edits.".to_string() } else { format!("{} to send", crate::actions::keys_label("M-enter")) }))
                        .child(if running {
                            Button::icon("agent-stop", "square", "Stop").color(t.danger).on_click(cx.listener(|this, _, _, cx| this.stop(cx))).into_any_element()
                        } else {
                            div()
                                .id("agent-send")
                                .size(px(30.))
                                .rounded(px(sz::R_MD))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(t.accent)
                                .text_color(t.text_on_accent)
                                .tooltip(|_, cx| crate::ui::tooltip(format!("Send ({})", crate::actions::keys_label("M-enter")).into(), cx))
                                .child(icon("arrow-up").size(px(15.)).text_color(t.text_on_accent))
                                .when(empty, |d| d.opacity(0.35).cursor_not_allowed())
                                .when(!empty, |d| d.cursor_pointer().hover(|s| s.bg(t.accent_hover)).on_click(cx.listener(|this, _, _, cx| this.send(cx))))
                                .into_any_element()
                        }),
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
            .children(notices)
            .child(div().flex_1().min_h_0().child(body))
            .when(self.tab == Tab::Conversation, |d| d.child(composer))
    }
}
