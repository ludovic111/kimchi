//! Settings › Agent: which model runs the built-in agent (with whether each provider is usable
//! right now, from `kimchi_agent::provider_status`), its model, address and API key
//! (`app.setAgentKey`), and the permissions that hold for the built-in agent and every MCP
//! client alike (`agent.permissions.*`).

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
const PERMISSIONS: [(&str, &str, &str); 5] = [
    ("files", "Files", "Import media, export, write files, save a copy of the project."),
    ("projects", "Projects", "Create, open, close, duplicate or delete projects."),
    ("generate", "Generate", "Generate images and video (spends the provider's credits)."),
    ("settings", "Settings", "Change settings other than these permissions and API keys."),
    ("appControl", "App control", "Quit kimchi, install an update."),
];

pub(super) struct AgentState {
    statuses: Vec<kimchi_agent::ProviderStatus>,
    /// Saved agent keys, as `…abcd`.
    previews: HashMap<&'static str, String>,
    loading: bool,
    generation: u64,
    key: Entity<TextInput>,
    model: Entity<TextInput>,
    base: Entity<TextInput>,
    /// The (provider, model, base URL) the fields were filled from.
    loaded: Option<(String, String, String)>,
}

impl AgentState {
    pub(super) fn new(cx: &mut Context<SettingsDialog>) -> Self {
        let field = |cx: &mut Context<SettingsDialog>| {
            cx.new(|cx| {
                let mut i = TextInput::new(cx);
                i.mono = true;
                i
            })
        };
        Self { statuses: vec![], previews: HashMap::new(), loading: false, generation: 0, key: field(cx), model: field(cx), base: field(cx), loaded: None }
    }

    pub(super) fn subscribe(this: &Self, window: &mut Window, cx: &mut Context<SettingsDialog>) -> Vec<Subscription> {
        let mut subs = vec![];
        for (input, which) in [(&this.key, 0u8), (&this.model, 1), (&this.base, 2)] {
            subs.push(cx.subscribe_in(input, window, move |dialog, _, e: &InputEvent, _, cx| match (e, which) {
                (InputEvent::Changed(_), _) => cx.notify(),
                (InputEvent::Submit(_), 0) => dialog.save_agent_key(cx),
                (InputEvent::Submit(_) | InputEvent::Blur, 1) => dialog.save_agent_field("agent.model", cx),
                (InputEvent::Submit(_) | InputEvent::Blur, 2) => dialog.save_agent_field("agent.baseUrl", cx),
                (InputEvent::Blur, _) => cx.notify(),
                (InputEvent::Cancel, _) => dialog.store.update(cx, |s, cx| s.close_dialog(cx)),
                _ => {}
            }));
        }
        subs
    }

    /// Refills the model and address fields when the settings changed under them.
    pub(super) fn sync(&mut self, settings: &Settings, cx: &mut Context<SettingsDialog>) {
        let a = &settings.agent;
        let now = (a.provider.clone(), a.model.clone(), a.base_url.clone());
        if self.loaded.as_ref() == Some(&now) {
            return;
        }
        let kind = ProviderKind::parse(&a.provider).unwrap_or(ProviderKind::ClaudeCode);
        let model_placeholder = match kind.default_model() {
            "" if kind == ProviderKind::Ollama => "The first installed model".to_string(),
            "" => "The CLI's own default".to_string(),
            m => format!("Default: {m}"),
        };
        let base_placeholder = kind.default_base_url().to_string();
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
                i.set_placeholder(if kind == ProviderKind::Anthropic { "sk-ant-…" } else { "sk-…" }, cx);
            });
        }
        self.loaded = Some(now);
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
        let task = gpui_tokio::Tokio::spawn(cx, async move {
            let list = kimchi_agent::provider_status(&session).await;
            let previews: HashMap<&'static str, String> = ["anthropic", "openai"].into_iter().filter_map(|id| session.secret(id).map(|k| (id, tail(&k)))).collect();
            (list, previews)
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                if this.agent.generation != generation {
                    return;
                }
                this.agent.loading = false;
                if let Ok((list, previews)) = r {
                    this.agent.statuses = list;
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
        let a = self.store.read(cx).settings.agent.clone();
        // A model id or address for one provider means nothing to another: start from its defaults.
        let mut resets = vec![];
        if !a.model.is_empty() {
            resets.push("agent.model");
        }
        if !a.base_url.is_empty() {
            resets.push("agent.baseUrl");
        }
        self.run("app.setSetting", json!({ "key": "agent.provider", "value": kind.id() }), cx, move |this, _, cx| {
            for key in resets {
                this.set_setting(key, json!(""), cx);
            }
            this.load_agent(cx);
        });
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
        if key.is_empty() || !matches!(kind, ProviderKind::Anthropic | ProviderKind::OpenAi) {
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

    fn provider_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let active = self.agent_kind(cx);
        let rows: Vec<AnyElement> = ProviderKind::ALL
            .iter()
            .map(|&kind| {
                let status = self.agent.statuses.iter().find(|s| s.provider == kind);
                let on = kind == active;
                let (state_color, state) = match status {
                    None => (t.text_2, if self.agent.loading { "Checking…" } else { "" }),
                    Some(s) if s.ready => (t.success, "Ready"),
                    Some(_) => (t.warning, "Not ready"),
                };
                div()
                    .id(SharedString::from(format!("agent-provider-{}", kind.id())))
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(sz::R_MD))
                    .border_1()
                    .cursor_pointer()
                    .role(gpui::Role::RadioButton)
                    .aria_label(kind.label())
                    .when(on, |d| d.bg(t.accent_soft).border_color(t.accent_ring))
                    .when(!on, |d| d.border_color(t.line).hover(|s| s.bg(t.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_agent(kind, cx)))
                    .child(
                        div()
                            .mt(px(2.))
                            .size(px(14.))
                            .flex_none()
                            .rounded_full()
                            .border_1()
                            .border_color(if on { t.accent } else { t.line_strong })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(on, |d| d.child(div().size(px(7.)).rounded_full().bg(t.accent))),
                    )
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
                                    .child(div().flex().items_center().gap(px(5.)).text_size(px(sz::XS)).text_color(state_color).when(!state.is_empty(), |d| d.child(div().size(px(6.)).rounded_full().bg(state_color))).child(state)),
                            )
                            .when_some(status.map(|s| s.message.clone()), |d, m| d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child(m))),
                    )
                    .into_any_element()
            })
            .collect();
        div().flex().flex_col().gap(px(6.)).children(rows).into_any_element()
    }

    pub(super) fn agent_section(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let settings = self.store.read(cx).settings.clone();
        let perms = settings.agent.permissions.clone();
        let kind = self.agent_kind(cx);
        let status = self.agent.statuses.iter().find(|s| s.provider == kind).cloned();
        let rows = self.provider_rows(cx);
        let uses_key = matches!(kind, ProviderKind::Anthropic | ProviderKind::OpenAi);
        let uses_base = !matches!(kind, ProviderKind::ClaudeCode | ProviderKind::Codex);
        let key_empty = self.agent.key.read(cx).text().trim().is_empty();
        let preview = self.agent.previews.get(kind.id()).cloned();
        let ollama_models: Vec<String> = status.as_ref().map(|s| s.models.clone()).unwrap_or_default();
        let current_model = settings.agent.model.clone();

        let model_field = {
            let chips = ollama_models.iter().enumerate().map(|(i, m)| {
                let on = *m == current_model;
                let name = m.clone();
                div()
                    .id(("ollama-model", i))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded_full()
                    .border_1()
                    .font_family(MONO)
                    .text_size(px(sz::XS))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.accent_soft).border_color(t.accent_ring).text_color(t.accent_text))
                    .when(!on, |d| d.border_color(t.line).text_color(t.text_2).hover(|s| s.bg(t.hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let v = name.clone();
                        this.agent.model.update(cx, |i, cx| i.set_text(v.clone(), cx));
                        this.run("app.setSetting", json!({ "key": "agent.model", "value": v }), cx, |this, _, cx| this.load_agent(cx));
                    }))
                    .child(m.clone())
            });
            div().flex().flex_col().gap(px(6.)).child(self.agent.model.clone()).when(!ollama_models.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(4.)).children(chips))).into_any_element()
        };

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
                            .child(div().flex_1().text_color(t.text_2).child("in the keychain"))
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
                .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(format!(
                    "Billed by {} per use, separately from any chat subscription. Or set {} in your environment.",
                    if kind == ProviderKind::Anthropic { "Anthropic" } else { "OpenAI" },
                    if kind == ProviderKind::Anthropic { "ANTHROPIC_API_KEY" } else { "OPENAI_API_KEY" }
                )))
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
                _ => perms.app_control,
            };
            let weak = weak.clone();
            perm_rows.push(toggle_row(
                match key {
                    "files" => "perm-files",
                    "projects" => "perm-projects",
                    "generate" => "perm-generate",
                    "settings" => "perm-settings",
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

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child("Who runs the agent"))
                            .child(
                                Button::new("agent-recheck", if self.agent.loading { "Checking…" } else { "Check again" })
                                    .small()
                                    .ghost()
                                    .with_icon(if self.agent.loading { "loader-circle" } else { "refresh-cw" })
                                    .disabled(self.agent.loading)
                                    .on_click(cx.listener(|this, _, _, cx| this.load_agent(cx))),
                            ),
                    )
                    .child(rows),
            )
            .child(group("Model", Some("Saved when you leave the field. Empty uses the default."), model_field, cx))
            .when(uses_base, |d| d.child(group("Address", Some("Change it for a proxy or another OpenAI-compatible server."), self.agent.base.clone().into_any_element(), cx)))
            .when_some(key_field, |d, k| d.child(group("API key", None, k, cx)))
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
