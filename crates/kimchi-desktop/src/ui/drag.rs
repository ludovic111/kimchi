//! Window-wide pointer tracking for drags (scrubbing, trimming, moving clips,
//! resizing panels): a div only hears moves while it is hovered, so while a
//! drag is on, this invisible element listens to the whole window.

use gpui::{AnyElement, App, DispatchPhase, Entity, IntoElement, MouseMoveEvent, MouseUpEvent, Styled, Window, canvas};

/// Renders nothing; while present, every mouse move and the release anywhere
/// in the window go to `on_move` / `on_up` on `entity`.
pub fn track<V: 'static>(
    entity: Entity<V>,
    on_move: impl Fn(&mut V, &MouseMoveEvent, &mut Window, &mut gpui::Context<V>) + 'static,
    on_up: impl Fn(&mut V, &MouseUpEvent, &mut Window, &mut gpui::Context<V>) + 'static,
) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |_, _, window: &mut Window, _cx: &mut App| {
            let e = entity.clone();
            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture {
                    e.update(cx, |v, cx| on_move(v, ev, window, cx));
                }
            });
            let e = entity.clone();
            window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture {
                    e.update(cx, |v, cx| on_up(v, ev, window, cx));
                }
            });
        },
    )
    .absolute()
    .size_0()
    .into_any_element()
}
