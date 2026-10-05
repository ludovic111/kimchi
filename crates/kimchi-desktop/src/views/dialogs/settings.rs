//! Settings: models & keys per provider, the built-in agent (provider, model, key, and the
//! permissions every agent and MCP client is held to), appearance, updates (with what's new),
//! diagnostics (logs and crash reports), and about / AI control (the one-line MCP install).
//! Every change is a registry command: `generate.setKey`, `generate.setProvider`,
//! `generate.check`, `app.setSetting`, `app.setAgentKey`, `app.checkUpdates`,
//! `app.installUpdate`, `app.restart`, `app.logs`, `app.crashReports`. The window is a person, so it may change the agent's
//! permissions and keys; agents themselves can't.

mod agent;
mod diagnostics;
mod models;

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, App, ClipboardItem, Context, ElementId, Entity, FontWeight, Render, ScrollHandle, SharedString, Subscription, Window, div, prelude::*, px, relative,
};
use serde_json::{Value, json};

use crate::store::{Dialog, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{self, InputEvent, TextInput};
use crate::ui::{Button, caps, icon, segmented};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Section {
    /// Models & keys, one provider (empty: the first ready one).
    Provider(String),
    Agent,
    Appearance,
    Updates,
    Diagnostics,
    About,
}

impl Section {
    fn parse(s: Option<&str>) -> Option<Self> {
        Some(match s? {
            "models" | "keys" | "providers" => Section::Provider(String::new()),
            "agent" | "permissions" => Section::Agent,
            "appearance" | "theme" => Section::Appearance,
            "updates" | "update" | "whatsnew" => Section::Updates,
            "diagnostics" | "logs" | "crashes" | "crash" => Section::Diagnostics,
            "about" | "ai" | "mcp" | "control" => Section::About,
            id => Section::Provider(id.to_string()),
        })
    }
}

pub struct SettingsDialog {
    store: Entity<Store>,
    section: Section,
    open: bool,

    // Models & keys: the fields show the provider they were loaded for.
    key: Entity<TextInput>,
    base: Entity<TextInput>,
    option: Entity<TextInput>,
    loaded_provider: Option<String>,
    checking: bool,
    check: Option<(bool, String)>,

    // Agent.
    agent: agent::AgentState,
    diag: diagnostics::DiagState,

    checking_updates: bool,
    reduce_transparency: bool,
    mcp: Option<PathBuf>,
    cli: Option<PathBuf>,
    copied: Option<&'static str>,
    nav_scroll: ScrollHandle,
    body_scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl SettingsDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let field = |cx: &mut Context<Self>, mono: bool| {
            cx.new(|cx| {
                let mut i = TextInput::new(cx);
                i.mono = mono;
                i
            })
        };
        let key = field(cx, true);
        let base = field(cx, true);
        let option = field(cx, false);
        let agent = agent::AgentState::new(cx);
        let mut subs = vec![cx.observe_in(&store, window, |this: &mut Self, store, _, cx| {
            let dialog = store.read(cx).dialog.clone();
            let open = matches!(dialog, Some(Dialog::Settings { .. }));
            if open && !this.open {
                let section = match &dialog {
                    Some(Dialog::Settings { section }) => Section::parse(section.as_deref()),
                    _ => None,
                };
                this.show(section.unwrap_or_else(|| this.section.clone()), cx);
            }
            this.open = open;
            let settings = store.read(cx).settings.clone();
            this.agent.sync(&settings, cx);
            cx.notify();
        })];
        for (input, which) in [(&key, 0u8), (&base, 1), (&option, 2)] {
            subs.push(cx.subscribe_in(input, window, move |this, _, e: &InputEvent, window, cx| match (e, which) {
                (InputEvent::Changed(_), _) => cx.notify(),
                (InputEvent::Submit, 0) => this.save_key(cx),
                (InputEvent::Submit, _) | (InputEvent::Blur, 1 | 2) => this.save_field(which, window, cx),
                (InputEvent::Blur, _) => cx.notify(),
                (InputEvent::Cancel, _) => this.store.update(cx, |s, cx| s.close_dialog(cx)),
            }));
        }
        subs.extend(agent::AgentState::subscribe(&agent, window, cx));
        let data_dir = store.read(cx).session.data_dir.clone();
        let entry = kimchi_control::discovery::entry(&data_dir, None);
        let mut this = Self {
            store,
            section: Section::Provider(String::new()),
            open: false,
            key,
            base,
            option,
            loaded_provider: None,
            checking: false,
            check: None,
            agent,
            diag: Default::default(),
            checking_updates: false,
            reduce_transparency: false,
            mcp: entry.mcp,
            cli: entry.cli,
            copied: None,
            nav_scroll: ScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            _subs: subs,
        };
        // A `defaults` read: off the main thread.
        let task = cx.background_spawn(async { crate::theme::os_reduces_transparency() });
        cx.spawn(async move |this, cx| {
            let on = task.await;
            this.update(cx, |s, cx| {
                s.reduce_transparency = on;
                cx.notify();
            })
            .ok();
        })
        .detach();
        let settings = this.store.read(cx).settings.clone();
        this.agent.sync(&settings, cx);
        this
    }

    fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.body_scroll.set_offset(gpui::point(px(0.), px(0.)));
        if self.section == Section::Agent {
            self.load_agent(cx);
        }
        if self.section == Section::Diagnostics {
            self.load_diagnostics(cx);
        }
        cx.notify();
    }

    fn select(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section != section {
            self.show(section, cx);
        }
    }

    /// Runs a command; `then` gets the result on success (errors are toasts).
    fn run(&self, name: &str, params: Value, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Value, &mut Context<Self>) + 'static) {
        let task = self.store.update(cx, |s, cx| s.call(name, params, cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| match r {
                Ok(v) => then(this, v, cx),
                Err(e) => this.store.update(cx, |s, cx| s.error(e, cx)),
            })
            .ok();
        })
        .detach();
    }

    fn set_setting(&self, key: &str, value: Value, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.run("app.setSetting", json!({ "key": key, "value": value }), cx));
    }

    fn copy(&mut self, which: &'static str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(which);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1600)).await;
            this.update(cx, |this, cx| {
                if this.copied == Some(which) {
                    this.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- the frame ------------------------------------------------------------

    fn nav(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let providers = self.store.read(cx).providers.clone();
        let current = self.current_provider(cx).map(|p| p.info.id.clone());
        let item = |id: ElementId, label: SharedString, lead: AnyElement, selected: bool, section: Section, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(9.))
                .h(px(30.))
                .px(px(9.))
                .rounded(px(sz::R_SM))
                .text_size(px(sz::BASE))
                .cursor_pointer()
                .role(gpui::Role::Tab)
                .aria_label(label.clone())
                .when(selected, |d| d.bg(t.accent_soft).text_color(t.accent_text).font_weight(FontWeight::SEMIBOLD))
                .when(!selected, |d| d.text_color(t.text_2).hover(|s| s.bg(t.hover).text_color(t.text)))
                .on_click(cx.listener(move |this, _, _, cx| this.select(section.clone(), cx)))
                .child(lead)
                .child(div().flex_1().min_w_0().truncate().child(label))
                .into_any_element()
        };
        let mut children: Vec<AnyElement> = vec![div().px(px(9.)).pb(px(4.)).child(caps("kimchi", cx)).into_any_element()];
        for (sec, label, ic) in [
            (Section::Appearance, "Appearance", "sun"),
            (Section::Agent, "Agent", "bot"),
            (Section::Updates, "Updates", "refresh-cw"),
            (Section::Diagnostics, "Diagnostics", "file-text"),
            (Section::About, "About & AI control", "waypoints"),
        ] {
            let selected = self.section == sec;
            let lead = icon(ic).text_color(if selected { t.accent_text } else { t.text_2 }).into_any_element();
            children.push(item(ElementId::Name(format!("nav-{label}").into()), label.into(), lead, selected, sec, cx));
        }
        for (kind, title, ic) in [(kimchi_gen::ProviderKind::Cloud, "Cloud models", "cloud"), (kimchi_gen::ProviderKind::Local, "On your machine", "cpu")] {
            children.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(9.))
                    .pt(px(14.))
                    .pb(px(4.))
                    .text_color(t.text_2)
                    .child(icon(ic).size(px(11.)).text_color(t.text_2))
                    .child(caps(title, cx))
                    .into_any_element(),
            );
            for p in providers.iter().filter(|p| p.info.kind == kind) {
                let selected = matches!(self.section, Section::Provider(_)) && current.as_deref() == Some(p.info.id.as_str());
                // The provider's logo, its status dot at the corner.
                let lead = div()
                    .relative()
                    .flex_none()
                    .child(crate::ui::logo(&p.info.id, px(16.)))
                    .child(div().absolute().right(px(-3.)).bottom(px(-3.)).child(status_dot(p, cx)))
                    .into_any_element();
                children.push(item(ElementId::Name(format!("nav-provider-{}", p.info.id).into()), p.info.name.clone().into(), lead, selected, Section::Provider(p.info.id.clone()), cx));
            }
        }
        if providers.is_empty() {
            children.push(div().px(px(9.)).py(px(6.)).text_size(px(sz::SM)).text_color(t.text_2).child("Loading providers…").into_any_element());
        }
        div()
            .id("settings-nav")
            .w(px(if narrow { 168. } else { 210. }))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.nav_scroll)
            .p(px(10.))
            .border_r_1()
            .border_color(t.line)
            .role(gpui::Role::TabList)
            .flex()
            .flex_col()
            .gap(px(1.))
            .children(children)
            .into_any_element()
    }

    fn title(&self) -> (String, String) {
        match &self.section {
            Section::Provider(_) => ("Models & keys".into(), format!("Keys are stored in your {} and only sent to the provider they belong to.", crate::ui::keychain_name())),
            Section::Agent => ("Agent".into(), "The built-in agent, and what any agent or MCP client may do in kimchi.".into()),
            Section::Appearance => ("Appearance".into(), "Light or dark, and the glass of the chrome.".into()),
            Section::Updates => ("Updates".into(), format!("kimchi {} · signed updates from GitHub Releases", kimchi_control::update::CURRENT)),
            Section::Diagnostics => ("Diagnostics".into(), "Logs and crash reports, to understand what went wrong.".into()),
            Section::About => ("About & AI control".into(), "Drive kimchi from Claude Code, Codex, scripts and its own agent.".into()),
        }
    }

    // ---- appearance -----------------------------------------------------------

    fn appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let a = self.store.read(cx).settings.appearance.clone();
        let weak = cx.entity().downgrade();
        let mode = segmented(
            "appearance-mode",
            vec![("system".to_string(), "System".into()), ("dark".to_string(), "Dark".into()), ("light".to_string(), "Light".into())],
            a.mode.clone(),
            move |v, _, cx| {
                let v = v.clone();
                weak.update(cx, |this, cx| this.set_setting("appearance.mode", json!(v), cx)).ok();
            },
            cx,
        );
        let weak = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(group(
                "Mode",
                Some("System follows your computer's light or dark setting and changes with it."),
                div().w(px(300.)).child(mode).into_any_element(),
                cx,
            ))
            .child(toggle_row(
                "appearance-transparency",
                "Transparency",
                "Panels, menus and dialogs are glass over a tinted backdrop. Off: every surface is opaque.",
                a.transparency,
                true,
                move |on, _, cx| {
                    weak.update(cx, |this, cx| this.set_setting("appearance.transparency", json!(on), cx)).ok();
                },
                cx,
            ))
            .when(self.reduce_transparency, |d| {
                d.child(note("circle-alert", "Reduce transparency is on in your system's accessibility settings, so surfaces stay opaque whatever this says.", t.warning, cx))
            })
            .into_any_element()
    }

    // ---- updates --------------------------------------------------------------

    fn check_updates(&mut self, cx: &mut Context<Self>) {
        self.checking_updates = true;
        cx.notify();
        self.run_or_reset("app.checkUpdates", cx);
    }

    fn run_or_reset(&self, name: &'static str, cx: &mut Context<Self>) {
        let task = self.store.update(cx, |s, cx| s.call(name, json!({}), cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                this.checking_updates = false;
                match r {
                    Ok(v) => {
                        // The session also announces it; take it now so the answer shows at once.
                        if let Ok(status) = serde_json::from_value::<kimchi_control::update::UpdateStatus>(v) {
                            this.store.update(cx, |s, cx| {
                                s.update = status;
                                cx.notify();
                            });
                        }
                    }
                    Err(e) => this.store.update(cx, |s, cx| s.error(e, cx)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn updates(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let u = s.update.clone();
        let check_on_start = s.settings.updates.check_on_start;
        let env_off = std::env::var("KIMCHI_NO_UPDATE").is_ok_and(|v| !v.is_empty() && v != "0");
        let weak = cx.entity().downgrade();
        let checked = u.checked_at.map(|at| format!("Last checked {}.", crate::views::home::ago(at))).unwrap_or_else(|| "Not checked yet in this session.".into());
        let auto_install = s.settings.updates.auto_install;
        let show_whats_new = s.settings.updates.show_whats_new;
        let status: AnyElement = if u.ready {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .p(px(12.))
                .rounded(px(sz::R_MD))
                .bg(t.success.opacity(0.1))
                .border_1()
                .border_color(t.success.opacity(0.35))
                .child(note("circle-check", &format!("kimchi {} is ready. Restart to use it.", u.available.clone().unwrap_or_default()), t.success, cx))
                .child(Button::new("update-restart", "Restart now").primary().with_icon("rotate-ccw").on_click(|_, _, cx| crate::app::restart(cx)))
                .into_any_element()
        } else if let Some(v) = u.available.clone() {
            let download = u.download_url.clone().unwrap_or_else(|| kimchi_control::update::RELEASES_URL.to_string());
            div()
                .p(px(14.))
                .rounded(px(sz::R_MD))
                .bg(t.accent_soft)
                .border_1()
                .border_color(t.accent_ring)
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().flex().items_center().gap(px(8.)).child(icon("download").text_color(t.accent_text)).child(div().font_weight(FontWeight::SEMIBOLD).child(format!("kimchi {v} is available"))))
                .when_some(u.notes.clone().filter(|n| !n.trim().is_empty()), |d, n| {
                    d.child(div().id("update-notes").max_h(px(180.)).overflow_y_scroll().text_size(px(sz::SM)).text_color(t.text_2).child(crate::ui::markdown::render(&n, cx)))
                })
                .when_some(u.progress, |d, p| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(div().flex_1().h(px(6.)).rounded_full().bg(t.line).overflow_hidden().child(div().h_full().w(relative(p.clamp(0.0, 1.0) as f32)).bg(t.accent)))
                            .child(div().font_family(MONO).text_size(px(sz::XS)).child(format!("{}%", (p * 100.0).round()))),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when(u.can_install, |d| {
                            d.child(
                                Button::new("update-install", if u.progress.is_some() { "Installing…" } else { "Install and restart" })
                                    .primary()
                                    .with_icon("download")
                                    .disabled(u.progress.is_some())
                                    .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("app.installUpdate", json!({}), cx))),
                            )
                        })
                        .when(!u.can_install, |d| {
                            d.child(Button::new("update-download", "Download").primary().icon_after("external-link").on_click(move |_, _, cx| cx.open_url(&download)))
                        }),
                )
                .when_some(u.install_blocked.clone().filter(|_| !u.can_install), |d, why| d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child(why)))
                .into_any_element()
        } else if u.checked_at.is_some() && u.error.is_none() {
            note("circle-check", "kimchi is up to date.", t.success, cx).into_any_element()
        } else {
            div().into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().flex().items_baseline().gap(px(8.)).child(div().text_size(px(sz::XL)).font_weight(FontWeight::SEMIBOLD).child("kimchi")).child(div().font_family(MONO).text_color(t.text_2).child(u.current.clone())))
                            .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(checked)),
                    )
                    .child(
                        Button::new("update-check", if self.checking_updates { "Checking…" } else { "Check now" })
                            .with_icon(if self.checking_updates { "loader-circle" } else { "refresh-cw" })
                            .disabled(self.checking_updates)
                            .on_click(cx.listener(|this, _, _, cx| this.check_updates(cx))),
                    ),
            )
            .child(status)
            .when_some(u.error.clone(), |d, e| d.child(note("circle-alert", &e, t.danger, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(12.))
                    .rounded(px(sz::R_MD))
                    .bg(t.bg_sunken.opacity(0.6))
                    .border_1()
                    .border_color(t.line)
                    .child(icon("gift").text_color(t.accent_text))
                    .child(div().flex_1().min_w_0().child(format!("What changed in kimchi {}", kimchi_control::update::CURRENT)))
                    .child(Button::new("whats-new", "What's new").small().on_click(|_, _, cx| {
                        cx.store().update(cx, |s, cx| s.open_dialog(Dialog::WhatsNew { since: None, all: false }, cx))
                    })),
            )
            .child(toggle_row(
                "updates-on-start",
                "Check for updates automatically",
                "Asks GitHub Releases at start and every few hours. Updates are signed and verified before anything is replaced.",
                check_on_start,
                true,
                move |on, _, cx| {
                    weak.update(cx, |this, cx| this.set_setting("updates.checkOnStart", json!(on), cx)).ok();
                },
                cx,
            ))
            .child({
                let weak = cx.entity().downgrade();
                toggle_row(
                    "updates-auto-install",
                    "Download and install updates by themselves",
                    "A found update is installed in the background and used the next time kimchi starts.",
                    auto_install,
                    check_on_start,
                    move |on, _, cx| {
                        weak.update(cx, |this, cx| this.set_setting("updates.autoInstall", json!(on), cx)).ok();
                    },
                    cx,
                )
            })
            .child({
                let weak = cx.entity().downgrade();
                toggle_row(
                    "updates-whats-new",
                    "Show what's new after an update",
                    "Once, the first time a new version opens.",
                    show_whats_new,
                    true,
                    move |on, _, cx| {
                        weak.update(cx, |this, cx| this.set_setting("updates.showWhatsNew", json!(on), cx)).ok();
                    },
                    cx,
                )
            })
            .child(if env_off {
                note("info", "KIMCHI_NO_UPDATE is set in this environment, so kimchi doesn't check at start. “Check now” still works.", t.text_2, cx)
            } else {
                note("info", "For managed or scripted installs, set KIMCHI_NO_UPDATE=1 to turn the check at start off.", t.text_2, cx)
            })
            .into_any_element()
    }

    // ---- about & AI control ---------------------------------------------------

    fn about(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let mcp = self.mcp.as_ref().map(|p| p.display().to_string());
        let mcp_path = mcp.clone().unwrap_or_else(|| crate::ui::mcp_fallback().into());
        let claude = format!("claude mcp add kimchi -- {} --live", crate::ui::shell_quote(&mcp_path));
        let codex = format!("codex mcp add kimchi -- {} --live", crate::ui::shell_quote(&mcp_path));
        // What Cursor and Claude Desktop take in their MCP config files.
        let json_config = json!({ "mcpServers": { "kimchi": { "command": mcp_path, "args": ["--live"] } } }).to_string();
        let cli = self.cli.as_ref().map(|p| format!("{} app.commands", crate::ui::shell_quote(&p.display().to_string())));
        let link = |id: &'static str, label: &'static str, url: &'static str| {
            Button::new(id, label).small().ghost().icon_after("arrow-up-right").on_click(move |_, _, cx| cx.open_url(url))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(crate::views::home::mark(44., cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().flex().items_baseline().gap(px(8.)).child(div().text_size(px(sz::XL)).font_weight(FontWeight::BOLD).child("kimchi")).child(div().font_family(MONO).text_color(t.text_2).child(kimchi_control::update::CURRENT)))
                            .child(div().text_color(t.text_2).child("A video editor where generative models are part of the cut."))
                            .child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Part of lsuite, the free open-source creative suite. MIT licensed.")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .child(link("about-page", "lsuite.xyz/kimchi", "https://lsuite.xyz/kimchi"))
                    .child(link("about-support", "Support", crate::app::SUPPORT_URL))
                    .child(link("about-docs", "AI control guide", crate::app::HELP_URL)),
            )
            .child(div().h(px(1.)).bg(t.line))
            .child(app_group(
                &["claude-code"],
                "Connect Claude Code",
                Some("Every command in kimchi is a tool for Claude Code, Codex or any MCP client. Live, it drives this window, and your permissions in Settings › Agent apply."),
                self.code_line("copy-claude", claude, cx),
                cx,
            ))
            .child(app_group(&["codex"], "Connect Codex", None, self.code_line("copy-codex", codex, cx), cx))
            .child(app_group(
                &["cursor", "claude-desktop", "vscode"],
                "Connect Cursor, Claude Desktop or VS Code",
                Some("Add this server to Cursor's mcp.json or Claude Desktop's claude_desktop_config.json. VS Code takes the same server under \"servers\" in .vscode/mcp.json."),
                self.code_line("copy-json", json_config, cx),
                cx,
            ))
            .when_some(cli, |d, cli| d.child(group("From a terminal", Some("kimchi-cli runs the same commands, on the open window or on a project file."), self.code_line("copy-cli", cli, cx), cx)))
            .when(mcp.is_none(), |d| {
                d.child(note("circle-alert", "kimchi-mcp wasn't found next to this copy of kimchi (a development build?). Build it with `cargo build -p kimchi-mcp`.", t.warning, cx))
            })
            .into_any_element()
    }

    fn code_line(&self, which: &'static str, text: String, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let copied = self.copied == Some(which);
        let copy_text = text.clone();
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
            .child(div().flex_1().min_w_0().font_family(MONO).text_size(px(sz::XS)).text_color(t.text).child(text))
            .child(
                Button::new(which, if copied { "Copied" } else { "Copy" })
                    .small()
                    .ghost()
                    .with_icon(if copied { "check" } else { "copy" })
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(which, copy_text.clone(), cx))),
            )
            .into_any_element()
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (title, subtitle) = self.title();
        // A narrow window gets a narrower list of sections.
        let narrow = f32::from(window.viewport_size().width) < 760.;
        let nav = self.nav(narrow, cx);
        let body = match self.section.clone() {
            Section::Provider(_) => self.provider_detail(window, cx),
            Section::Agent => self.agent_section(window, cx),
            Section::Appearance => self.appearance(cx),
            Section::Updates => self.updates(cx),
            Section::Diagnostics => self.diagnostics_section(cx),
            Section::About => self.about(cx),
        };
        div()
            .id("settings-dialog")
            .key_context("SettingsDialog")
            .role(gpui::Role::Dialog)
            .aria_label("Settings")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(12.))
                    .px(px(20.))
                    .pt(px(18.))
                    .pb(px(14.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(sz::LG)).font_weight(FontWeight::SEMIBOLD).child(title))
                            .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(subtitle)),
                    )
                    .child(Button::icon("settings-x", "x", "Close (Esc)").on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))),
            )
            .child(
                // As tall as the window allows (the dialog frame caps it), both sides scroll.
                div()
                    .h(px(560.))
                    .min_h_0()
                    .flex_shrink_1()
                    .flex()
                    .child(nav)
                    .child(div().id("settings-body").debug_selector(|| "settings-body".into()).flex_1().min_w_0().h_full().overflow_y_scroll().track_scroll(&self.body_scroll).p(px(20.)).child(body)),
            )
    }
}

// ---- shared pieces ------------------------------------------------------------

/// A titled group: caps label, an optional line of help, then the control.
fn group(title: &str, help: Option<&str>, control: AnyElement, cx: &App) -> AnyElement {
    let t = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(div().text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child(title.to_string()))
        .when_some(help, |d, h| d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child(h.to_string())))
        .child(control)
        .into_any_element()
}

/// A `group` for connecting outside apps: their logos before the title.
fn app_group(logos: &[&str], title: &str, help: Option<&str>, control: AnyElement, cx: &App) -> AnyElement {
    let t = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .children(logos.iter().map(|id| crate::ui::logo(id, px(16.))))
                .child(div().ml(px(2.)).text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child(title.to_string())),
        )
        .when_some(help, |d, h| d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child(h.to_string())))
        .child(control)
        .into_any_element()
}

/// A switch with a title and one line of explanation.
fn toggle_row(id: &'static str, title: &str, desc: &str, on: bool, enabled: bool, on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .py(px(6.))
        .role(gpui::Role::Switch)
        .aria_label(SharedString::from(title.to_string()))
        .when(!enabled, |d| d.opacity(0.45))
        .when(enabled, |d| d.cursor_pointer().on_click(move |_, w, cx| on_toggle(!on, w, cx)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .min_w_0()
                .child(div().font_weight(FontWeight::MEDIUM).child(title.to_string()))
                .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(desc.to_string())),
        )
        .child(
            div()
                .flex_none()
                .w(px(30.))
                .h(px(18.))
                .rounded_full()
                .p(px(2.))
                .bg(if on { t.accent } else { t.line_strong })
                .child(div().size(px(14.)).rounded_full().bg(gpui::white()).when(on, |d| d.ml(px(12.)))),
        )
        .into_any_element()
}

/// An icon and a sentence in a state colour.
fn note(ic: &'static str, text: &str, color: gpui::Hsla, cx: &App) -> AnyElement {
    let t = cx.theme();
    div()
        .flex()
        .items_start()
        .gap(px(8.))
        .text_size(px(sz::SM))
        .text_color(if color == t.text_2 { t.text_2 } else { color })
        .child(icon(ic).mt(px(1.)).text_color(color))
        .child(div().flex_1().min_w_0().child(text.to_string()))
        .into_any_element()
}

/// A provider's state in the list: ready (green), enabled without a key or server (hollow
/// grey), off (empty ring).
fn status_dot(p: &kimchi_gen::ProviderStatus, cx: &App) -> AnyElement {
    let t = cx.theme();
    let d = div().size(px(8.)).flex_none().rounded_full();
    if !p.settings.enabled {
        d.border_1().border_color(t.line_strong).into_any_element()
    } else if p.ready {
        d.bg(t.success).into_any_element()
    } else {
        d.bg(t.line_strong).into_any_element()
    }
}

/// A masked key: dots and the last characters (`…abcd` from the status).
fn masked(preview: &str) -> String {
    format!("••••••••{}", preview.trim_start_matches('…'))
}

/// Shows a text field, or, while it isn't being edited, its content as dots.
fn secret_field(id: &'static str, input: &Entity<TextInput>, window: &Window, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let text = input.read(cx).text().to_string();
    if text.is_empty() || input.read(cx).is_focused(window) {
        return div().flex_1().min_w_0().child(input.clone()).into_any_element();
    }
    let input = input.clone();
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .px(px(10.))
        .py(px(6.))
        .rounded(px(sz::R_SM))
        .bg(t.bg_sunken.opacity(if t.is_dark() { 0.7 } else { 0.9 }))
        .border_1()
        .border_color(t.line_strong)
        .font_family(MONO)
        .text_color(t.text)
        .cursor_text()
        .aria_label("API key (hidden)")
        .on_click(move |_, window, cx| input::focus(&input, window, cx))
        .child(format!("{} ({} characters)", "•".repeat(text.chars().count().min(24)), text.chars().count()))
        .into_any_element()
}
