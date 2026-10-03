//! Settings › Diagnostics: the log of this run (`app.logs`), how much goes in it
//! (`diagnostics.logLevel`), crash reports (`app.crashReports`, `app.clearCrashReports`) and
//! what a bug report needs (`app.diagnostics`). Everything stays on this computer; the person
//! copies or opens what they want to share.

use gpui::{AnyElement, ClipboardItem, Context, FontWeight, div, prelude::*, px};
use kimchi_control::diagnostics::Report;
use serde_json::{Value, json};

use super::{SettingsDialog, group, note};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, icon, segmented};

#[derive(Default)]
pub(super) struct DiagState {
    reports: Vec<Report>,
    log: Vec<String>,
    log_file: Option<String>,
    /// The report shown in full: (id, text).
    open: Option<(String, String)>,
    loaded: bool,
}

impl SettingsDialog {
    /// Reads the log's last lines and the crash reports (on showing the section, and on refresh).
    pub(super) fn load_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.run("app.logs", json!({ "lines": 60 }), cx, |this, v, cx| {
            this.diag.log = v["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(str::to_string)).collect();
            this.diag.log_file = v["file"].as_str().map(str::to_string);
            cx.notify();
        });
        self.run("app.crashReports", json!({}), cx, |this, v, cx| {
            this.diag.reports = serde_json::from_value(v["reports"].clone()).unwrap_or_default();
            this.diag.loaded = true;
            cx.notify();
        });
    }

    fn open_report(&mut self, id: String, cx: &mut Context<Self>) {
        if self.diag.open.as_ref().is_some_and(|(open, _)| *open == id) {
            self.diag.open = None;
            cx.notify();
            return;
        }
        self.run("app.crashReports", json!({ "id": id }), cx, |this, v, cx| {
            if let (Some(id), Some(text)) = (v["id"].as_str(), v["text"].as_str()) {
                this.diag.open = Some((id.to_string(), text.to_string()));
                cx.notify();
            }
        });
    }

    fn copy_system_info(&mut self, cx: &mut Context<Self>) {
        self.run("app.diagnostics", json!({}), cx, |this, v, cx| {
            this.copy("copy-diagnostics", summary(&v), cx);
        });
    }

    pub(super) fn diagnostics_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let data_dir = self.store.read(cx).session.data_dir.clone();
        let logs = kimchi_control::diagnostics::logs_dir(&data_dir);
        let crashes = kimchi_control::diagnostics::crashes_dir(&data_dir);
        let level = self.store.read(cx).settings.diagnostics.log_level.clone();
        let env_level = std::env::var("RUST_LOG").ok().filter(|v| !v.trim().is_empty());
        let weak = cx.entity().downgrade();
        let levels = segmented(
            "log-level",
            vec![("info".to_string(), "Normal".into()), ("debug".to_string(), "Detailed".into()), ("trace".to_string(), "Everything".into())],
            level,
            move |v, _, cx| {
                let v = v.clone();
                weak.update(cx, |this, cx| this.set_setting("diagnostics.logLevel", json!(v), cx)).ok();
            },
            cx,
        );
        let log_text = if self.diag.log.is_empty() { "Nothing in the log yet.".to_string() } else { self.diag.log.join("\n") };
        let copy_log = self.diag.log.join("\n");
        let copied_log = self.copied == Some("copy-log");
        let copied_info = self.copied == Some("copy-diagnostics");

        let log = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .id("log-tail")
                    .max_h(px(150.))
                    .overflow_y_scroll()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(sz::R_SM))
                    .bg(t.bg_sunken)
                    .border_1()
                    .border_color(t.line)
                    .font_family(MONO)
                    .text_size(px(sz::XS))
                    .text_color(t.text_2)
                    .child(log_text),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(Button::new("open-logs", "Open logs folder").small().with_icon("folder-open").on_click(move |_, _, cx| cx.open_with_system(&logs)))
                    .child(
                        Button::new("copy-log", if copied_log { "Copied" } else { "Copy recent lines" })
                            .small()
                            .ghost()
                            .with_icon(if copied_log { "check" } else { "copy" })
                            .on_click(cx.listener(move |this, _, _, cx| this.copy("copy-log", copy_log.clone(), cx))),
                    )
                    .child(Button::new("refresh-logs", "Refresh").small().ghost().with_icon("refresh-cw").on_click(cx.listener(|this, _, _, cx| this.load_diagnostics(cx)))),
            );

        let mut reports: Vec<AnyElement> = vec![];
        for (i, r) in self.diag.reports.iter().enumerate() {
            let open = self.diag.open.as_ref().filter(|(id, _)| *id == r.id).map(|(_, text)| text.clone());
            let id = r.id.clone();
            let (ic, color, what) = if r.kind == "unclean" { ("circle-stop", t.warning, "Didn't quit properly") } else { ("bug", t.danger, "Crash") };
            let local = r.at.with_timezone(&chrono::Local).format("%b %-d, %H:%M").to_string();
            let copy_text = open.clone();
            let path = r.path.clone();
            reports.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .py(px(8.))
                    .when(i > 0, |d| d.border_t_1().border_color(t.line))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(10.))
                            .child(icon(ic).mt(px(2.)).text_color(color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.))
                                    .child(div().flex().gap(px(6.)).child(div().font_weight(FontWeight::MEDIUM).child(what)).child(div().text_color(t.text_3).child(local)))
                                    .child(div().text_size(px(sz::SM)).text_color(t.text_2).truncate().child(r.summary.clone())),
                            )
                            .child(
                                Button::new(("report-view", i), if open.is_some() { "Hide" } else { "View" })
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| this.open_report(id.clone(), cx))),
                            ),
                    )
                    .when_some(open, |d, text| {
                        d.child(
                            div()
                                .id(("report-text", i))
                                .max_h(px(220.))
                                .overflow_y_scroll()
                                .px(px(10.))
                                .py(px(8.))
                                .rounded(px(sz::R_SM))
                                .bg(t.bg_sunken)
                                .border_1()
                                .border_color(t.line)
                                .font_family(MONO)
                                .text_size(px(sz::XS))
                                .child(text),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(Button::new(("report-copy", i), "Copy report").small().with_icon("copy").on_click(move |_, _, cx| {
                                    if let Some(text) = copy_text.clone() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                                    }
                                }))
                                .child(Button::new(("report-show", i), crate::ui::reveal_label()).small().ghost().with_icon("folder-search").on_click(move |_, _, cx| cx.reveal_path(&path))),
                        )
                    })
                    .into_any_element(),
            );
        }
        let has_reports = !reports.is_empty();
        let crash_body: AnyElement = if !self.diag.loaded {
            div().text_size(px(sz::SM)).text_color(t.text_2).child("Loading…").into_any_element()
        } else if !has_reports {
            note("circle-check", "No crash reports. kimchi hasn't crashed on this computer.", t.success, cx)
        } else {
            div()
                .flex()
                .flex_col()
                .child(div().flex().flex_col().children(reports))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .pt(px(4.))
                        .child(Button::new("open-crashes", "Open folder").small().ghost().with_icon("folder-open").on_click(move |_, _, cx| cx.open_with_system(&crashes)))
                        .child(Button::new("clear-crashes", "Delete all").small().ghost().with_icon("trash").on_click(cx.listener(|this, _, _, cx| {
                            this.run("app.clearCrashReports", json!({}), cx, |this, _, cx| {
                                this.diag.reports.clear();
                                this.diag.open = None;
                                cx.notify();
                            })
                        }))),
                )
                .into_any_element()
        };
        let session = self.store.read(cx).session.clone();

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(note("shield-check", "Logs and reports stay on this computer. Nothing is sent anywhere: copy or attach what you want to share.", t.text_2, cx))
            .child(group(
                "Crash reports",
                Some("Written when kimchi runs into a bug, or finds at start that it didn't quit properly last time."),
                crash_body,
                cx,
            ))
            .child(group("This run's log", self.diag.log_file.as_deref(), log.into_any_element(), cx))
            .child(group(
                "Detail in the log",
                Some("Detailed is the default; Everything helps when chasing a problem, and makes the log much bigger."),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().w(px(330.)).child(levels))
                    .when_some(env_level, |d, v| d.child(note("info", &format!("RUST_LOG={v} is set, so it decides instead."), t.text_2, cx)))
                    .into_any_element(),
                cx,
            ))
            .child(group(
                "Report a problem",
                Some("The system details below say which kimchi, which computer and which ffmpeg; they hold no keys, prompts or projects."),
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        Button::new("copy-diagnostics", if copied_info { "Copied" } else { "Copy system details" })
                            .small()
                            .with_icon(if copied_info { "check" } else { "copy" })
                            .on_click(cx.listener(|this, _, _, cx| this.copy_system_info(cx))),
                    )
                    .child(Button::new("open-issue", "Open an issue on GitHub").small().ghost().icon_after("arrow-up-right").on_click(move |_, _, cx| cx.open_url(&crate::app::issue_url(&session))))
                    .into_any_element(),
                cx,
            ))
            .into_any_element()
    }
}

/// `app.diagnostics` as lines a person can paste into an issue.
fn summary(v: &Value) -> String {
    let s = |k: &str| match &v[k] {
        Value::Null => "—".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let mut out = format!(
        "kimchi {}\nSystem: {} ({})\nInstall: {}\nffmpeg: {}\n3D renderer: {}\nLog level: {}\nLog: {}\nAgent: {}\n",
        s("version"),
        s("system"),
        s("arch"),
        s("install"),
        s("ffmpeg"),
        s("renderer3d"),
        s("logLevel"),
        s("logFile"),
        s("agentProvider")
    );
    if let Some(reports) = v["crashReports"].as_array().filter(|r| !r.is_empty()) {
        out.push_str("Recent crash reports:\n");
        for r in reports {
            out.push_str(&format!("- {} {}: {}\n", r["at"].as_str().unwrap_or(""), r["kind"].as_str().unwrap_or(""), r["summary"].as_str().unwrap_or("")));
        }
    }
    out
}
