//! Settings › Agent: whether the Agent panel is offered, which provider runs it (grouped: the
//! CLIs on this computer, model APIs, local servers; each with whether it is usable right now
//! and the one thing to do next, from `kimchi_agent::provider_status`), the chosen one's key
//! (`app.setAgentKey`), address and model (picked from `kimchi_agent::list_models`), and the
//! permissions that hold for the built-in agent and every MCP client alike
//! (`agent.permissions.*`).

use std::collections::HashMap;

use gpui::{AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div, prelude::*, px};
use kimchi_agent::ProviderKind;
use kimchi_control::Settings;
use serde_json::json;

use super::{SettingsDialog, group, masked, note, secret_field, toggle_row};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, icon};

/// The permission switches: (setting key, title, what it allows).
const PERMISSIONS: [(&str, &str, &str); 6] = [
    ("files", "Files", "Import media, export, write files, save a copy of the project."),
    ("projects", "Projects", "Create, open, close, duplicate or delete projects."),
    ("generate", "Generate", "Generate images and video (spends the provider's credits)."),
    ("settings", "Settings", "Change settings other than these permissions and API keys."),
    ("appControl", "App control", "Quit kimchi, install an update."),
    ("plugins", "Plugins", "Write, build, install, remove and switch plugins (Plugins › Build with your agent)."),
];

pub(super) struct AgentState {
    statuses: Vec<kimchi_agent::ProviderStatus>,
    /// The chosen provider's models.
    models: Option<kimchi_agent::ModelList>,
    /// Saved agent keys, as `…abcd`.
    previews: HashMap<&'static str, String>,
    loading: bool,
    /// The next load fetches the model list again.
    refresh_models: bool,
    generation: u64,
    key: Entity<TextInput>,
    model: Entity<TextInput>,
    base: Entity<TextInput>,
    /// The (provider, model, base URL) the fields were filled from.
    loaded: Option<(String, String, String)>,
    /// lsuite AI's account card.
    lsuite: Entity<crate::views::lsuite::LsuiteCard>,
}

impl AgentState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsDialog>) -> Self {
        let field = |cx: &mut Context<SettingsDialog>| {
            cx.new(|cx| {
                let mut i = TextInput::new(cx);
                i.mono = true;
                i
            })
        };
        let lsuite = cx.new(|cx| crate::views::lsuite::LsuiteCard::new(window, cx));
        Self { statuses: vec![], models: None, previews: HashMap::new(), loading: false, refresh_models: false, generation: 0, key: field(cx), model: field(cx), base: field(cx), loaded: None, lsuite }
    }

    pub(super) fn subscribe(this: &Self, window: &mut Window, cx: &mut Context<SettingsDialog>) -> Vec<Subscription> {
        let mut subs = vec![];
        for (input, which) in [(&this.key, 0u8), (&this.model, 1), (&this.base, 2)] {
            subs.push(cx.subscribe_in(input, window, move |dialog, _, e: &InputEvent, _, cx| match (e, which) {
                (InputEvent::Changed(_), _) => cx.notify(),
                (InputEvent::Submit, 0) => dialog.save_agent_key(cx),
                (InputEvent::Submit | InputEvent::Blur, 1) => dialog.save_agent_field("agent.model", cx),
                (InputEvent::Submit | InputEvent::Blur, 2) => dialog.save_agent_field("agent.baseUrl", cx),
                (InputEvent::Blur, _) => cx.notify(),
                (InputEvent::Cancel, _) => dialog.store.update(cx, |s, cx| s.close_dialog(cx)),
                _ => {}
            }));
        }
        subs
    }

    /// Refills the model and address fields when the settings changed under them. True when
    /// another provider was chosen (its statuses and models are loaded again).
    pub(super) fn sync(&mut self, settings: &Settings, cx: &mut Context<SettingsDialog>) -> bool {
        let a = &settings.agent;
        let now = (a.provider.clone(), a.model.clone(), a.base_url.clone());
        if self.loaded.as_ref() == Some(&now) {
            return false;
        }
        let kind = ProviderKind::parse(&a.provider).unwrap_or(ProviderKind::ClaudeCode);
        let model_placeholder = match kind.default_model() {
            "" if kind == ProviderKind::AzureOpenAi => "Your deployment's name".to_string(),
            "" if kind.group() == kimchi_agent::Group::Local => "The first model it has".to_string(),
            "" => "The CLI's own default".to_string(),
            m => format!("Default: {m}"),
        };
        let info = kind.info();
        let base_placeholder = if info.base_url_hint.is_empty() { info.default_base_url.to_string() } else { info.base_url_hint.to_string() };
        let provider_changed = self.loaded.as_ref().is_none_or(|l| l.0 != now.0);
        self.model.update(cx, |i, cx| {
            i.set_text(now.1.clone(), cx);
            i.set_placeholder(model_placeholder, cx);
        });
        self.base.update(cx, |i, cx| {
            i.set_text(now.2.clone(), cx);
            i.set_placeholder(base_placeholder, cx);
        });
        if provider_changed {
            self.key.update(cx, |i, cx| {
                i.set_text("", cx);
                i.set_placeholder(info.key.map(|k| if k.hint.is_empty() { "Paste the key" } else { k.hint }).unwrap_or(""), cx);
            });
        }
        self.loaded = Some(now);
        provider_changed
    }
}

/// The last characters of a key, as the provider list shows them.
fn tail(k: &str) -> String {
    let t: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{t}")
}

impl SettingsDialog {
    /// Reads which agent providers are usable (CLIs found and signed in, keys, Ollama), off the
    /// main thread: it runs programs and may read the keychain.
    pub(super) fn load_agent(&mut self, cx: &mut Context<Self>) {
        self.agent.loading = true;
        self.agent.generation += 1;
        let generation = self.agent.generation;
        let session = self.store.read(cx).session.clone();
        let kind = self.agent_kind(cx);
        let refresh = std::mem::take(&mut self.agent.refresh_models);
        let task = gpui_tokio::Tokio::spawn(cx, async move {
            let (list, models) = futures::join!(kimchi_agent::provider_status(&session), kimchi_agent::list_models(&session, kind, refresh));
            let previews: HashMap<&'static str, String> =
                kimchi_agent::providers::ALL.iter().filter_map(|i| i.key).filter_map(|k| session.secret(k.id).map(|v| (k.id, tail(&v)))).collect();
            (list, models, previews)
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                if this.agent.generation != generation {
                    return;
                }
                this.agent.loading = false;
                if let Ok((list, models, previews)) = r {
                    this.agent.statuses = list;
                    this.agent.models = Some(models);
                    this.agent.previews = previews;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn agent_kind(&self, cx: &gpui::App) -> ProviderKind {
        ProviderKind::parse(&self.store.read(cx).settings.agent.provider).unwrap_or(ProviderKind::ClaudeCode)
    }

    fn choose_agent(&mut self, kind: ProviderKind, cx: &mut Context<Self>) {
        if self.agent_kind(cx) == kind {
            return;
        }
        // The provider's own model and address start from its defaults; the list reloads once
        // the settings say so (`sync`).
        self.run("agent.setProvider", json!({ "provider": kind.id() }), cx, |_, _, _| {});
    }

    fn save_agent_field(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let a = self.store.read(cx).settings.agent.clone();
        let (input, current) = match key {
            "agent.model" => (&self.agent.model, a.model),
            _ => (&self.agent.base, a.base_url),
        };
        let v = input.read(cx).text().trim().to_string();
        if v != current {
            self.run("app.setSetting", json!({ "key": key, "value": v }), cx, |this, _, cx| this.load_agent(cx));
        }
    }

    fn save_agent_key(&mut self, cx: &mut Context<Self>) {
        let kind = self.agent_kind(cx);
        let key = self.agent.key.read(cx).text().trim().to_string();
        if key.is_empty() || kind.info().key.is_none() {
            return;
        }
        self.run("app.setAgentKey", json!({ "provider": kind.id(), "key": key }), cx, |this, _, cx| {
            this.agent.key.update(cx, |i, cx| i.set_text("", cx));
            this.load_agent(cx);
        });
    }

    fn remove_agent_key(&mut self, cx: &mut Context<Self>) {
        let kind = self.agent_kind(cx);
        self.run("app.setAgentKey", json!({ "provider": kind.id() }), cx, |this, _, cx| this.load_agent(cx));
    }

    /// The status's button: open the link, or copy the command to run.
    fn run_action(&mut self, action: &kimchi_agent::Action, cx: &mut Context<Self>) {
        if let Some(url) = &action.url {
            cx.open_url(url);
        } else if let Some(command) = &action.command {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(command.clone()));
            self.store.update(cx, |s, cx| s.info(format!("Copied: {command}. Run it in a terminal, then check again."), cx));
        }
    }

    fn provider_row(&self, kind: ProviderKind, active: ProviderKind, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let status = self.agent.statuses.iter().find(|s| s.provider == kind).cloned();
        let on = kind == active;
        let (state_color, state) = match &status {
            None => (t.text_2, if self.agent.loading { "Checking…" } else { "" }),
            Some(s) if s.ready => (t.success, "Ready"),
            Some(s) if s.next == Some(kimchi_agent::Next::SignIn) => (t.text_2, "Sign in"),
            Some(_) => (t.text_2, "Set up"),
        };
        let line = match &status {
            Some(s) if on || !s.ready => s.message.clone(),
            _ => kind.info().tagline.to_string(),
        };
        let action = status.as_ref().filter(|_| on).and_then(|s| s.action.clone());
        div()
            .id(SharedString::from(format!("agent-provider-{}", kind.id())))
            .flex()
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(sz::R_MD))
            .border_1()
            .cursor_pointer()
            .role(gpui::Role::RadioButton)
            .aria_label(kind.label())
            .when(on, |d| d.bg(t.accent_soft).border_color(t.accent_ring))
            .when(!on, |d| d.border_color(t.line).hover(|s| s.bg(t.hover)))
            .on_click(cx.listener(move |this, _, _, cx| this.choose_agent(kind, cx)))
            .child(div().mt(px(1.)).child(crate::ui::logo(kind.id(), px(18.))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).when(on, |d| d.text_color(t.accent_text)).child(kind.label()))
                            .child(div().flex().items_center().gap(px(5.)).text_size(px(sz::XS)).text_color(state_color).when(!state.is_empty(), |d| d.child(div().size(px(6.)).bg(state_color))).child(state)),
                    )
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(line))
                    .when_some(action, |d, a| {
                        let label = a.label.clone();
                        d.child(
                            div().mt(px(4.)).flex().child(
                                Button::new(SharedString::from(format!("agent-action-{}", kind.id())), label)
                                    .small()
                                    .icon_after(if a.url.is_some() { "arrow-up-right" } else { "copy" })
                                    .on_click(cx.listener(move |this, _, _, cx| this.run_action(&a, cx))),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Every provider, under its group's heading.
    fn provider_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.agent_kind(cx);
        let mut out: Vec<AnyElement> = vec![];
        for group in kimchi_agent::Group::ALL {
            out.push(div().pt(px(if out.is_empty() { 0. } else { 8. })).child(crate::ui::caps(group.label(), cx)).into_any_element());
            for kind in ProviderKind::ALL.into_iter().filter(|k| k.group() == group) {
                out.push(self.provider_row(kind, active, cx));
            }
        }
        div().flex().flex_col().gap(px(6.)).children(out).into_any_element()
    }

    /// The chosen provider's models as chips, narrowed by what the model field holds.
    fn model_chips(&self, current: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = cx.theme().clone();
        let list = self.agent.models.as_ref().filter(|l| l.provider == self.agent_kind(cx))?;
        let typed = self.agent.model.read(cx).text().trim().to_ascii_lowercase();
        let filter = if typed == current.to_ascii_lowercase() { String::new() } else { typed };
        let matching: Vec<&kimchi_agent::ModelInfo> = list.models.iter().filter(|m| filter.is_empty() || m.id.to_ascii_lowercase().contains(&filter)).collect();
        const SHOWN: usize = 24;
        let more = matching.len().saturating_sub(SHOWN);
        let chips = matching.into_iter().take(SHOWN).enumerate().map(|(i, m)| {
            let on = m.id == current;
            let no_tools = m.tools == Some(false);
            let id = m.id.clone();
            div()
                .id(("agent-model", i))
                .px(px(8.))
                .py(px(2.))
                
                .border_1()
                .font_family(MONO)
                .text_size(px(sz::XS))
                .cursor_pointer()
                .when(no_tools, |d| d.opacity(0.55))
                .when(on, |d| d.bg(t.accent_soft).border_color(t.accent_ring).text_color(t.accent_text))
                .when(!on, |d| d.border_color(t.line).text_color(t.text_2).hover(|s| s.bg(t.hover)))
                .tooltip({
                    let tip = match (&m.name, no_tools) {
                        (_, true) => "Can't use tools, so it can't run the agent".to_string(),
                        (Some(n), _) => n.clone(),
                        (None, _) => m.id.clone(),
                    };
                    move |_, cx| crate::ui::tooltip(tip.clone().into(), cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let v = id.clone();
                    this.agent.model.update(cx, |i, cx| i.set_text(v.clone(), cx));
                    this.run("app.setSetting", json!({ "key": "agent.model", "value": v }), cx, |this, _, cx| this.load_agent(cx));
                }))
                .child(m.id.clone())
        });
        let note = match (&list.error, list.source) {
            (Some(e), _) => Some(e.clone()),
            (None, "builtin") => Some("A few common models; any other id works too.".to_string()),
            _ => None,
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(div().flex().flex_wrap().gap(px(4.)).children(chips))
                .when(more > 0, |d| d.child(div().text_size(px(sz::XS)).text_color(t.text_2).child(format!("{more} more: type to narrow the list."))))
                .when_some(note, |d, n| d.child(div().text_size(px(sz::XS)).text_color(t.text_2).child(n)))
                .into_any_element(),
        )
    }

    pub(super) fn agent_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let settings = self.store.read(cx).settings.clone();
        let perms = settings.agent.permissions.clone();
        let kind = self.agent_kind(cx);
        let status = self.agent.statuses.iter().find(|s| s.provider == kind).cloned();
        let rows = self.provider_rows(cx);
        let info = kind.info();
        let uses_key = info.key.is_some();
        let is_lsuite = kind == ProviderKind::Lsuite;
        let uses_base = !kind.is_cli() && !is_lsuite;
        let key_empty = self.agent.key.read(cx).text().trim().is_empty();
        let preview = info.key.and_then(|k| self.agent.previews.get(k.id).cloned());
        let current_model = settings.agent.model.clone();
        let current_shown = if current_model.is_empty() { status.as_ref().map(|s| s.default_model.clone()).unwrap_or_default() } else { current_model.clone() };
        let chips = self.model_chips(&current_shown, cx);
        let model_field = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div().flex().items_center().gap(px(8.)).child(div().flex_1().min_w_0().child(self.agent.model.clone())).child(
                    Button::icon("agent-models-refresh", "refresh-cw", "Fetch the list of models again").on_click(cx.listener(|this, _, _, cx| {
                        this.agent.refresh_models = true;
                        this.load_agent(cx);
                    })),
                ),
            )
            .when_some(chips, |d, c| d.child(c))
            .into_any_element();
        let key_note = match info.key {
            Some(_) if kind == ProviderKind::Bedrock => {
                "A Bedrock API key; or kimchi uses your AWS access keys (aws configure, or AWS_ACCESS_KEY_ID). Billed by AWS per use.".to_string()
            }
            Some(k) if !k.required => "Only if the server asks for one.".to_string(),
            Some(k) => {
                let env = k.env.first().map(|e| format!(" Or set {e} in your environment.")).unwrap_or_default();
                format!("Billed by {} per use, separately from any chat subscription.{env}", info.label.trim_end_matches(" API"))
            }
            None => String::new(),
        };
        let key_url = info.key.and_then(|k| k.url);

        let key_field = uses_key.then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(7.))
                .when_some(preview.clone(), |d, p| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(sz::R_SM))
                            .bg(t.success.opacity(0.12))
                            .text_size(px(sz::SM))
                            .child(icon("key-round").text_color(t.success))
                            .child(div().font_family(MONO).text_color(t.text).child(masked(&p)))
                            .child(div().flex_1().text_color(t.text_2).child(format!("in the {}", crate::ui::keychain_name())))
                            .child(Button::new("agent-remove-key", "Remove").small().ghost().color(t.danger).on_click(cx.listener(|this, _, _, cx| this.remove_agent_key(cx)))),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(secret_field("agent-key-masked", &self.agent.key, window, cx))
                        .child(Button::new("agent-save-key", "Save").primary().disabled(key_empty).on_click(cx.listener(|this, _, _, cx| this.save_agent_key(cx)))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().text_size(px(sz::SM)).text_color(t.text_2).child(key_note.clone()))
                        .when_some(key_url, |d, url| {
                            d.child(Button::new("agent-get-key", "Get a key").small().ghost().icon_after("arrow-up-right").on_click(move |_, _, cx| cx.open_url(url)))
                        }),
                )
                .into_any_element()
        });

        let master = perms.enabled;
        let weak = cx.entity().downgrade();
        let mut perm_rows: Vec<AnyElement> = vec![toggle_row(
            "perm-enabled",
            "Let agents act in kimchi",
            "Master switch: off refuses every request from the built-in agent and from MCP clients.",
            master,
            true,
            {
                let weak = weak.clone();
                move |on, _, cx| {
                    weak.update(cx, |this, cx| this.set_setting("agent.permissions.enabled", json!(on), cx)).ok();
                }
            },
            cx,
        )];
        for (key, title, desc) in PERMISSIONS {
            let on = match key {
                "files" => perms.files,
                "projects" => perms.projects,
                "generate" => perms.generate,
                "settings" => perms.settings,
                "plugins" => perms.plugins,
                _ => perms.app_control,
            };
            let weak = weak.clone();
            perm_rows.push(toggle_row(
                match key {
                    "files" => "perm-files",
                    "projects" => "perm-projects",
                    "generate" => "perm-generate",
                    "settings" => "perm-settings",
                    "plugins" => "perm-plugins",
                    _ => "perm-app-control",
                },
                title,
                desc,
                on && master,
                master,
                move |on, _, cx| {
                    weak.update(cx, |this, cx| this.set_setting(&format!("agent.permissions.{key}"), json!(on), cx)).ok();
                },
                cx,
            ));
        }

        let enabled = settings.agent.enabled;
        let weak_enabled = cx.entity().downgrade();
        let base_help = if kind == ProviderKind::Bedrock {
            "The AWS region (empty: your AWS configuration's)."
        } else if info.needs_base_url {
            info.base_url_hint
        } else {
            "Change it for a proxy, or a server on another computer."
        };
        // Claude Code can run on lsuite AI instead of the person's own Claude sign-in.
        let signed_in = self.store.read(cx).account.as_ref().is_some_and(|a| a.signed_in);
        let on_lsuite = settings.agent.claude_code_on_lsuite;
        let weak_cc = cx.entity().downgrade();
        let claude_on_lsuite = (kind == ProviderKind::ClaudeCode && (signed_in || on_lsuite)).then(|| {
            toggle_row(
                "agent-claude-on-lsuite",
                "Run Claude Code on lsuite AI",
                "Your lsuite plan pays for Claude Code's requests, so it needs no Claude sign-in of its own.",
                on_lsuite,
                true,
                move |on, _, cx| {
                    weak_cc.update(cx, |this, cx| this.set_setting("agent.claudeCodeOnLsuite", json!(on), cx)).ok();
                },
                cx,
            )
        });
        let chosen = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(14.))
            .rounded(px(sz::R_MD))
            .border_1()
            .border_color(t.line)
            .when(is_lsuite, |d| d.child(self.agent.lsuite.clone()))
            .when(!is_lsuite, |d| d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(crate::ui::logo(kind.id(), px(20.)))
                    .child(div().flex_1().min_w_0().text_size(px(sz::MD)).font_weight(FontWeight::SEMIBOLD).child(kind.label()))
                    .child(
                        Button::new("agent-recheck", if self.agent.loading { "Checking…" } else { "Check again" })
                            .small()
                            .ghost()
                            .with_icon(if self.agent.loading { "loader-circle" } else { "refresh-cw" })
                            .disabled(self.agent.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.load_agent(cx))),
                    ),
            ))
            .when_some(status.as_ref().filter(|_| !is_lsuite).map(|s| (s.ready, s.message.clone())), |d, (ready, m)| d.child(note(if ready { "circle-check" } else { "info" }, &m, if ready { t.success } else { t.text_2 }, cx)))
            .when_some(claude_on_lsuite, |d, row| d.child(row))
            .when_some(key_field, |d, k| d.child(group(if kind == ProviderKind::Bedrock { "Bedrock API key" } else { "API key" }, None, k, cx)))
            .when(uses_base, |d| d.child(group(if kind == ProviderKind::Bedrock { "Region" } else { "Address" }, Some(base_help), self.agent.base.clone().into_any_element(), cx)))
            .child(group(if kind == ProviderKind::AzureOpenAi { "Deployment" } else { "Model" }, Some("Saved when you leave the field. Empty uses the default."), model_field, cx));

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(toggle_row(
                "agent-enabled",
                "Offer the Agent panel",
                "Off hides the Agent panel and its shortcut. MCP clients and kimchi-cli still work.",
                enabled,
                true,
                move |on, _, cx| {
                    weak_enabled.update(cx, |this, cx| this.set_setting("agent.enabled", json!(on), cx)).ok();
                },
                cx,
            ))
            .child(chosen)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(div().text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child("Who runs the agent"))
                    .child(rows),
            )
            .child(div().h(px(1.)).bg(t.line))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(px(sz::MD)).font_weight(FontWeight::SEMIBOLD).child("Permissions"))
                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(
                        "What the built-in agent and every MCP client may do besides editing the open project, which is always allowed and always undoable. Agents can't change these.",
                    ))
                    .child(div().mt(px(6.)).flex().flex_col().gap(px(2.)).children(perm_rows)),
            )
            .when(!master, |d| d.child(note("lock", "Agents are off: the agent panel and MCP clients get “not allowed” for everything.", t.warning, cx)))
            .into_any_element()
    }
}
