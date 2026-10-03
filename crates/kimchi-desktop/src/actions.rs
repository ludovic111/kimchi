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
        ToggleJobs,
        ToggleTheme,
        ShowShortcuts,
        OpenHelp,
        OpenSupport,
        WhatsNew,
        ShowDiagnostics,
        ReportProblem,
        RestartApp,
        OpenStudio,
        StudioEscape,
        StudioPlay,
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
const GROUPS: [&str; 7] = ["Playback", "Editing", "Timeline", "Panels", "Project", "Studio", "Studio: modelling"];

pub static SHORTCUTS: &[Shortcut] = &[
    sc!("Playback", "Play / pause", ["space"], Editing, PlayPause),
    sc!("Playback", "Play backwards, faster each press", ["j"], Editing, ShuttleBack),
    sc!("Playback", "Stop", ["k"], Editing, ShuttleStop),
    sc!("Playback", "Play, faster each press", ["l"], Editing, ShuttleForward),
    sc!("Playback", "Loop playback", ["M-l"], Editing, ToggleLoop),
    sc!("Playback", "Previous / next frame", ["left"], Editing, StepBack),
    sc!("", "Next frame", ["right"], Editing, StepForward),
    sc!("Playback", "Back / forward one second", ["shift-left"], Editing, StepBackSecond),
    sc!("", "Forward one second", ["shift-right"], Editing, StepForwardSecond),
    sc!("Playback", "Previous / next cut or marker", ["up"], Editing, PrevEdit),
    sc!("", "Next cut or marker", ["down"], Editing, NextEdit),
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
    sc!("Panels", "Keyboard shortcuts", ["?", "M-/"], App, ShowShortcuts),
    sc!("Project", "Import media", ["M-i"], App, Import),
    sc!("Project", "Export", ["M-e"], App, Export),
    sc!("Project", "Save", ["M-s"], App, Save),
    sc!("Project", "New project", ["M-n"], App, NewProject),
    sc!("Project", "All projects", ["M-w"], App, CloseProject),
    sc!("Project", "Settings", ["M-,"], App, OpenSettings),
    sc!("", "Quit", ["M-q"], App, Quit),
    sc!("Timeline", "Open in the Studio", ["M-shift-o"], Editing, OpenStudio),
    // The Studio: Blender's keys in 3D, After Effects' in 2D.
    sc!("Studio", "Back to the edit", ["escape"], Studio, StudioEscape),
    sc!("Studio", "Play / pause the clip", ["space"], Studio, StudioPlay),
    sc!("Studio", "Add", ["shift-a"], Studio, StudioAdd),
    sc!("Studio", "Move (2D: pen)", ["g"], Studio, StudioGrab),
    sc!("Studio", "Rotate", ["r"], Studio, StudioRotate),
    sc!("Studio", "Scale", ["s"], Studio, StudioScale),
    sc!("Studio", "Delete", ["x", "delete", "backspace"], Studio, StudioDelete),
    sc!("Studio", "Duplicate", ["shift-d", "M-d"], Studio, StudioDuplicate),
    sc!("Studio", "Select all / none", ["a", "M-a"], Studio, StudioSelectAll),
    sc!("Studio", "Box select", ["b"], Studio, StudioBoxSelect),
    sc!("Studio", "Hide selected", ["h"], Studio, StudioHide),
    sc!("Studio", "Show everything", ["alt-h"], Studio, StudioUnhide),
    sc!("Studio", "Keyframe here (edit mode: inset)", ["i"], Studio, StudioInsert),
    sc!("Studio", "Front view (edit mode: vertices)", ["1"], Studio, StudioKey1),
    sc!("Studio", "Edges (edit mode)", ["2"], Studio, StudioKey2),
    sc!("Studio", "Right view (edit mode: faces)", ["3"], Studio, StudioKey3),
    sc!("Studio", "Top view", ["7"], Studio, StudioKey7),
    sc!("Studio", "Through the camera", ["0"], Studio, StudioKey0),
    sc!("Studio", "Perspective / orthographic", ["5"], Studio, StudioOrtho),
    sc!("Studio", "Frame the selection", ["."], Studio, StudioFrame),
    sc!("Studio", "Frame (edit mode: fill)", ["f"], Studio, StudioFill),
    sc!("Studio", "Frame everything", ["home"], Studio, StudioFrameAll),
    sc!("Studio", "Fit the canvas (2D)", ["shift-z"], Studio, StudioFit),
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

pub fn bind(cx: &mut App) {
    let mut b: Vec<KeyBinding> = vec![];
    for s in SHORTCUTS {
        for k in s.keys {
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
            b.push(KeyBinding::load(&concrete(k), s.action(), context.map(|c| gpui::KeyBindingContextPredicate::parse(c).expect("valid context").into()), false, None, &gpui::DummyKeyboardMapper).expect("valid keystroke"));
        }
    }
    b.extend(crate::ui::input::bindings());
    cx.bind_keys(b);
}

impl Shortcut {
    pub fn action(&self) -> Box<dyn Action> {
        (self.action)()
    }

    /// The first key, as this platform writes it.
    pub fn hint(&self) -> SharedString {
        keys_label(self.keys[0])
    }
}

/// The shortcut of an action, as this platform writes it (`⇧⌘Z`, `Ctrl+Shift+Z`).
pub fn hint(action: &dyn Action) -> Option<SharedString> {
    SHORTCUTS.iter().find(|s| s.action().name() == action.name()).map(Shortcut::hint)
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
    keys_label_for(keys, cfg!(target_os = "macos")).into()
}

fn keys_label_for(keys: &str, mac: bool) -> String {
    // The key is the last part ("-" itself is a key: "M--").
    let (mods, key) = match keys.strip_suffix("--") {
        Some(m) => (m, "-"),
        None => match keys.rsplit_once('-') {
            Some((m, k)) if !k.is_empty() => (m, k),
            _ => ("", keys),
        },
    };
    let has = |m: &str| mods.split('-').any(|x| x == m);
    let key = match key {
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "space" => "Space".to_string(),
        "escape" => "Esc".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "enter" => if mac { "↵" } else { "Enter" }.to_string(),
        "backspace" => if mac { "⌫" } else { "Backspace" }.to_string(),
        "delete" => if mac { "⌦" } else { "Delete" }.to_string(),
        "-" => "−".to_string(),
        k => k.to_uppercase(),
    };
    if mac {
        // macOS order: ⌃⌥⇧⌘.
        let mut s = String::new();
        if has("ctrl") {
            s.push('⌃');
        }
        if has("alt") {
            s.push('⌥');
        }
        if has("shift") {
            s.push('⇧');
        }
        if has("M") || has("cmd") {
            s.push('⌘');
        }
        s + &key
    } else {
        let mut parts = vec![];
        if has("M") || has("ctrl") {
            parts.push("Ctrl");
        }
        if has("alt") {
            parts.push("Alt");
        }
        if has("shift") {
            parts.push("Shift");
        }
        parts.push(&key);
        parts.join("+")
    }
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
        Menu::new("View").items([
            MenuItem::action("Command Palette", Palette),
            MenuItem::action("Media", ShowMedia),
            MenuItem::action("Generate", ShowGenerate),
            MenuItem::action("Text", ShowText),
            MenuItem::action("Motion", ShowMotion),
            MenuItem::action("Captions", ShowCaptions),
            MenuItem::action("Agent", ToggleAgent),
            MenuItem::action("Generation Jobs", ToggleJobs),
            MenuItem::separator(),
            MenuItem::action("Toggle Light / Dark", ToggleTheme),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
            MenuItem::action("What's New", WhatsNew),
            MenuItem::action("Driving kimchi from AI and scripts", OpenHelp),
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
