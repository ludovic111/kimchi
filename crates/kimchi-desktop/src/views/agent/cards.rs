//! What the conversation shows: the person's prompts, the agent's reply, one
//! card per command (name, parameters, outcome, who ran it, when) and the end
//! of each run with "Revert this run".

use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*, px};
use kimchi_agent::{RunInfo, RunState};
use kimchi_control::{CommandRecord, Source};
use serde_json::Value;

use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, icon};
use crate::views::agent::{clock, params_summary, source_badge, source_label, tokens};
use crate::views::agent_panel::AgentPanel;

/// How a run ended, as the conversation's entry for it says.
pub struct Ending<'a> {
    pub run: u64,
    pub state: RunState,
    pub error: Option<&'a str>,
    pub changes: usize,
    pub seconds: f64,
    pub tokens: u64,
}

/// Most characters of a command's answer shown in an open card.
const RESULT_PREVIEW: usize = 1600;

impl AgentPanel {
    /// A request; one sent from the CLI or an MCP client says so.
    pub(crate) fn user_bubble(&self, i: usize, text: &str, source: Source, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        div()
            .id(("user", i))
            .flex()
            .justify_end()
            .items_start()
            .gap(px(6.))
            .when(source != Source::Window, |d| d.child(div().mt(px(9.)).child(source_badge(source_label(source), cx))))
            .child(
                div()
                    .max_w(gpui::relative(0.88))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(sz::R_LG))
                    .bg(t.accent_soft)
                    .border_1()
                    .border_color(t.accent_ring)
                    .text_size(px(sz::BASE))
                    .line_height(px(19.))
                    .child(text.to_string()),
            )
            .into_any_element()
    }

    pub(crate) fn assistant_text(&self, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_size(px(sz::BASE))
            .line_height(px(19.))
            .text_color(t.text)
            .children(text.split("\n\n").filter(|p| !p.trim().is_empty()).map(|p| div().child(p.trim().to_string())))
            .into_any_element()
    }

    /// One command: what ran, with what, whether it worked, who ran it and when. Click to see the parameters and the answer.
    pub(crate) fn command_card(&self, record: &CommandRecord, result: Option<&Value>, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let seq = record.seq;
        let open = self.expanded.contains(&seq);
        let summary = params_summary(&record.params);
        let (status_icon, status_color) = if record.ok { ("check", t.success) } else { ("circle-alert", t.danger) };
        let quiet = !record.mutates && record.ok;
        div()
            .id(("command", seq))
            .flex()
            .flex_col()
            .gap(px(3.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(sz::R_MD))
            .bg(t.bg_sunken.opacity(if quiet { 0.3 } else { 0.55 }))
            .border_1()
            .border_color(if record.ok { t.line } else { t.danger.opacity(0.45) })
            .cursor_pointer()
            .hover(|s| s.border_color(t.line_strong))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(&seq) {
                    this.expanded.insert(seq);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(icon(status_icon).size(px(12.)).text_color(status_color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .text_size(px(sz::SM))
                            .font_weight(if quiet { FontWeight::NORMAL } else { FontWeight::SEMIBOLD })
                            .text_color(if quiet { t.text_2 } else { t.text })
                            .child(record.command.clone()),
                    )
                    .child(source_badge(source_label(record.source), cx))
                    .child(div().flex_none().font_family(MONO).text_size(px(10.)).text_color(t.text_2).child(clock(record.at)))
                    .child(icon(if open { "chevron-up" } else { "chevron-down" }).size(px(12.)).text_color(t.text_2)),
            )
            .when(!summary.is_empty() && !open, |d| d.child(div().pl(px(19.)).truncate().font_family(MONO).text_size(px(10.5)).text_color(t.text_2).child(summary)))
            .when_some(seen_picture(record, result), |d, path| {
                // What the agent looked at (a frame, a media look), as it saw it.
                d.child(
                    div()
                        .mt(px(4.))
                        .h(px(150.))
                        .w_full()
                        .rounded(px(sz::R_SM))
                        .overflow_hidden()
                        .bg(t.bg_sunken.opacity(0.8))
                        .border_1()
                        .border_color(t.line)
                        .child(gpui::img(path).size_full().object_fit(gpui::ObjectFit::Contain)),
                )
            })
            .when_some(record.error.clone(), |d, e| d.child(div().pl(px(19.)).text_size(px(sz::XS)).text_color(t.danger).child(e)))
            .when(open, |d| {
                let params = serde_json::to_string_pretty(&record.params).unwrap_or_default();
                let answer = result.map(|r| {
                    let s = serde_json::to_string_pretty(r).unwrap_or_default();
                    if s.chars().count() > RESULT_PREVIEW { format!("{}…", s.chars().take(RESULT_PREVIEW).collect::<String>()) } else { s }
                });
                d.child(code_block("Parameters", params, cx)).when_some(answer, |d, a| d.child(code_block("Answer", a, cx)))
            })
            .into_any_element()
    }

    pub(crate) fn outcome_row(&self, i: usize, o: Ending, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let run: Option<&RunInfo> = self.snap.run(o.run);
        let can_revert = run.is_some_and(RunInfo::can_revert);
        let reverted = run.is_some_and(|r| r.reverted);
        let mut facts = vec![];
        facts.push(match o.changes {
            0 => "no changes".to_string(),
            1 => "1 change".to_string(),
            n => format!("{n} changes"),
        });
        facts.push(format!("{:.0}s", o.seconds.max(1.0)));
        if o.tokens > 0 {
            facts.push(format!("{} tokens", tokens(o.tokens)));
        }
        let (ic, color, title): (&'static str, gpui::Hsla, SharedString) = match o.state {
            RunState::Done | RunState::Running => ("circle-check", t.success, "Done".into()),
            RunState::Error => ("circle-alert", t.danger, "Stopped on an error".into()),
            RunState::Cancelled => ("circle-stop", t.text_2, if o.changes > 0 { "Stopped; finished edits stay".into() } else { "Stopped".into() }),
        };
        let run_ix = o.run;
        div()
            .id(("outcome", i))
            .flex()
            .flex_col()
            .gap(px(6.))
            .when(o.state == RunState::Error, |d| {
                d.p(px(10.)).rounded(px(sz::R_MD)).bg(t.danger.opacity(0.08)).border_1().border_color(t.danger.opacity(0.35))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(sz::SM))
                    .child(icon(ic).text_color(color))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().flex_1().min_w_0().truncate().text_color(t.text_2).child(facts.join(" · ")))
                    .when(can_revert, |d| {
                        d.child(
                            Button::new(("revert-run", i), "Revert this run")
                                .small()
                                .ghost()
                                .with_icon("rotate-ccw")
                                .tooltip("Put the project back as it was before this run (one undo step)")
                                .on_click(cx.listener(move |this, _, _, cx| this.revert_run(run_ix, cx))),
                        )
                    })
                    .when(reverted, |d| d.child(div().flex_none().text_size(px(sz::XS)).text_color(t.text_2).child("Reverted"))),
            )
            .when_some(o.error.map(str::to_string), |d, m| d.child(div().text_size(px(sz::SM)).text_color(t.text).child(m)))
            .into_any_element()
    }
}

/// The picture a command answered with, when it is one the agent is shown and it is still there.
fn seen_picture(record: &CommandRecord, result: Option<&Value>) -> Option<std::path::PathBuf> {
    if !record.ok {
        return None;
    }
    let answer = result.or(record.result.as_ref())?;
    kimchi_control::vision::pictures_in(&record.command, answer).into_iter().next().filter(|p| p.is_file())
}

fn code_block(title: &str, body: String, cx: &gpui::App) -> AnyElement {
    let t = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .mt(px(4.))
        .child(crate::ui::caps(title.to_string(), cx))
        .child(
            div()
                .p(px(8.))
                .rounded(px(sz::R_SM))
                .bg(t.bg_sunken.opacity(0.8))
                .border_1()
                .border_color(t.line)
                .font_family(MONO)
                .text_size(px(10.5))
                .line_height(px(15.))
                .text_color(t.text)
                .children(body.lines().map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.to_string() }))),
        )
        .into_any_element()
}
