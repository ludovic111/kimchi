//! A text field: single line or wrapped multi-line, with IME, selection,
//! clipboard and the usual keys. Adapted from GPUI's `examples/input.rs`.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::{ActiveTheme, size as sz};

actions!(
    text_input,
    [Backspace, Delete, Left, Right, Up, Down, SelectLeft, SelectRight, SelectUp, SelectDown, SelectAll, Home, End, WordLeft, WordRight, DeleteWordLeft, Paste, Cut, Copy, Newline, Submit, Cancel, ShowCharacterPalette]
);

/// Focuses a text field.
pub fn focus(input: &Entity<TextInput>, window: &mut Window, cx: &mut App) {
    let handle = input.read(cx).focus_handle.clone();
    window.focus(&handle, cx);
}

pub fn bindings() -> Vec<KeyBinding> {
    let ctx = Some("TextInput");
    #[cfg(target_os = "macos")]
    let m = "cmd";
    #[cfg(not(target_os = "macos"))]
    let m = "ctrl";
    vec![
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new(&format!("{m}-a"), SelectAll, ctx),
        KeyBinding::new(&format!("{m}-v"), Paste, ctx),
        KeyBinding::new(&format!("{m}-c"), Copy, ctx),
        KeyBinding::new(&format!("{m}-x"), Cut, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new(&format!("{m}-left"), Home, ctx),
        KeyBinding::new(&format!("{m}-right"), End, ctx),
        KeyBinding::new("alt-left", WordLeft, ctx),
        KeyBinding::new("alt-right", WordRight, ctx),
        KeyBinding::new("alt-backspace", DeleteWordLeft, ctx),
        KeyBinding::new("shift-enter", Newline, ctx),
        KeyBinding::new("enter", Submit, ctx),
        KeyBinding::new(&format!("{m}-enter"), Submit, ctx),
        KeyBinding::new("escape", Cancel, ctx),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, ctx),
    ]
}

/// What the field tells its owner.
#[derive(Clone, Debug)]
pub enum InputEvent {
    Changed(String),
    /// Enter (single line), or cmd-Enter / Enter (multi-line, see `submit_on_enter`).
    Submit,
    Cancel,
    Blur,
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// One wrapped layout per logical line (split on `\n`).
    last_lines: Vec<WrappedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_line_height: Pixels,
    is_selecting: bool,
    pub multiline: bool,
    /// In a multi-line field, Enter submits and shift-Enter adds a line (else Enter adds a line).
    pub submit_on_enter: bool,
    pub mono: bool,
    pub disabled: bool,
    /// Lines a multi-line field shows at least (it grows with its content).
    pub min_lines: usize,
    /// No fill or border of its own (it sits inside a composer that draws them).
    pub bare: bool,
    _blur: Option<gpui::Subscription>,
}

impl EventEmitter<InputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            focus_handle,
            content: String::new(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_lines: vec![],
            last_bounds: None,
            last_line_height: px(18.),
            is_selecting: false,
            multiline: false,
            submit_on_enter: false,
            mono: false,
            disabled: false,
            min_lines: 1,
            bare: false,
            _blur: None,
        }
    }

    pub fn multiline(mut self, min_lines: usize) -> Self {
        self.multiline = true;
        self.min_lines = min_lines.max(1);
        self
    }

    pub fn placeholder(mut self, p: impl Into<SharedString>) -> Self {
        self.placeholder = p.into();
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Replaces the text (no `Changed` event).
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if text == self.content {
            return;
        }
        self.content = if self.multiline { text } else { text.replace('\n', " ") };
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        cx.notify();
    }

    pub fn set_placeholder(&mut self, p: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = p.into();
        cx.notify();
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }


    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Changed(self.content.clone()));
        cx.notify();
    }

    // ---- actions ----------------------------------------------------------

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical(self.cursor_offset(), -1).unwrap_or(0);
        self.move_to(target, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical(self.cursor_offset(), 1).unwrap_or(self.content.len());
        self.move_to(target, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical(self.cursor_offset(), -1).unwrap_or(0);
        self.select_to(target, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical(self.cursor_offset(), 1).unwrap_or(self.content.len());
        self.select_to(target, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_text(cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor_offset();
        let start = if self.multiline { self.content[..c].rfind('\n').map(|i| i + 1).unwrap_or(0) } else { 0 };
        self.move_to(start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor_offset();
        let end = if self.multiline { self.content[c..].find('\n').map(|i| c + i).unwrap_or(self.content.len()) } else { self.content.len() };
        self.move_to(end, cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor_offset()), cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_word(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.select_to(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline && !self.submit_on_enter && !window.modifiers().platform {
            self.replace_text_in_range(None, "\n", window, cx);
            return;
        }
        cx.emit(InputEvent::Submit);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.disabled {
            return;
        }
        window.focus(&self.focus_handle, cx);
        self.is_selecting = true;
        let at = self.index_for_mouse_position(event.position);
        if event.click_count >= 2 {
            let (s, e) = self.word_at(at);
            self.selected_range = s..e;
            self.selection_reversed = false;
            cx.notify();
        } else if event.modifiers.shift {
            self.select_to(at, cx);
        } else {
            self.move_to(at, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(&mut self, _: &ShowCharacterPalette, window: &mut Window, _: &mut Context<Self>) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = if self.multiline { text } else { text.replace('\n', " ") };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    // ---- offsets ----------------------------------------------------------

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed { self.selected_range.start = offset } else { self.selected_range.end = offset };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(idx, _)| (idx < offset).then_some(idx)).unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).find_map(|(idx, _)| (idx > offset).then_some(idx)).unwrap_or(self.content.len())
    }

    fn previous_word(&self, offset: usize) -> usize {
        self.content.unicode_word_indices().rev().find_map(|(idx, _)| (idx < offset).then_some(idx)).unwrap_or(0)
    }

    fn next_word(&self, offset: usize) -> usize {
        self.content.unicode_word_indices().find_map(|(idx, w)| (idx + w.len() > offset).then_some(idx + w.len())).unwrap_or(self.content.len())
    }

    fn word_at(&self, offset: usize) -> (usize, usize) {
        self.content
            .unicode_word_indices()
            .find(|(idx, w)| *idx <= offset && offset <= idx + w.len())
            .map(|(idx, w)| (idx, idx + w.len()))
            .unwrap_or((offset, offset))
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    // ---- layout lookups ---------------------------------------------------

    fn line_starts(&self) -> Vec<usize> {
        line_starts(&self.content)
    }

    /// Position of `offset` relative to the text's top-left.
    fn position_for(&self, offset: usize) -> Option<Point<Pixels>> {
        position_for(&self.last_lines, &self.line_starts(), offset, self.last_line_height)
    }

    /// Offset nearest to a point relative to the text's top-left.
    fn offset_for(&self, p: Point<Pixels>) -> usize {
        offset_for(&self.last_lines, &self.line_starts(), p, self.last_line_height, self.content.len())
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else { return 0 };
        if self.content.is_empty() {
            return 0;
        }
        self.offset_for(point(position.x - bounds.left(), position.y - bounds.top()))
    }

    /// The offset one visual line up (`dir` -1) or down (+1), keeping x.
    fn vertical(&self, offset: usize, dir: i32) -> Option<usize> {
        if !self.multiline {
            return None;
        }
        let p = self.position_for(offset)?;
        let lh = self.last_line_height;
        let y = p.y + lh * (dir as f32) + lh * 0.5;
        if y < px(0.) {
            return Some(0);
        }
        let total: Pixels = self.last_lines.iter().map(|l| l.size(lh).height.max(lh)).fold(px(0.), |a, b| a + b);
        if y > total {
            return Some(self.content.len());
        }
        Some(self.offset_for(point(p.x, y)))
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(&mut self, range_utf16: Range<usize>, actual_range: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(&mut self, _ignore_disabled_input: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected_range), reversed: self.selection_reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked_range.clone()).unwrap_or(self.selected_range.clone());
        let new_text = if self.multiline { new_text.to_string() } else { new_text.replace('\n', " ") };
        self.content = self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..];
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked_range.clone()).unwrap_or(self.selected_range.clone());
        self.content = self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.marked_range = if new_text.is_empty() { None } else { Some(range.start..range.start + new_text.len()) };
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.changed(cx);
    }

    fn bounds_for_range(&mut self, range_utf16: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let a = self.position_for(range.start)?;
        let b = self.position_for(range.end)?;
        Some(Bounds::from_corners(point(bounds.left() + a.x, bounds.top() + a.y), point(bounds.left() + b.x, bounds.top() + b.y + self.last_line_height)))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let bounds = self.last_bounds?;
        let local = point(p.x - bounds.left(), p.y - bounds.top());
        Some(self.offset_to_utf16(self.offset_for(local)))
    }
}

/// Byte offset where each logical line starts.
fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, c) in content.char_indices() {
        if c == '\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn position_for(lines: &[WrappedLine], starts: &[usize], offset: usize, lh: Pixels) -> Option<Point<Pixels>> {
    let li = starts.iter().rposition(|s| *s <= offset)?;
    let mut y = px(0.);
    for line in lines.iter().take(li) {
        y += line.size(lh).height.max(lh);
    }
    let line = lines.get(li)?;
    let p = line.position_for_index(offset - starts[li], lh)?;
    Some(point(p.x, y + p.y))
}

fn offset_for(lines: &[WrappedLine], starts: &[usize], p: Point<Pixels>, lh: Pixels, len: usize) -> usize {
    let mut y = px(0.);
    for (li, line) in lines.iter().enumerate() {
        let h = line.size(lh).height.max(lh);
        if p.y < y + h || li + 1 == lines.len() {
            let local = point(p.x.max(px(0.)), (p.y - y).max(px(0.)).min(h - px(1.)));
            let i = match line.closest_index_for_position(local, lh) {
                Ok(i) | Err(i) => i,
            };
            return (starts.get(li).copied().unwrap_or(0) + i).min(len);
        }
        y += h;
    }
    len
}

/// Paints the text, the selection and the caret.
struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<WrappedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
    line_height: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

fn shape(input: &TextInput, width: Option<Pixels>, window: &mut Window, cx: &App) -> Vec<WrappedLine> {
    let style = window.text_style();
    let theme = cx.theme();
    let (text, color): (SharedString, _) =
        if input.content.is_empty() { (input.placeholder.clone(), theme.text_3) } else { (input.content.clone().into(), style.color) };
    let base = TextRun { len: text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
    let runs = match (&input.marked_range, input.content.is_empty()) {
        (Some(m), false) => vec![
            TextRun { len: m.start, ..base.clone() },
            TextRun { len: m.end - m.start, underline: Some(UnderlineStyle { color: Some(base.color), thickness: px(1.0), wavy: false }), ..base.clone() },
            TextRun { len: text.len() - m.end, ..base },
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        _ => vec![base],
    };
    let font_size = style.font_size.to_pixels(window.rem_size());
    let wrap = if input.multiline { width } else { None };
    window.text_system().shape_text(text, font_size, &runs, wrap, None).map(|l| l.into_iter().collect()).unwrap_or_default()
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let line_height = window.line_height();
        if !self.input.read(cx).multiline {
            style.size.height = line_height.into();
            return (window.request_layout(style, [], cx), ());
        }
        let input = self.input.clone();
        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.or(match available.width {
                gpui::AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let lh = window.line_height();
            let read = input.read(cx);
            let min = read.min_lines;
            let lines = shape(read, width, window, cx);
            let h: Pixels = lines.iter().map(|l| l.size(lh).height.max(lh)).fold(px(0.), |a, b| a + b);
            size(width.unwrap_or(px(100.)), h.max(lh * min as f32))
        });
        (id, ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> PrepaintState {
        let line_height = window.line_height();
        let lines = shape(self.input.read(cx), Some(bounds.size.width), window, cx);
        self.input.update(cx, |input, _| {
            input.last_line_height = line_height;
            input.last_bounds = Some(bounds);
        });
        let input = self.input.read(cx);
        let starts = line_starts(&input.content);
        let pos = |o: usize| position_for(&lines, &starts, o, line_height);
        let accent = cx.theme().accent;
        let empty = input.content.is_empty();
        let mut selections = vec![];
        let mut cursor = None;
        if input.selected_range.is_empty() || empty {
            let at = if empty { Some(point(px(0.), px(0.))) } else { pos(input.cursor_offset()) };
            if let Some(p) = at {
                cursor = Some(fill(Bounds::new(point(bounds.left() + p.x, bounds.top() + p.y), size(px(1.5), line_height)), accent));
            }
        } else {
            // One rectangle per visual line the selection covers.
            let (s, e) = (input.selected_range.start, input.selected_range.end);
            let mut boundaries: Vec<usize> = input.content[s..e].char_indices().map(|(i, _)| s + i).collect();
            boundaries.push(e);
            let mut row: Option<(Point<Pixels>, Pixels)> = None;
            let flush = |row: Option<(Point<Pixels>, Pixels)>, selections: &mut Vec<PaintQuad>| {
                if let Some((a, right)) = row {
                    let right = right.max(a.x + px(3.));
                    selections.push(fill(
                        Bounds::from_corners(point(bounds.left() + a.x, bounds.top() + a.y), point(bounds.left() + right, bounds.top() + a.y + line_height)),
                        accent.opacity(0.28),
                    ));
                }
            };
            for o in boundaries {
                let Some(p) = pos(o) else { continue };
                row = match row {
                    Some((a, _)) if (p.y - a.y).abs() < px(1.) => Some((a, p.x)),
                    other => {
                        flush(other, &mut selections);
                        Some((p, p.x))
                    }
                };
            }
            flush(row, &mut selections);
        }
        PrepaintState { lines, cursor, selections, line_height }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), prepaint: &mut PrepaintState, window: &mut Window, cx: &mut App) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        for s in prepaint.selections.drain(..) {
            window.paint_quad(s);
        }
        let lh = prepaint.line_height;
        let mut y = bounds.top();
        for line in &prepaint.lines {
            let _ = line.paint(point(bounds.left(), y), lh, gpui::TextAlign::Left, Some(bounds), window, cx);
            y += line.size(lh).height.max(lh);
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        let lines = std::mem::take(&mut prepaint.lines);
        self.input.update(cx, |input, _| input.last_lines = lines);
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._blur.is_none() {
            let handle = self.focus_handle.clone();
            self._blur = Some(cx.on_blur(&handle, window, |_, _, cx| cx.emit(InputEvent::Blur)));
        }
        let theme = cx.theme().clone();
        let focused = self.focus_handle.is_focused(window);
        div()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .px(px(if self.bare { 12. } else { 10. }))
            .py(px(if self.multiline { 8. } else { 6. }))
            .when(!self.bare, |d| {
                d.rounded(px(sz::R_SM))
                    .bg(theme.bg_sunken.opacity(if theme.is_dark() { 0.7 } else { 0.9 }))
                    .border_1()
                    .border_color(if focused { theme.accent_ring } else { theme.line_strong })
            })
            .text_size(px(sz::BASE))
            .line_height(px(sz::BASE * 1.45))
            .text_color(theme.text)
            .when(self.mono, |d| d.font_family(crate::theme::MONO))
            .when(self.disabled, |d| d.opacity(0.5))
            .child(TextElement { input: cx.entity() })
    }
}
