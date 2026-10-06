//! The Changes tab: the panel's runs with "Revert this run", and the shared
//! undo history (`history.list`) with who made each step. Edits by the person
//! fold into one line between the agents' steps.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use serde_json::json;

use crate::store::StoreExt;
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, caps, icon};
use crate::views::agent::source_badge;
use crate::views::agent_panel::AgentPanel;

impl AgentPanel {
    pub(crate) fn changes_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let history = self.history.clone().unwrap_or_default();
        let agent_steps = history.undo.iter().filter(|s| s.source != "window").count();
        let runs: Vec<kimchi_agent::RunInfo> = self.snap.runs.iter().rev().cloned().collect();

        // Undo steps newest first; consecutive steps by the person fold into one line.
        let mut rows: Vec<AnyElement> = vec![];
        let mut mine = 0usize;
        let flush = |mine: &mut usize, rows: &mut Vec<AnyElement>, key: usize| {
            if *mine > 0 {
                rows.push(
                    div()
                        .id(("mine", key))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(4.))
                        .text_size(px(sz::XS))
                        .text_color(t.text_2)
                        .child(div().w(px(12.)).flex().justify_center().child(div().size(px(4.)).bg(t.line_strong)))
                        .child(if *mine == 1 { "1 edit of yours".to_string() } else { format!("{} edits of yours", *mine) })
                        .into_any_element(),
                );
                *mine = 0;
            }
        };
        for (i, step) in history.undo.iter().enumerate().take(200) {
            if step.source == "window" {
                mine += 1;
                continue;
            }
            flush(&mut mine, &mut rows, i);
            rows.push(
                div()
                    .id(("step", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(sz::R_SM))
                    .when(i == 0, |d| d.bg(t.hover))
                    .child(icon("history").size(px(12.)).text_color(t.text_2))
                    .child(div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(sz::SM)).child(step.label.clone()))
                    .child(source_badge(&step.source, cx))
                    .when(i == 0, |d| d.child(div().text_size(px(10.)).text_color(t.text_2).child("last")))
                    .into_any_element(),
            );
        }
        flush(&mut mine, &mut rows, usize::MAX / 2);

        let top = history.undo.first().map(|s| format!("Undo {} ({})", s.label, if s.source == "window" { "yours" } else { s.source.as_str() }));
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(12.))
            .child(
                div()
                    .text_size(px(sz::SM))
                    .text_color(t.text_2)
                    .line_height(px(18.))
                    .child("Agents, MCP clients and the CLI edit through the same commands as you, into one undo history. Revert a whole run, or undo step by step."),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(caps("Runs from this panel", cx))
                    .when(runs.is_empty(), |d| d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child("None yet.")))
                    .children(runs.into_iter().map(|r| {
                        let i = r.id as usize;
                        let changes = match r.changes {
                            0 => "no changes".to_string(),
                            1 => "1 change".to_string(),
                            n => format!("{n} changes"),
                        };
                        div()
                            .id(("run", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .p(px(8.))
                            .rounded(px(sz::R_MD))
                            .bg(t.bg_sunken.opacity(0.5))
                            .border_1()
                            .border_color(t.line)
                            .child(crate::ui::logo(r.provider.id(), px(16.)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(div().truncate().text_size(px(sz::SM)).font_weight(FontWeight::MEDIUM).child(r.prompt.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(sz::XS))
                                            .text_color(t.text_2)
                                            .child(format!(
                                                "{} · {} · {}",
                                                r.started_at.with_timezone(&chrono::Local).format("%H:%M"),
                                                r.provider.label(),
                                                if r.finished() { changes } else { "running…".into() }
                                            )),
                                    ),
                            )
                            .when(r.reverted, |d| d.child(div().text_size(px(sz::XS)).text_color(t.text_2).child("Reverted")))
                            .when(r.can_revert(), |d| {
                                let id = r.id;
                                d.child(
                                    Button::new(("changes-revert", i), "Revert this run")
                                        .small()
                                        .with_icon("rotate-ccw")
                                        .on_click(cx.listener(move |this, _, _, cx| this.revert_run(id, cx))),
                                )
                            })
                    })),
            )
            .children(self.outside_sessions(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(caps(format!("Undo history · {agent_steps} by agents"), cx))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(2.))
                                    .child(
                                        Button::icon("changes-undo", "undo-2", top.unwrap_or_else(|| "Nothing to undo".into()))
                                            .disabled(!history.can_undo)
                                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("history.undo", json!({}), cx))),
                                    )
                                    .child(
                                        Button::icon("changes-redo", "redo-2", "Redo")
                                            .disabled(!history.can_redo)
                                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("history.redo", json!({}), cx))),
                                    ),
                            ),
                    )
                    .when(self.history.is_none(), |d| d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Open a project to see its history.")))
                    .when(self.history.is_some() && agent_steps == 0, |d| {
                        d.child(div().text_size(px(sz::SM)).text_color(t.text_2).child("No step by an agent, MCP or the CLI in this project's history."))
                    })
                    .when(agent_steps > 0, |d| d.child(div().flex().flex_col().gap(px(1.)).children(rows)))
                    .when(!history.redo.is_empty(), |d| {
                        d.child(div().text_size(px(sz::XS)).text_color(t.text_2).child(format!(
                            "{} step{} can be redone.",
                            history.redo.len(),
                            if history.redo.len() == 1 { "" } else { "s" }
                        )))
                    }),
            )
            .into_any_element()
    }
}

impl AgentPanel {
    /// Agents working from a terminal (Claude Code, Codex, any MCP client): their commands carry the
    /// checkpoint the bridge took before their first change, so each session reverts in one click.
    fn outside_sessions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = cx.theme().clone();
        let store = self.store.read(cx);
        // The built-in agent's own commands (Claude Code and Codex reach kimchi over MCP too)
        // belong to its runs, above.
        let ours: std::collections::HashSet<u64> = self
            .snap
            .entries
            .iter()
            .filter_map(|e| match e {
                kimchi_agent::Entry::Command { record, run: Some(_), .. } => Some(record.seq),
                _ => None,
            })
            .collect();
        let mut sessions: Vec<(u64, kimchi_control::Source, usize, chrono::DateTime<chrono::Utc>)> = vec![];
        for r in store.commands.iter().filter(|r| r.mutates && r.ok && !ours.contains(&r.seq)) {
            let Some(cp) = r.checkpoint else { continue };
            match sessions.iter_mut().find(|s| s.0 == cp) {
                Some(s) => s.2 += 1,
                None => sessions.push((cp, r.source, 1, r.at)),
            }
        }
        if sessions.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(caps("From a terminal", cx))
                .children(sessions.into_iter().rev().map(|(cp, source, n, at)| {
                    let reverted = self.reverted_sessions.contains(&cp);
                    div()
                        .id(("outside-session", cp as usize))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .p(px(8.))
                        .rounded(px(sz::R_MD))
                        .bg(t.bg_sunken.opacity(0.5))
                        .border_1()
                        .border_color(t.line)
                        .child(source_badge(source.as_str(), cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(sz::XS))
                                .text_color(t.text_2)
                                .child(format!("since {} · {n} change{}", at.with_timezone(&chrono::Local).format("%H:%M"), if n == 1 { "" } else { "s" })),
                        )
                        .when(reverted, |d| d.child(div().text_size(px(sz::XS)).text_color(t.text_2).child("Reverted")))
                        .when(!reverted, |d| {
                            d.child(
                                Button::new(("outside-revert", cp as usize), "Revert this session")
                                    .small()
                                    .with_icon("rotate-ccw")
                                    .on_click(cx.listener(move |this, _, _, cx| this.revert_session(cp, cx))),
                            )
                        })
                }))
                .into_any_element(),
        )
    }

    fn revert_session(&mut self, checkpoint: u64, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.run_then("history.revertTo", json!({ "checkpoint": checkpoint }), cx, |s, _, cx| s.info("Reverted the session: one undo brings it back.", cx))
        });
        self.reverted_sessions.insert(checkpoint);
        cx.notify();
    }
}
