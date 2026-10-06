//! Settings › Models & keys: one image/video provider — on/off, its API key (saved in the
//! keychain through `generate.setKey`), server address and options (`generate.setProvider`),
//! and a connection test (`generate.check`).

use gpui::{AnyElement, App, Context, FontWeight, Window, div, prelude::*, px};
use kimchi_gen::{KeySource, ProviderStatus};
use serde_json::{Value, json};

use super::{Section, SettingsDialog, group, masked, note, secret_field, toggle_row};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, icon};

/// Provider options worth a field: (provider, option key, label, placeholder, help).
const OPTIONS: [(&str, &str, &str, &str, &str); 4] = [
    ("comfyui", "workflows_dir", "Workflows folder", "~/Documents/kimchi/comfyui-workflows", "API-format workflow JSON files with {{prompt}}, {{image}}, {{seed}}… placeholders become models."),
    ("fal", "models", "Extra models", "fal-ai/some-model, fal-ai/another", "Comma-separated endpoint ids to add to the picker."),
    ("replicate", "models", "Extra models", "owner/model, owner/model:version", "Comma-separated. Inputs are read from each model's schema."),
    ("openai_compat", "model", "Model id", "e.g. stablediffusion", "Used when the server doesn't list its models."),
];

fn option_for(provider: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    OPTIONS.iter().find(|o| o.0 == provider).map(|o| (o.1, o.2, o.3, o.4))
}

impl SettingsDialog {
    /// The provider shown: the one asked for, else the first ready one, else OpenRouter.
    pub(super) fn current_provider<'a>(&self, cx: &'a App) -> Option<&'a ProviderStatus> {
        let list = &self.store.read(cx).providers;
        let want = match &self.section {
            Section::Provider(id) if !id.is_empty() => Some(id.as_str()),
            _ => self.loaded_provider.as_deref(),
        };
        want.and_then(|w| list.iter().find(|p| p.info.id == w))
            .or_else(|| list.iter().find(|p| p.ready))
            .or_else(|| list.iter().find(|p| p.info.id == "openrouter"))
            .or(list.first())
    }

    /// Fills the fields for `p` when it isn't the one they show.
    fn load_provider(&mut self, p: &ProviderStatus, cx: &mut Context<Self>) {
        if self.loaded_provider.as_deref() == Some(p.info.id.as_str()) {
            return;
        }
        self.loaded_provider = Some(p.info.id.clone());
        self.check = None;
        self.checking = false;
        let key_placeholder = if p.key_preview.is_some() { "Replace the key".to_string() } else { p.info.key_hint.clone().unwrap_or_else(|| "Paste your key".into()) };
        self.key.update(cx, |i, cx| {
            i.set_text("", cx);
            i.set_placeholder(key_placeholder, cx);
        });
        let base = p.settings.base_url.clone().unwrap_or_default();
        let default_base = p.info.default_base_url.clone();
        self.base.update(cx, |i, cx| {
            i.set_text(base, cx);
            i.set_placeholder(default_base, cx);
        });
        if let Some((key, _, placeholder, _)) = option_for(&p.info.id) {
            let v = option_text(p.settings.options.get(key));
            self.option.update(cx, |i, cx| {
                i.set_text(v, cx);
                i.set_placeholder(placeholder, cx);
            });
        }
    }

    pub(super) fn save_key(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.loaded_provider.clone() else { return };
        let key = self.key.read(cx).text().trim().to_string();
        if key.is_empty() {
            return;
        }
        self.run("generate.setKey", json!({ "provider": id, "key": key }), cx, |this, v, cx| {
            this.take_statuses(v, cx);
            this.key.update(cx, |i, cx| {
                i.set_text("", cx);
                i.set_placeholder("Replace the key", cx);
            });
            this.test(cx);
        });
    }

    fn remove_key(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.loaded_provider.clone() else { return };
        self.run("generate.setKey", json!({ "provider": id }), cx, |this, v, cx| {
            this.take_statuses(v, cx);
            this.check = None;
            cx.notify();
        });
    }

    /// `generate.setKey` answers with every provider's status.
    fn take_statuses(&mut self, v: Value, cx: &mut Context<Self>) {
        if let Ok(list) = serde_json::from_value::<Vec<ProviderStatus>>(v) {
            self.store.update(cx, |s, cx| {
                s.providers = list;
                cx.notify();
            });
        }
    }

    /// `generate.setProvider` answers with the one provider.
    fn take_status(&mut self, v: Value, cx: &mut Context<Self>) {
        if let Ok(p) = serde_json::from_value::<ProviderStatus>(v) {
            self.store.update(cx, |s, cx| {
                if let Some(slot) = s.providers.iter_mut().find(|x| x.info.id == p.info.id) {
                    *slot = p;
                }
                cx.notify();
            });
        }
    }

    fn set_provider(&mut self, patch: Value, cx: &mut Context<Self>) {
        let Some(id) = self.loaded_provider.clone() else { return };
        let mut params = json!({ "provider": id });
        if let (Some(p), Some(o)) = (params.as_object_mut(), patch.as_object()) {
            p.extend(o.clone());
        }
        self.run("generate.setProvider", params, cx, |this, v, cx| this.take_status(v, cx));
    }

    /// Saves the server address (1) or the option (2) when it changed.
    pub(super) fn save_field(&mut self, which: u8, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.current_provider(cx).cloned() else { return };
        if self.loaded_provider.as_deref() != Some(p.info.id.as_str()) {
            return;
        }
        match which {
            1 => {
                let v = self.base.read(cx).text().trim().to_string();
                if v != p.settings.base_url.clone().unwrap_or_default() {
                    self.set_provider(json!({ "baseUrl": v }), cx);
                }
            }
            _ => {
                let Some((key, ..)) = option_for(&p.info.id) else { return };
                let v = self.option.read(cx).text().trim().to_string();
                if v != option_text(p.settings.options.get(key)) {
                    let value = if v.is_empty() { Value::Null } else { json!(v) };
                    self.set_provider(json!({ "options": { key: value } }), cx);
                }
            }
        }
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.loaded_provider.clone() else { return };
        self.checking = true;
        self.check = None;
        cx.notify();
        let task = self.store.update(cx, |s, cx| s.call("generate.check", json!({ "provider": id }), cx));
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |this, cx| {
                // Only if the same provider is still shown.
                if this.loaded_provider.as_deref() == Some(id.as_str()) {
                    this.checking = false;
                    this.check = Some(match r {
                        Ok(v) => (true, v["message"].as_str().unwrap_or("It answers.").to_string()),
                        Err(e) => (false, e),
                    });
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// A warning about what was pasted in the key field, before it is saved.
    fn key_warning(&self, p: &ProviderStatus, cx: &App) -> Option<String> {
        let k = self.key.read(cx).text().trim().to_string();
        if k.is_empty() {
            return None;
        }
        let lower = k.to_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("www.") {
            return Some("That's a link, not a key. Open “Get a key”, create one, and paste the key itself.".into());
        }
        if k.chars().any(char::is_whitespace) {
            return Some("Keys don't contain spaces — check what was copied.".into());
        }
        // Hints like "sk-or-v1-…" double as the expected start of a key.
        let prefix = p.info.key_hint.as_deref().and_then(|h| h.strip_suffix('…'));
        match prefix {
            Some(pre) if !pre.is_empty() && !k.starts_with(pre) => Some(format!("{} keys usually start with “{pre}”.", p.info.name)),
            _ => None,
        }
    }

    pub(super) fn provider_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let Some(p) = self.current_provider(cx).cloned() else {
            return div().text_color(t.text_2).child("Loading providers…").into_any_element();
        };
        self.load_provider(&p, cx);
        let id = p.info.id.clone();
        let weak = cx.entity().downgrade();
        let tasks = p.info.tasks.iter().map(|task| {
            let s = serde_json::to_value(task).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default().replace('_', " ");
            let mut c = s.chars();
            let label = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
            div().px(px(8.)).py(px(2.)).bg(t.hover).border_1().border_color(t.line).text_size(px(sz::XS)).text_color(t.text_2).child(label)
        });
        let needs_key = p.info.needs_key || p.info.key_hint.is_some();
        let key_empty = self.key.read(cx).text().trim().is_empty();
        let warning = self.key_warning(&p, cx);
        let key_url = p.info.key_url.clone();
        let website = p.info.website.clone();
        let site_label = website.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string();
        let key_field: Option<AnyElement> = needs_key.then(|| {
            let label_row = div()
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child("API key"))
                .when_some(key_url.clone(), |d, url| {
                    d.child(Button::new("get-key", "Get a key").small().ghost().icon_after("external-link").color(t.accent_text).on_click(move |_, _, cx| cx.open_url(&url)))
                });
            let saved = p.key_preview.clone().map(|preview| {
                let source = match p.key_source {
                    KeySource::Env => format!("from {}", p.info.key_env.join(" / ")),
                    _ => format!("in the {}", crate::ui::keychain_name()),
                };
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
                    .child(div().font_family(MONO).text_color(t.text).child(masked(&preview)))
                    .child(div().flex_1().text_color(t.text_2).child(source))
                    .when(p.key_source == KeySource::Keychain, |d| {
                        d.child(Button::new("remove-key", "Remove").small().ghost().color(t.danger).on_click(cx.listener(|this, _, _, cx| this.remove_key(cx))))
                    })
            });
            div()
                .flex()
                .flex_col()
                .gap(px(7.))
                .child(label_row)
                .children(saved)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(secret_field("provider-key-masked", &self.key, window, cx))
                        .child(Button::new("save-key", "Save").primary().disabled(key_empty).on_click(cx.listener(|this, _, _, cx| this.save_key(cx)))),
                )
                .when_some(warning, |d, w| d.child(div().text_size(px(sz::SM)).text_color(t.warning).child(w)))
                .when_some(p.info.key_env.first().cloned(), |d, env| {
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .text_size(px(sz::SM))
                            .text_color(t.text_2)
                            .child("Or set")
                            .child(div().font_family(MONO).text_color(t.text).child(env))
                            .child("in your environment."),
                    )
                })
                .into_any_element()
        });
        let option = option_for(&id).map(|(_, label, _, help)| group(label, Some(help), self.option.clone().into_any_element(), cx));
        let check = self.check.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(9.))
                                    .child(crate::ui::logo(&p.info.id, px(22.)))
                                    .child(div().text_size(px(sz::XL)).font_weight(FontWeight::SEMIBOLD).child(p.info.name.clone())),
                            )
                            .child(div().text_color(t.text_2).child(p.info.tagline.clone())),
                    )
                    .child(div().w(px(150.)).flex_none().child(toggle_row(
                        "provider-enabled",
                        if p.settings.enabled { "Enabled" } else { "Disabled" },
                        if p.ready { "Ready" } else if !p.settings.enabled { "Hidden from pickers" } else { "Not ready" },
                        p.settings.enabled,
                        true,
                        move |on, _, cx| {
                            weak.update(cx, |this, cx| this.set_provider(json!({ "enabled": on }), cx)).ok();
                        },
                        cx,
                    ))),
            )
            .child(div().flex().flex_wrap().gap(px(5.)).mt(px(-6.)).children(tasks))
            .children(key_field)
            .when(p.info.base_url_editable, |d| d.child(group("Server address", Some("Saved when you leave the field."), self.base.clone().into_any_element(), cx)))
            .children(option)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(
                        Button::new("test-connection", if self.checking { "Testing…" } else { "Test connection" })
                            .with_icon(if self.checking { "loader-circle" } else { "plug-zap" })
                            .disabled(self.checking)
                            .on_click(cx.listener(|this, _, _, cx| this.test(cx))),
                    )
                    .when_some(check, |d, (ok, text)| d.child(div().flex_1().min_w_0().pt(px(6.)).child(note(if ok { "check" } else { "circle-alert" }, &text, if ok { t.success } else { t.danger }, cx)))),
            )
            .child(
                div().flex().child(
                    Button::new("provider-site", site_label)
                        .small()
                        .ghost()
                        .icon_after("external-link")
                        .on_click(move |_, _, cx| cx.open_url(&website)),
                ),
            )
            .into_any_element()
    }
}

fn option_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}
