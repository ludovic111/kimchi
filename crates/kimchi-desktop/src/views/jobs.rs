//! The Generations popover under the top bar's button: every job with its
//! model, prompt, progress and outcome; cancel, reveal, clear finished.
//! It also turns a failed job into an error toast, wherever it was started.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Anchor, Animation, AnimationExt, Bounds, Context, Entity, FontWeight, ObjectFit, Pixels, Render, Subscription, Window, anchored, deferred, div, img,
    prelude::*, px, relative,
};
use kimchi_gen::{Job, JobStatus, OutputKind};
use serde_json::json;

use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{GlassExt, icon};
use crate::views::generate::{ago, bounds_probe, image_path, short_duration};

pub struct JobsPopover {
    store: Entity<Store>,
    /// The top-bar button's bounds: a click there toggles, it doesn't close.
    anchor: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Last status seen per job, to report failures once.
    known: HashMap<String, JobStatus>,
    _sub: Subscription,
}

impl JobsPopover {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let sub = cx.observe(&store, |this, _, cx| {
            this.report_failures(cx);
            cx.notify();
        });
        let known = store.read(cx).jobs.iter().map(|j| (j.id.clone(), j.status)).collect();
        Self { store, anchor: Rc::new(Cell::new(None)), known, _sub: sub }
    }

    fn report_failures(&mut self, cx: &mut Context<Self>) {
        let mut failed = vec![];
        for j in &self.store.read(cx).jobs {
            let before = self.known.insert(j.id.clone(), j.status);
            if j.status == JobStatus::Failed && before != Some(JobStatus::Failed) {
                failed.push(j.error.clone().unwrap_or_else(|| format!("{} failed.", j.model_name)));
            }
        }
        for e in failed {
            self.store.update(cx, |s, cx| s.error(e, cx));
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            if s.jobs_open {
                s.jobs_open = false;
                s.sync_ui(cx);
                cx.notify();
            }
        });
    }

    fn row(&self, j: &Job, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let active = !j.status.is_done();
        let provider = s.providers.iter().find(|p| p.info.id == j.provider).map(|p| p.info.name.clone()).unwrap_or_else(|| j.provider.clone());
        let out = j.outputs.first();
        // A video output shows its library poster once it has landed.
        let poster = out.and_then(|o| match o.kind {
            OutputKind::Image => Some(o.path.clone()),
            _ => s.project.as_ref().and_then(|p| p.assets.iter().find(|a| a.path == o.path).and_then(|a| a.thumbnail.clone())),
        });
        let thumb = div()
            .flex_none()
            .w(px(52.))
            .h(px(36.))
            .rounded(px(sz::R_SM))
            .overflow_hidden()
            .bg(t.bg_sunken)
            .border_1()
            .border_color(t.line)
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.text_2)
            .child(match (poster, j.status) {
                (Some(p), _) => img(image_path(&p)).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                (None, JobStatus::Succeeded) => icon("film").into_any_element(),
                (None, JobStatus::Failed) => icon("circle-alert").text_color(t.danger).into_any_element(),
                (None, JobStatus::Cancelled) => icon("x").into_any_element(),
                (None, _) => div()
                    .size_full()
                    .bg(t.accent_soft)
                    .with_animation(gpui::ElementId::Name(format!("job-pulse-{}", j.id).into()), Animation::new(Duration::from_millis(1800)).repeat(), |d, x| {
                        d.opacity(0.4 + 0.6 * (x * std::f32::consts::PI).sin())
                    })
                    .into_any_element(),
            });

        let mut sub: Vec<String> = vec![j.model_name.clone(), provider];
        match j.status {
            JobStatus::Succeeded => {
                sub.push(short_duration(j.elapsed_ms as f64 / 1000.0));
                if let Some(c) = j.cost_usd {
                    sub.push(format!("${c:.3}"));
                }
            }
            JobStatus::Running | JobStatus::Queued => sub.push(short_duration((chrono::Utc::now() - j.created_at).num_milliseconds().max(0) as f64 / 1000.0)),
            _ => sub.push(ago(j.finished_at.unwrap_or(j.created_at))),
        }

        let progress = active.then(|| {
            let bar = div().absolute().top_0().bottom_0().rounded_full().bg(t.accent);
            let fill = match j.progress.fraction {
                Some(f) => bar.left_0().w(relative(f as f32)).into_any_element(),
                None => bar
                    .w(relative(0.4))
                    .with_animation(gpui::ElementId::Name(format!("job-slide-{}", j.id).into()), Animation::new(Duration::from_millis(1400)).repeat(), |d, x| {
                        d.left(relative(-0.4 + 1.4 * x))
                    })
                    .into_any_element(),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .mt(px(7.))
                .child(div().relative().h(px(3.)).rounded_full().overflow_hidden().bg(t.line).child(fill))
                .child(div().text_size(px(sz::XS)).text_color(t.text_2).child(j.progress.message.clone().unwrap_or_else(|| {
                    if j.status == JobStatus::Queued { "Queued".into() } else { "Working".into() }
                })))
        });

        let id = j.id.clone();
        let reveal = out.map(|o| std::path::PathBuf::from(&o.path));
        div()
            .id(gpui::ElementId::Name(format!("job-{}", j.id).into()))
            .flex()
            .items_start()
            .gap(px(10.))
            .p(px(8.))
            .rounded(px(sz::R_MD))
            .hover(|s| s.bg(t.hover))
            .child(thumb)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_size(px(12.5)).child(if j.request.prompt.is_empty() { "Untitled".to_string() } else { j.request.prompt.clone() }))
                    .child(div().mt(px(1.)).truncate().text_size(px(sz::XS)).text_color(t.text_2).child(sub.join(" · ")))
                    .children(progress)
                    .when_some(j.error.clone().filter(|_| !active), |d, e| d.child(div().mt(px(3.)).text_size(px(sz::XS)).text_color(t.danger).child(e))),
            )
            .when(active, |d| {
                d.child(
                    div()
                        .id(gpui::ElementId::Name(format!("job-cancel-{}", j.id).into()))
                        .flex_none()
                        .size(px(24.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(t.text_2)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).text_color(t.text))
                        .tooltip(|_, cx| crate::ui::tooltip("Cancel".into(), cx))
                        .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.run("generate.cancel", json!({ "jobId": id }), cx)))
                        .child(icon("square").size(px(11.))),
                )
            })
            .when(j.status == JobStatus::Succeeded, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(2.))
                        .when_some(reveal, |d, path| {
                            d.child(
                                div()
                                    .id(gpui::ElementId::Name(format!("job-reveal-{}", j.id).into()))
                                    .size(px(24.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(t.text_2)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.hover).text_color(t.text))
                                    .tooltip(|_, cx| crate::ui::tooltip(if cfg!(target_os = "macos") { "Show in Finder".into() } else { "Show in folder".into() }, cx))
                                    .on_click(move |_, _, cx| cx.reveal_path(&path))
                                    .child(icon("folder-open")),
                            )
                        })
                        .child(div().mt(px(1.)).child(icon("check").text_color(t.success))),
                )
            })
    }
}

impl Render for JobsPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let mut jobs = self.store.read(cx).jobs.clone();
        jobs.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        let any_done = jobs.iter().any(|j| j.status.is_done());
        let max_h = (f32::from(window.viewport_size().height) * 0.7).max(200.);
        let anchor = self.anchor.clone();
        let rows: Vec<gpui::AnyElement> = jobs.iter().map(|j| self.row(j, cx).into_any_element()).collect();
        let panel = div()
            .id("jobs-popover")
            .occlude()
            .mt(px(8.))
            .w(px(380.))
            .max_h(px(max_h))
            .flex()
            .flex_col()
            .rounded(px(sz::R_LG))
            .glass(t.glass2)
            .bg(crate::views::generate::popover_fill(cx))
            .shadow(t.glass_shadow())
            .overflow_hidden()
            .text_size(px(sz::BASE))
            .text_color(t.text)
            .cursor_default()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                if anchor.get().is_some_and(|b| b.contains(&e.position)) {
                    return;
                }
                this.close(cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Generations"))
                    .when(any_done, |d| {
                        d.child(
                            div()
                                .id("jobs-clear")
                                .text_size(px(11.5))
                                .text_color(t.text_2)
                                .cursor_pointer()
                                .hover(|s| s.text_color(t.text))
                                .child("Clear finished")
                                .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.run("generate.clearFinished", json!({}), cx))),
                        )
                    }),
            )
            .when(jobs.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(14.))
                        .pt(px(4.))
                        .pb(px(16.))
                        .text_size(px(12.5))
                        .text_color(t.text_2)
                        .child("Nothing yet. Write a prompt in the Generate panel: results land in your library and on the timeline."),
                )
            })
            .child(div().id("jobs-list").flex_1().min_h_0().overflow_y_scroll().px(px(6.)).pb(px(6.)).children(rows));

        div()
            .absolute()
            .inset_0()
            .child(bounds_probe(self.anchor.clone()))
            .child(div().absolute().top(relative(1.)).right(px(-60.)).child(deferred(anchored().anchor(Anchor::TopRight).snap_to_window_with_margin(px(8.)).child(crate::ui::motion::enter(panel.relative(), "jobs-in", crate::ui::motion::FAST, (0., -6.)))).with_priority(3)))
    }
}
