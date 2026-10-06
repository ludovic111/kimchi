//! Keyboard shortcuts and the menu bar. Every action ends in a registry
//! command (or a pure view change such as zoom or a panel).
//!
//! [`SHORTCUTS`] is the one list of keys: it binds them, names them in the
//! shortcuts sheet (`?`), and gives tooltips, menus and the palette their hint
//! in this platform's notation (`⇧⌘Z` on macOS, `Ctrl+Shift+Z` elsewhere).

use gpui::{Action, App, KeyBinding, Menu, MenuItem, OsAction, SharedString, actions};

actions!(
    kimchi,
    [
        Quit,
        About,
        NewProject,
        CloseProject,
        OpenSettings,
        CheckUpdates,
        Save,
        Undo,
        Redo,
        Import,
        Export,
        Palette,
        CopyClips,
        CutClips,
        PasteClips,
        Duplicate,
        Split,
        TrimStart,
        TrimEnd,
        NudgeLeft,
        NudgeRight,
        NudgeLeftMore,
        NudgeRightMore,
        FocusGenerate,
        SelectAll,
        PlayPause,
        ShuttleBack,
        ShuttleStop,
        ShuttleForward,
        ToggleLoop,
        StepBack,
        StepForward,
        StepBackSecond,
        StepForwardSecond,
        PrevEdit,
        NextEdit,
        GoToStart,
        GoToEnd,
        Delete,
        RippleDelete,
        Deselect,
        ToggleSnap,
        ZoomIn,
        ZoomOut,
        ZoomFit,
        AddMarker,
        AddText,
        ShowMedia,
        ShowGenerate,
        ShowText,
        ShowMotion,
        ShowCaptions,
        ToggleAgent,
        ToggleLeftPanel,
        ToggleInspector,
        ToggleJobs,
        ToggleTheme,
        ShowShortcuts,
        OpenHelp,
        OpenSupport,
        WhatsNew,
        SetUpKimchi,
        ShowDiagnostics,
        ReportProblem,
        RestartApp,
        OpenStudio,
        StudioEscape,
        StudioPlay,
        StudioPreviousKey,
        StudioNextKey,
        StudioExtendPreviousItem,
        StudioExtendNextItem,
        StudioGrab,
        StudioRotate,
        StudioScale,
        StudioAdd,
        StudioDelete,
        StudioDuplicate,
        StudioToggleEdit,
        StudioSelectAll,
        StudioBoxSelect,
        StudioKey1,
        StudioKey2,
        StudioKey3,
        StudioKey7,
        StudioKey0,
        StudioOrtho,
        StudioFrame,
        StudioFill,
        StudioFrameAll,
        StudioInsert,
        StudioExtrude,
        StudioBevel,
        StudioLoopCut,
        StudioMerge,
        StudioFlip,
        StudioRecalc,
        StudioToolSelect,
        StudioToolCycle,
        StudioPen,
        StudioShape,
        StudioText,
        StudioAnchor,
        StudioFit,
        StudioGraph,
        StudioHide,
        StudioUnhide,
        ToggleMixer,
        MuteTrack,
        SoloTrack,
        ArmTrack,
        RecordVoiceOver,
        AddEffect,
        StudioAlignCamera,
        StudioFly,
        StudioZoomIn,
        StudioZoomOut,
        StudioZoom100,
    ]
);

/// Where a shortcut works.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everywhere, even while typing in a field.
    App,
    /// In the window, but not while a text field has focus or a dialog is open (single keys,
    /// ⌘Z on the project…). Escape still closes dialogs.
    Editing,
    /// In the Studio (motion clips' editor), like `Editing`; wins over the editor's keys there.
    Studio,
}

/// One shortcut. `keys` are GPUI keystrokes where `M` is ⌘ on macOS and Ctrl elsewhere; the
/// first is the one shown, the rest are aliases. An empty `group` keeps it out of the sheet.
pub struct Shortcut {
    pub group: &'static str,
    pub label: &'static str,
    pub keys: &'static [&'static str],
    pub scope: Scope,
    pub action: fn() -> Box<dyn Action>,
}

macro_rules! sc {
    ($group:literal, $label:literal, [$($k:literal),+], $scope:ident, $action:ident) => {
        Shortcut { group: $group, label: $label, keys: &[$($k),+], scope: Scope::$scope, action: || Box::new($action) }
    };
}

#[cfg(test)]
const GROUPS: [&str; 8] = ["Playback", "Editing", "Timeline", "Audio", "Panels", "Project", "Studio", "Studio: modelling"];

pub static SHORTCUTS: &[Shortcut] = &[
    sc!("Playback", "Play / pause", ["space"], Editing, PlayPause),
    sc!("Playback", "Play backwards, faster each press", ["j"], Editing, ShuttleBack),
    sc!("Playback", "Stop", ["k"], Editing, ShuttleStop),
    sc!("Playback", "Play, faster each press", ["l"], Editing, ShuttleForward),
    sc!("Playback", "Loop playback", ["M-l"], Editing, ToggleLoop),
    sc!("Playback", "Previous frame / collapse Outliner item", ["left"], Editing, StepBack),
    sc!("", "Next frame / expand Outliner item", ["right"], Editing, StepForward),
    sc!("Playback", "Back / forward one second", ["shift-left"], Editing, StepBackSecond),
    sc!("", "Forward one second", ["shift-right"], Editing, StepForwardSecond),
    sc!("Playback", "Previous cut or marker / Outliner item", ["up"], Editing, PrevEdit),
    sc!("", "Next cut or marker / Outliner item", ["down"], Editing, NextEdit),
    sc!("Playback", "Go to start", ["home"], Editing, GoToStart),
    sc!("Playback", "Go to end", ["end"], Editing, GoToEnd),
    sc!("Editing", "Undo", ["M-z"], Editing, Undo),
    sc!("Editing", "Redo", ["M-shift-z", "M-y"], Editing, Redo),
    sc!("Editing", "Copy", ["M-c"], Editing, CopyClips),
    sc!("Editing", "Cut", ["M-x"], Editing, CutClips),
    sc!("Editing", "Paste at the playhead", ["M-v"], Editing, PasteClips),
    sc!("Editing", "Duplicate", ["M-d"], Editing, Duplicate),
    sc!("Editing", "Split at the playhead", ["s", "M-b"], Editing, Split),
    sc!("Editing", "Trim start to the playhead", ["q"], Editing, TrimStart),
    sc!("Editing", "Trim end to the playhead", ["w"], Editing, TrimEnd),
    sc!("Editing", "Nudge one frame", ["alt-left"], Editing, NudgeLeft),
    sc!("", "Nudge one frame right", ["alt-right"], Editing, NudgeRight),
    sc!("Editing", "Nudge ten frames", ["alt-shift-left"], Editing, NudgeLeftMore),
    sc!("", "Nudge ten frames right", ["alt-shift-right"], Editing, NudgeRightMore),
    sc!("Editing", "Delete", ["backspace", "delete"], Editing, Delete),
    sc!("Editing", "Delete and close the gap", ["shift-backspace", "shift-delete"], Editing, RippleDelete),
    sc!("Editing", "Select all", ["M-a"], Editing, SelectAll),
    sc!("Editing", "Deselect", ["escape", "M-shift-a"], Editing, Deselect),
    sc!("Timeline", "Add a title", ["t"], Editing, AddText),
    sc!("Timeline", "Add a marker", ["m"], Editing, AddMarker),
    sc!("Timeline", "Snapping", ["n"], Editing, ToggleSnap),
    sc!("Timeline", "Zoom in", ["=", "+", "M-="], Editing, ZoomIn),
    sc!("Timeline", "Zoom out", ["-", "M--"], Editing, ZoomOut),
    sc!("Timeline", "Zoom to fit", ["shift-z", "\\", "M-0"], Editing, ZoomFit),
    sc!("Panels", "Command palette", ["M-k"], App, Palette),
    sc!("Panels", "Generate (focus the prompt)", ["M-g"], App, FocusGenerate),
    sc!("Panels", "Media", ["M-1"], App, ShowMedia),
    sc!("Panels", "Generate", ["M-2"], App, ShowGenerate),
    sc!("Panels", "Text", ["M-3"], App, ShowText),
    sc!("Panels", "Motion", ["M-4"], App, ShowMotion),
    sc!("Panels", "Captions", ["M-5"], App, ShowCaptions),
    sc!("Panels", "Agent", ["M-j"], App, ToggleAgent),
    sc!("Panels", "Show / hide the left panel", ["M-alt-b"], App, ToggleLeftPanel),
    sc!("Panels", "Show / hide the inspector", ["M-alt-i"], App, ToggleInspector),
    sc!("Panels", "Keyboard shortcuts", ["?", "M-/"], App, ShowShortcuts),
    sc!("Project", "Import media", ["M-i"], App, Import),
    sc!("Project", "Export", ["M-e"], App, Export),
    sc!("Project", "Save", ["M-s"], App, Save),
    sc!("Project", "New project", ["M-n"], App, NewProject),
    sc!("Project", "All projects", ["M-w"], App, CloseProject),
    sc!("Project", "Settings", ["M-,"], App, OpenSettings),
    sc!("", "Quit", ["M-q"], App, Quit),
    sc!("Timeline", "Open in the Studio", ["M-shift-o"], Editing, OpenStudio),
    // Sound: the mixer key is ryolune's.
    sc!("Audio", "Mixer", ["x"], Editing, ToggleMixer),
    sc!("Audio", "Mute the selected track", ["alt-m"], Editing, MuteTrack),
    sc!("Audio", "Solo the selected track", ["alt-s"], Editing, SoloTrack),
    sc!("Audio", "Arm the selected track for recording", ["alt-a"], Editing, ArmTrack),
    sc!("Audio", "Record a voice-over / stop", ["shift-r"], Editing, RecordVoiceOver),
    sc!("Audio", "Add an effect", ["M-shift-e"], Editing, AddEffect),
    // The Studio: Blender's keys in 3D, After Effects' in 2D.
    sc!("Studio", "Back to the edit", ["escape"], Studio, StudioEscape),
    sc!("Studio", "Play / pause the clip", ["space"], Studio, StudioPlay),
    sc!("Studio", "Previous keyframe", ["alt-left"], Studio, StudioPreviousKey),
    sc!("Studio", "Next keyframe", ["alt-right"], Studio, StudioNextKey),
    sc!("Studio", "Extend Outliner selection up", ["shift-up"], Studio, StudioExtendPreviousItem),
    sc!("Studio", "Extend Outliner selection down", ["shift-down"], Studio, StudioExtendNextItem),
    sc!("Studio", "Add", ["shift-a"], Studio, StudioAdd),
    sc!("Studio", "Move / retime keys (2D: pen)", ["g"], Studio, StudioGrab),
    sc!("Studio", "Rotate", ["r"], Studio, StudioRotate),
    sc!("Studio", "Scale / stretch key timing", ["s"], Studio, StudioScale),
    sc!("Studio", "Delete", ["x", "delete", "backspace"], Studio, StudioDelete),
    sc!("Studio", "Duplicate", ["shift-d", "M-d"], Studio, StudioDuplicate),
    sc!("Studio", "Select all / none", ["a", "M-a"], Studio, StudioSelectAll),
    sc!("Studio", "Box select", ["b"], Studio, StudioBoxSelect),
    sc!("Studio", "Hide selected", ["h"], Studio, StudioHide),
    sc!("Studio", "Show everything", ["alt-h"], Studio, StudioUnhide),
    sc!("Studio", "Keyframe here (edit mode: inset)", ["i"], Studio, StudioInsert),
    sc!("Studio", "Front view / vertices / graph X", ["1"], Studio, StudioKey1),
    sc!("Studio", "Edges / graph Y", ["2"], Studio, StudioKey2),
    sc!("Studio", "Right view / faces / graph Z", ["3"], Studio, StudioKey3),
    sc!("Studio", "Top view", ["7"], Studio, StudioKey7),
    sc!("Studio", "Through the camera", ["0"], Studio, StudioKey0),
    sc!("Studio", "Perspective / orthographic", ["5"], Studio, StudioOrtho),
    sc!("Studio", "Frame the selection", ["."], Studio, StudioFrame),
    sc!("Studio", "Frame (edit mode: fill)", ["f"], Studio, StudioFill),
    sc!("Studio", "Frame everything", ["home"], Studio, StudioFrameAll),
    sc!("Studio", "Fit the canvas (2D)", ["shift-z"], Studio, StudioFit),
    sc!("Studio", "Zoom in", ["=", "+"], Studio, StudioZoomIn),
    sc!("Studio", "Zoom out", ["-"], Studio, StudioZoomOut),
    sc!("Studio", "Canvas at 100% (2D)", ["/"], Studio, StudioZoom100),
    sc!("Studio", "Fly through the scene (WASD, QE, mouse to look)", ["shift-`", "~"], Studio, StudioFly),
    sc!("Studio", "Align the active camera to the view", ["M-alt-0"], Studio, StudioAlignCamera),
    sc!("Studio", "Select tool", ["v"], Studio, StudioToolSelect),
    sc!("Studio", "Next tool", ["w"], Studio, StudioToolCycle),
    sc!("Studio", "Pen (2D)", ["p"], Studio, StudioPen),
    sc!("Studio", "Shape tools (2D)", ["q"], Studio, StudioShape),
    sc!("Studio", "Text tool (2D)", ["t"], Studio, StudioText),
    sc!("Studio", "Anchor point tool (2D)", ["y"], Studio, StudioAnchor),
    sc!("Studio", "Dope sheet / graph editor", ["M-shift-g"], Studio, StudioGraph),
    sc!("Studio: modelling", "Edit mode", ["tab"], Studio, StudioToggleEdit),
    sc!("Studio: modelling", "Extrude", ["e"], Studio, StudioExtrude),
    sc!("Studio: modelling", "Bevel", ["M-b"], Studio, StudioBevel),
    sc!("Studio: modelling", "Loop cut", ["M-r"], Studio, StudioLoopCut),
    sc!("Studio: modelling", "Merge", ["m"], Studio, StudioMerge),
    sc!("Studio: modelling", "Flip normals", ["alt-n"], Studio, StudioFlip),
    sc!("Studio: modelling", "Recalculate normals", ["shift-n"], Studio, StudioRecalc),
];

#[cfg(target_os = "macos")]
const M: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const M: &str = "ctrl";

/// A keystroke from the table, with `M` made concrete.
fn concrete(keys: &str) -> String {
    match keys.strip_prefix("M-") {
        Some(rest) => format!("{M}-{rest}"),
        None => keys.to_string(),
    }
}

/// The keys of the layout in use (`settings.shortcuts.keymap`), by action name.
struct Active {
    layout: &'static kimchi_control::keymaps::Layout,
    bindings: Vec<kimchi_control::keymaps::Binding>,
}

thread_local! {
    // The window's thread (each UI test has its own app on its own thread).
    static ACTIVE: std::cell::RefCell<Option<std::rc::Rc<Active>>> = const { std::cell::RefCell::new(None) };
}

fn active() -> Option<std::rc::Rc<Active>> {
    ACTIVE.with(|a| a.borrow().clone())
}

/// The keyboard layout in use.
pub fn layout() -> &'static kimchi_control::keymaps::Layout {
    active().map(|a| a.layout).unwrap_or(&kimchi_control::keymaps::LAYOUTS[0])
}

/// The action's name in the tables (`PlayPause`).
fn name_of(action: &dyn Action) -> &'static str {
    action.name().trim_start_matches("kimchi::")
}

/// The binding of an action in the layout in use (`None` before [`bind`]).
pub fn binding(name: &str) -> Option<kimchi_control::keymaps::Binding> {
    active()?.bindings.iter().find(|b| b.action == name).cloned()
}

/// A shortcut's keys in the layout in use, first shown (empty: none in this layout).
pub fn keys_of(s: &Shortcut) -> Vec<String> {
    let name = name_of(&*s.action());
    match binding(name) {
        Some(b) => b.keys,
        None => s.keys.iter().map(|k| k.to_string()).collect(),
    }
}

/// Binds the keys of the layout in settings (again, after it changed). Other bindings (text
/// fields, dialogs) stay.
pub fn bind(cx: &mut App) {
    use crate::store::StoreExt;
    let id = cx.try_global::<crate::store::GlobalStore>().map(|_| cx.store().read(cx).settings.shortcuts.keymap.clone()).unwrap_or_default();
    let layout = kimchi_control::keymaps::layout(&id).unwrap_or(&kimchi_control::keymaps::LAYOUTS[0]);
    let bindings = kimchi_control::keymaps::resolve_here(layout);
    let ours: std::collections::HashSet<&str> = SHORTCUTS.iter().map(|s| s.action().name()).collect();
    let others: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().filter(|b| !ours.contains(b.action().name())).cloned().collect();
    ACTIVE.with(|a| *a.borrow_mut() = Some(std::rc::Rc::new(Active { layout, bindings })));
    let mut b: Vec<KeyBinding> = vec![];
    for s in SHORTCUTS {
        for k in keys_of(s) {
            let k = k.as_str();
            // A key without a modifier (`?`) types a character in a field, so it never works there.
            let bare = !k.starts_with("M-") && !k.contains("ctrl-") && !k.contains("alt-") && !k.contains("cmd-");
            let context = match s.scope {
                _ if s.action().name() == Quit.name() => None,
                Scope::App if !bare => Some("Workspace"),
                _ if s.action().name() == Deselect.name() => Some("Workspace && !TextInput"),
                Scope::App | Scope::Editing => Some("Workspace && !TextInput && !Modal"),
                // The Studio's own keys; `StudioBusy` is set while a mouse tool or menu takes the keys.
                Scope::Studio => Some("Studio && !TextInput && !Modal && !StudioBusy"),
            };
            match KeyBinding::load(&concrete(k), s.action(), context.map(|c| gpui::KeyBindingContextPredicate::parse(c).expect("valid context").into()), false, None, &gpui::DummyKeyboardMapper) {
                Ok(binding) => b.push(binding),
                Err(e) => tracing::warn!("the key {k} of {} doesn't parse: {e}", s.label),
            }
        }
    }
    // The first time, the text fields' keys come along; later they are among the others.
    let fresh = others.is_empty();
    cx.clear_key_bindings();
    cx.bind_keys(others);
    if fresh {
        cx.bind_keys(crate::ui::input::bindings());
    }
    cx.bind_keys(b);
    if !fresh {
        cx.set_menus(menus());
    }
}

impl Shortcut {
    pub fn action(&self) -> Box<dyn Action> {
        (self.action)()
    }

    /// The first key in the layout in use, as this platform writes it (empty: no key there).
    pub fn hint(&self) -> Option<SharedString> {
        keys_of(self).first().map(|k| keys_label(k))
    }
}

/// The shortcut of an action, as this platform writes it (`⇧⌘Z`, `Ctrl+Shift+Z`).
pub fn hint(action: &dyn Action) -> Option<SharedString> {
    SHORTCUTS.iter().find(|s| s.action().name() == action.name()).and_then(Shortcut::hint)
}

/// `"Undo"` → `"Undo (⌘Z)"`: tooltips name their shortcut.
pub fn tip(label: &str, action: &dyn Action) -> SharedString {
    match hint(action) {
        Some(h) => format!("{label} ({h})").into(),
        None => label.to_string().into(),
    }
}

/// A table keystroke (`"M-shift-z"`, `"alt-left"`, `"?"`) in this platform's notation.
pub fn keys_label(keys: &str) -> SharedString {
    kimchi_control::keymaps::label(keys, cfg!(target_os = "macos")).into()
}

#[cfg(test)]
fn keys_label_for(keys: &str, mac: bool) -> String {
    kimchi_control::keymaps::label(keys, mac)
}

pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new("kimchi").items([
            MenuItem::action("About kimchi", About),
            MenuItem::action("Check for Updates…", CheckUpdates),
            MenuItem::action("What's New", WhatsNew),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit kimchi", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Project", NewProject),
            MenuItem::action("Save", Save),
            MenuItem::action("Import Media…", Import),
            MenuItem::action("Export…", Export),
            MenuItem::separator(),
            MenuItem::action("All Projects", CloseProject),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", CutClips, OsAction::Cut),
            MenuItem::os_action("Copy", CopyClips, OsAction::Copy),
            MenuItem::os_action("Paste", PasteClips, OsAction::Paste),
            MenuItem::action("Duplicate", Duplicate),
            MenuItem::action("Delete", Delete),
            MenuItem::action("Ripple Delete", RippleDelete),
            MenuItem::separator(),
            MenuItem::action("Split at Playhead", Split),
            MenuItem::action("Trim Start to Playhead", TrimStart),
            MenuItem::action("Trim End to Playhead", TrimEnd),
            MenuItem::separator(),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::action("Deselect All", Deselect),
        ]),
        Menu::new("Timeline").items([
            MenuItem::action("Play / Pause", PlayPause),
            MenuItem::action("Loop Playback", ToggleLoop),
            MenuItem::action("Go to Start", GoToStart),
            MenuItem::action("Go to End", GoToEnd),
            MenuItem::action("Previous Cut", PrevEdit),
            MenuItem::action("Next Cut", NextEdit),
            MenuItem::separator(),
            MenuItem::action("Add Marker", AddMarker),
            MenuItem::action("Add Title", AddText),
            MenuItem::separator(),
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Zoom to Fit", ZoomFit),
            MenuItem::action("Snapping", ToggleSnap),
        ]),
        Menu::new("Audio").items([
            MenuItem::action("Mixer", ToggleMixer),
            MenuItem::action("Add Effect…", AddEffect),
            MenuItem::separator(),
            MenuItem::action("Mute Track", MuteTrack),
            MenuItem::action("Solo Track", SoloTrack),
            MenuItem::action("Arm Track", ArmTrack),
            MenuItem::action("Record Voice-Over", RecordVoiceOver),
        ]),
        Menu::new("View").items([
            MenuItem::action("Command Palette", Palette),
            MenuItem::action("Media", ShowMedia),
            MenuItem::action("Generate", ShowGenerate),
            MenuItem::action("Text", ShowText),
            MenuItem::action("Motion", ShowMotion),
            MenuItem::action("Captions", ShowCaptions),
            MenuItem::action("Left Panel", ToggleLeftPanel),
            MenuItem::action("Inspector", ToggleInspector),
            MenuItem::action("Agent", ToggleAgent),
            MenuItem::action("Generation Jobs", ToggleJobs),
            MenuItem::separator(),
            MenuItem::action("Toggle Light / Dark", ToggleTheme),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
            MenuItem::action("What's New", WhatsNew),
            MenuItem::action("kimchi User Guide", OpenHelp),
            MenuItem::action("Set Up kimchi…", SetUpKimchi),
            MenuItem::separator(),
            MenuItem::action("Logs and Crash Reports", ShowDiagnostics),
            MenuItem::action("Report a Problem…", ReportProblem),
            MenuItem::action("Support kimchi", OpenSupport),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_like_each_platform() {
        assert_eq!(keys_label_for("M-shift-z", true), "⇧⌘Z");
        assert_eq!(keys_label_for("M-shift-z", false), "Ctrl+Shift+Z");
        assert_eq!(keys_label_for("alt-shift-left", true), "⌥⇧←");
        assert_eq!(keys_label_for("M--", false), "Ctrl+−");
        assert_eq!(keys_label_for("-", true), "−");
        assert_eq!(keys_label_for("shift-backspace", false), "Shift+Backspace");
        assert_eq!(keys_label_for("?", true), "?");
        assert_eq!(keys_label_for("space", true), "Space");
    }

    /// kimchi's own keys are the same in both tables (the layouts are built on kimchi-control's).
    #[test]
    fn the_table_matches_the_keymaps() {
        let names: Vec<&str> = SHORTCUTS.iter().map(|s| name_of(&*s.action())).collect();
        let theirs: Vec<&str> = kimchi_control::keymaps::ACTIONS.iter().map(|a| a.name).collect();
        assert_eq!(names, theirs, "actions.rs SHORTCUTS and kimchi_control::keymaps::ACTIONS list the same actions in order");
        for (s, a) in SHORTCUTS.iter().zip(kimchi_control::keymaps::ACTIONS) {
            assert_eq!(s.keys, a.keys, "{}", a.name);
            assert_eq!((s.group, s.label), (a.group, a.label), "{}", a.name);
        }
        // Every layout's keys parse.
        for l in kimchi_control::keymaps::LAYOUTS {
            for b in kimchi_control::keymaps::resolve_here(l) {
                for k in &b.keys {
                    gpui::Keystroke::parse(&concrete(k)).unwrap_or_else(|_| panic!("{}: {k} doesn't parse", l.id));
                }
            }
        }
    }

    #[test]
    fn every_key_parses_and_is_bound_once() {
        let mut seen = std::collections::HashSet::new();
        for s in SHORTCUTS {
            for k in s.keys {
                gpui::Keystroke::parse(&concrete(k)).unwrap_or_else(|_| panic!("{k} doesn't parse"));
                // The Studio's keys win over the editor's while it is open: unique per scope.
                assert!(seen.insert((*k, s.scope == Scope::Studio)), "{k} is bound twice");
            }
            assert!(s.group.is_empty() || GROUPS.contains(&s.group), "{} has an unknown group", s.label);
        }
    }
}
