//! lsuite AI in the window: the account card shown where the agent's provider is chosen (Settings ›
//! Agent, the first-run setup, the Agent panel when it isn't signed in yet). Signed out it says
//! "No setup. Sign in and your agent works." with **Sign in** (the browser) and a key to paste for
//! computers without one; signed in, the plan and the month's allowance (`Pro · 38 % used · resets
//! 1 Nov`), **Manage plan** and **Sign out**. Everything goes through `account.*` (the store's
//! `sign_in`, `sign_out`, `manage_plan`); the state is the store's (`Store::account`).

use gpui::{AnyElement, Context, Entity, FontWeight, Render, Subscription, Window, div, prelude::*, px};

use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, icon};

pub struct LsuiteCard {
    store: Entity<Store>,
    key: Entity<TextInput>,
    /// The key field is shown (after "Use a key").
    with_key: bool,
    /// One line, without the plan's allowance bar (the Agent panel's notice).
    pub compact: bool,
    _subs: Vec<Subscription>,
}

impl LsuiteCard {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let key = cx.new(|cx| {
            let mut i = TextInput::new(cx).placeholder("lsk_…");
            i.mono = true;
            i
        });
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe_in(&key, window, |this: &mut Self, _, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Submit) {
                    this.use_key(cx);
                }
                cx.notify();
            }),
        ];
        Self { store, key, with_key: false, compact: false, _subs: subs }
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    fn use_key(&mut self, cx: &mut Context<Self>) {
        let k = self.key.read(cx).text().trim().to_string();
        if k.is_empty() {
            return;
        }
        self.key.update(cx, |i, cx| i.set_text("", cx));
        self.with_key = false;
        self.store.update(cx, |s, cx| s.sign_in_with_key(k, cx));
    }
}

/// The lsuite mark, title and "Demo" tag.
fn title(cx: &gpui::App) -> AnyElement {
    let t = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(crate::ui::logo("lsuite", px(22.)))
        .child(div().text_size(px(sz::MD)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child("lsuite AI"))
        .child(div().px(px(5.)).border_1().border_color(t.line_strong).font_family(MONO).text_size(px(10.)).text_color(t.text_2).child("DEMO"))
        .into_any_element()
}

/// A square bar of the allowance used, in ink.
fn allowance_bar(percent: u32, cx: &gpui::App) -> AnyElement {
    let t = cx.theme();
    let fill = (percent.min(100) as f32) / 100.0;
    div()
        .h(px(6.))
        .w_full()
        .bg(t.bg_sunken)
        .border_1()
        .border_color(t.line)
        .child(div().h_full().w(gpui::relative(fill)).bg(if percent >= 100 { t.danger } else { t.ink }))
        .into_any_element()
}

impl Render for LsuiteCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let account = s.account.clone();
        let signing_in = s.signing_in;
        let compact = self.compact;
        let line = |text: String| div().text_size(px(sz::SM)).line_height(px(18.)).text_color(t.text_2).child(text);
        let mut col = div().flex().flex_col().gap(px(10.));
        if !compact {
            col = col.child(title(cx));
        }
        match account {
            None => col = col.child(line("Checking your lsuite account…".into())),
            Some(a) if signing_in && !a.signed_in => {
                col = col
                    .child(div().flex().items_center().gap(px(6.)).text_size(px(sz::SM)).child(icon("loader-circle").text_color(t.text_2)).child("Finish signing in in your browser…"))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(Button::new("lsuite-open-again", "Open the page again").small().icon_after("arrow-up-right").on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.sign_in(cx)))))
                            .child(Button::new("lsuite-cancel", "Cancel").small().ghost().on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.cancel_sign_in(cx))))),
                    );
            }
            Some(a) if !a.signed_in || a.expired => {
                let text = if a.expired {
                    "Your sign-in has expired. Sign in again and your agent works.".to_string()
                } else {
                    "No setup. Sign in and your agent works: Claude models on a monthly plan, for every lsuite app on this computer.".to_string()
                };
                col = col.child(line(text));
                if !compact {
                    col = col.child(div().text_size(px(sz::XS)).text_color(t.text_3).child("A demo for now: choosing a plan charges nothing."));
                }
                col = col.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(Button::new("lsuite-sign-in", "Sign in").primary().small().icon_after("arrow-up-right").on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.sign_in(cx)))))
                        .when(!self.with_key, |d| {
                            d.child(Button::new("lsuite-with-key", "Use a key").small().ghost().with_icon("key-round").tooltip("Paste the key your lsuite account page shows (lsk_…)").on_click(cx.listener(|this, _, window, cx| {
                                this.with_key = true;
                                crate::ui::input::focus(&this.key, window, cx);
                                cx.notify();
                            })))
                        }),
                );
                if self.with_key {
                    let empty = self.key.read(cx).text().trim().is_empty();
                    col = col.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1().min_w_0().child(self.key.clone()))
                            .child(Button::new("lsuite-key-go", "Sign in").small().disabled(empty).on_click(cx.listener(|this, _, _, cx| this.use_key(cx)))),
                    );
                }
            }
            Some(a) => {
                let who = if a.email.is_empty() { a.name.clone() } else { a.email.clone() };
                let percent = a.usage.as_ref().map(|u| u.percent());
                let out = a.usage.as_ref().is_some_and(|u| u.exhausted());
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(sz::SM)).text_color(t.text).child(who))
                        .child(div().flex_none().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(a.summary.clone())),
                );
                if let (Some(p), false) = (percent, compact) {
                    col = col.child(allowance_bar(p, cx));
                }
                if !a.can_run {
                    col = col.child(line("No plan with AI on this account yet. Choose one to start (a demo: nothing is charged).".into()));
                } else if out {
                    col = col.child(line("This month's allowance is used up. More comes with the next month, or with another plan.".into()));
                }
                if let Some(e) = a.error.clone() {
                    col = col.child(div().text_size(px(sz::XS)).text_color(t.text_2).child(e));
                }
                col = col.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child({
                            let b = Button::new("lsuite-manage", if a.can_run { "Manage plan" } else { "Choose a plan" }).small().icon_after("arrow-up-right");
                            let b = if !a.can_run || out { b.primary() } else { b };
                            b.on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.manage_plan(cx))))
                        })
                        .child(Button::new("lsuite-sign-out", "Sign out").small().ghost().on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.sign_out(cx)))))
                        .child(div().flex_1())
                        .child(Button::icon("lsuite-refresh", "refresh-cw", "Check the plan again").on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.refresh_account(cx))))),
                );
            }
        }
        let _ = window;
        col
    }
}
