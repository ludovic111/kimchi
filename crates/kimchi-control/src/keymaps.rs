//! Keyboard layouts: kimchi's own keys ([`ACTIONS`]), and other editors' keys for the actions
//! they share ([`LAYOUTS`]), so someone coming from Premiere Pro or Final Cut Pro keeps their
//! hands where they were. `settings.shortcuts.keymap` picks one; the window binds what
//! [`resolve`] gives, the `?` sheet and the tooltips show it, and `app.keymaps` lists it.
//!
//! How a layout is applied ([`resolve`]):
//! - an action the other app has a key for takes that app's keys (kimchi's own are dropped);
//! - an action it has no key for keeps kimchi's keys, except the ones that app uses for
//!   something else: the keys its own actions take, and the ones listed in
//!   [`Layout::taken`] (Q in Final Cut Pro connects a clip, so it doesn't trim in kimchi there).
//!   What was dropped and why is kept with the binding for the `?` sheet;
//! - the Studio's keys (Blender's and After Effects') never change.
//!
//! Keys are GPUI keystrokes as in the window's table: `M-` is ⌘ on macOS and Ctrl elsewhere;
//! `mac:` or `pc:` in front of a key keeps it to macOS or to Windows and Linux (Premiere nudges
//! with ⌘← on a Mac and Alt+← on Windows). Final Cut Pro and iMovie only run on macOS: their ⌘
//! is Ctrl on other systems.
//!
//! The layouts follow each app's own list of default shortcuts (the `source` of each layout,
//! read in October 2026): Premiere Pro 2026, Final Cut Pro 11, DaVinci Resolve 21, Media
//! Composer 2025, CapCut desktop, Kdenlive 26.08, Shotcut 26.9, VEGAS Pro 2026, iMovie 10.4.

use serde::Serialize;

/// Where a shortcut works (the window decides what that means for focus).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Everywhere, even while typing in a field (keys with ⌘ / Ctrl).
    App,
    /// In the window, but not while typing or behind a dialog.
    Editing,
    /// In the Studio (motion clips' editor); wins over the editor's keys there.
    Studio,
}

/// One of kimchi's actions with a key: its name is the window's action (`ui.action` runs it).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Action {
    pub name: &'static str,
    /// The `?` sheet's group; empty keeps it out of the sheet (the second half of a "previous /
    /// next" pair, or Quit).
    pub group: &'static str,
    pub label: &'static str,
    /// kimchi's own keys; the first is the one shown.
    pub keys: &'static [&'static str],
    pub scope: Scope,
}

const fn a(group: &'static str, label: &'static str, keys: &'static [&'static str], scope: Scope, name: &'static str) -> Action {
    Action { name, group, label, keys, scope }
}

use Scope::{App, Editing, Studio};

/// The `?` sheet's groups, in order.
pub const GROUPS: [&str; 8] = ["Playback", "Editing", "Timeline", "Audio", "Panels", "Project", "Studio", "Studio: modelling"];

/// kimchi's keys: the one table the window binds (through [`resolve`]), the `?` sheet shows and
/// tooltips, menus and the palette name.
pub static ACTIONS: &[Action] = &[
    a("Playback", "Play / pause", &["space"], Editing, "PlayPause"),
    a("Playback", "Play backwards, faster each press", &["j"], Editing, "ShuttleBack"),
    a("Playback", "Stop", &["k"], Editing, "ShuttleStop"),
    a("Playback", "Play, faster each press", &["l"], Editing, "ShuttleForward"),
    a("Playback", "Loop playback", &["M-l"], Editing, "ToggleLoop"),
    a("Playback", "Previous / next frame", &["left"], Editing, "StepBack"),
    a("", "Next frame", &["right"], Editing, "StepForward"),
    a("Playback", "Back / forward one second", &["shift-left"], Editing, "StepBackSecond"),
    a("", "Forward one second", &["shift-right"], Editing, "StepForwardSecond"),
    a("Playback", "Previous / next cut or marker", &["up"], Editing, "PrevEdit"),
    a("", "Next cut or marker", &["down"], Editing, "NextEdit"),
    a("Playback", "Go to start", &["home"], Editing, "GoToStart"),
    a("Playback", "Go to end", &["end"], Editing, "GoToEnd"),
    a("Editing", "Undo", &["M-z"], Editing, "Undo"),
    a("Editing", "Redo", &["M-shift-z", "M-y"], Editing, "Redo"),
    a("Editing", "Copy", &["M-c"], Editing, "CopyClips"),
    a("Editing", "Cut", &["M-x"], Editing, "CutClips"),
    a("Editing", "Paste at the playhead", &["M-v"], Editing, "PasteClips"),
    a("Editing", "Duplicate", &["M-d"], Editing, "Duplicate"),
    a("Editing", "Split at the playhead", &["s", "M-b"], Editing, "Split"),
    a("Editing", "Trim start to the playhead", &["q"], Editing, "TrimStart"),
    a("Editing", "Trim end to the playhead", &["w"], Editing, "TrimEnd"),
    a("Editing", "Nudge one frame", &["alt-left"], Editing, "NudgeLeft"),
    a("", "Nudge one frame right", &["alt-right"], Editing, "NudgeRight"),
    a("Editing", "Nudge ten frames", &["alt-shift-left"], Editing, "NudgeLeftMore"),
    a("", "Nudge ten frames right", &["alt-shift-right"], Editing, "NudgeRightMore"),
    a("Editing", "Delete", &["backspace", "delete"], Editing, "Delete"),
    a("Editing", "Delete and close the gap", &["shift-backspace", "shift-delete"], Editing, "RippleDelete"),
    a("Editing", "Select all", &["M-a"], Editing, "SelectAll"),
    a("Editing", "Deselect", &["escape", "M-shift-a"], Editing, "Deselect"),
    a("Timeline", "Add a title", &["t"], Editing, "AddText"),
    a("Timeline", "Add a marker", &["m"], Editing, "AddMarker"),
    a("Timeline", "Snapping", &["n"], Editing, "ToggleSnap"),
    a("Timeline", "Zoom in", &["=", "+", "M-="], Editing, "ZoomIn"),
    a("Timeline", "Zoom out", &["-", "M--"], Editing, "ZoomOut"),
    a("Timeline", "Zoom to fit", &["shift-z", "\\", "M-0"], Editing, "ZoomFit"),
    a("Panels", "Command palette", &["M-k"], App, "Palette"),
    a("Panels", "Generate (focus the prompt)", &["M-g"], App, "FocusGenerate"),
    a("Panels", "Media", &["M-1"], App, "ShowMedia"),
    a("Panels", "Generate", &["M-2"], App, "ShowGenerate"),
    a("Panels", "Text", &["M-3"], App, "ShowText"),
    a("Panels", "Motion", &["M-4"], App, "ShowMotion"),
    a("Panels", "Captions", &["M-5"], App, "ShowCaptions"),
    a("Panels", "Agent", &["M-j"], App, "ToggleAgent"),
    a("Panels", "Show / hide the left panel", &["M-alt-b"], App, "ToggleLeftPanel"),
    a("Panels", "Show / hide the inspector", &["M-alt-i"], App, "ToggleInspector"),
    a("Panels", "Keyboard shortcuts", &["?", "M-/"], App, "ShowShortcuts"),
    a("Project", "Import media", &["M-i"], App, "Import"),
    a("Project", "Export", &["M-e"], App, "Export"),
    a("Project", "Save", &["M-s"], App, "Save"),
    a("Project", "New project", &["M-n"], App, "NewProject"),
    a("Project", "All projects", &["M-w"], App, "CloseProject"),
    a("Project", "Settings", &["M-,"], App, "OpenSettings"),
    a("", "Quit", &["M-q"], App, "Quit"),
    a("Timeline", "Open in the Studio", &["M-shift-o"], Editing, "OpenStudio"),
    // Sound: the mixer key is ryolune's.
    a("Audio", "Mixer", &["x"], Editing, "ToggleMixer"),
    a("Audio", "Mute the selected track", &["alt-m"], Editing, "MuteTrack"),
    a("Audio", "Solo the selected track", &["alt-s"], Editing, "SoloTrack"),
    a("Audio", "Arm the selected track for recording", &["alt-a"], Editing, "ArmTrack"),
    a("Audio", "Record a voice-over / stop", &["shift-r"], Editing, "RecordVoiceOver"),
    a("Audio", "Add an effect", &["M-shift-e"], Editing, "AddEffect"),
    // The Studio: Blender's keys in 3D, After Effects' in 2D.
    a("Studio", "Back to the edit", &["escape"], Studio, "StudioEscape"),
    a("Studio", "Play / pause the clip", &["space"], Studio, "StudioPlay"),
    a("Studio", "Add", &["shift-a"], Studio, "StudioAdd"),
    a("Studio", "Move (2D: pen)", &["g"], Studio, "StudioGrab"),
    a("Studio", "Rotate", &["r"], Studio, "StudioRotate"),
    a("Studio", "Scale", &["s"], Studio, "StudioScale"),
    a("Studio", "Delete", &["x", "delete", "backspace"], Studio, "StudioDelete"),
    a("Studio", "Duplicate", &["shift-d", "M-d"], Studio, "StudioDuplicate"),
    a("Studio", "Select all / none", &["a", "M-a"], Studio, "StudioSelectAll"),
    a("Studio", "Box select", &["b"], Studio, "StudioBoxSelect"),
    a("Studio", "Hide selected", &["h"], Studio, "StudioHide"),
    a("Studio", "Show everything", &["alt-h"], Studio, "StudioUnhide"),
    a("Studio", "Keyframe here (edit mode: inset)", &["i"], Studio, "StudioInsert"),
    a("Studio", "Front view (edit mode: vertices)", &["1"], Studio, "StudioKey1"),
    a("Studio", "Edges (edit mode)", &["2"], Studio, "StudioKey2"),
    a("Studio", "Right view (edit mode: faces)", &["3"], Studio, "StudioKey3"),
    a("Studio", "Top view", &["7"], Studio, "StudioKey7"),
    a("Studio", "Through the camera", &["0"], Studio, "StudioKey0"),
    a("Studio", "Perspective / orthographic", &["5"], Studio, "StudioOrtho"),
    a("Studio", "Frame the selection", &["."], Studio, "StudioFrame"),
    a("Studio", "Frame (edit mode: fill)", &["f"], Studio, "StudioFill"),
    a("Studio", "Frame everything", &["home"], Studio, "StudioFrameAll"),
    a("Studio", "Fit the canvas (2D)", &["shift-z"], Studio, "StudioFit"),
    a("Studio", "Zoom in", &["=", "+"], Studio, "StudioZoomIn"),
    a("Studio", "Zoom out", &["-"], Studio, "StudioZoomOut"),
    a("Studio", "Canvas at 100% (2D)", &["/"], Studio, "StudioZoom100"),
    a("Studio", "Fly through the scene (WASD, QE, mouse to look)", &["shift-`", "~"], Studio, "StudioFly"),
    a("Studio", "Align the active camera to the view", &["M-alt-0"], Studio, "StudioAlignCamera"),
    a("Studio", "Select tool", &["v"], Studio, "StudioToolSelect"),
    a("Studio", "Next tool", &["w"], Studio, "StudioToolCycle"),
    a("Studio", "Pen (2D)", &["p"], Studio, "StudioPen"),
    a("Studio", "Shape tools (2D)", &["q"], Studio, "StudioShape"),
    a("Studio", "Text tool (2D)", &["t"], Studio, "StudioText"),
    a("Studio", "Anchor point tool (2D)", &["y"], Studio, "StudioAnchor"),
    a("Studio", "Dope sheet / graph editor", &["M-shift-g"], Studio, "StudioGraph"),
    a("Studio: modelling", "Edit mode", &["tab"], Studio, "StudioToggleEdit"),
    a("Studio: modelling", "Extrude", &["e"], Studio, "StudioExtrude"),
    a("Studio: modelling", "Bevel", &["M-b"], Studio, "StudioBevel"),
    a("Studio: modelling", "Loop cut", &["M-r"], Studio, "StudioLoopCut"),
    a("Studio: modelling", "Merge", &["m"], Studio, "StudioMerge"),
    a("Studio: modelling", "Flip normals", &["alt-n"], Studio, "StudioFlip"),
    a("Studio: modelling", "Recalculate normals", &["shift-n"], Studio, "StudioRecalc"),
];

/// Another editor's keys.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    /// `settings.shortcuts.keymap`.
    pub id: &'static str,
    /// "Premiere Pro".
    pub name: &'static str,
    /// The `kimchi_interop::apps` id it comes from.
    pub app: Option<&'static str>,
    /// Where its keys come from.
    pub source: &'static str,
    /// What to know about it, in a sentence (or empty).
    pub notes: &'static str,
    /// Its keys for kimchi's actions: (action, keys).
    #[serde(skip)]
    pub bindings: &'static [(&'static str, &'static [&'static str])],
    /// Keys that do something else in that app, which kimchi's own keys for other actions give
    /// up there: (key, what it does in the app).
    #[serde(skip)]
    pub taken: &'static [(&'static str, &'static str)],
}

pub static LAYOUTS: &[Layout] = &[
    Layout {
        id: "kimchi",
        name: "kimchi",
        app: None,
        source: "",
        notes: "kimchi's own keys: J / K / L, S to split, Q / W to trim to the playhead, ⌘K for the command palette.",
        bindings: &[],
        taken: &[],
    },
    Layout {
        id: "premiere",
        name: "Premiere Pro",
        app: Some("premiere"),
        source: "https://helpx.adobe.com/premiere-pro/using/default-keyboard-shortcuts.html",
        notes: "Add Edit (⌘K) splits, S turns snapping on and off, \\ fits the sequence; the command palette has no key here.",
        bindings: &[
            ("PlayPause", &["space"]),
            ("ShuttleBack", &["j"]),
            ("ShuttleStop", &["k"]),
            ("ShuttleForward", &["l"]),
            ("StepBack", &["left"]),
            ("StepForward", &["right"]),
            ("StepBackSecond", &["shift-left"]),
            ("StepForwardSecond", &["shift-right"]),
            ("PrevEdit", &["up", "M-shift-m"]),
            ("NextEdit", &["down", "shift-m"]),
            ("GoToStart", &["home"]),
            ("GoToEnd", &["end"]),
            ("Undo", &["M-z"]),
            ("Redo", &["M-shift-z"]),
            ("CopyClips", &["M-c"]),
            ("CutClips", &["M-x"]),
            ("PasteClips", &["M-v"]),
            ("Duplicate", &["M-shift-/"]),
            ("Split", &["M-k", "M-shift-k"]),
            ("TrimStart", &["q"]),
            ("TrimEnd", &["w"]),
            ("NudgeLeft", &["pc:alt-left", "mac:cmd-left"]),
            ("NudgeRight", &["pc:alt-right", "mac:cmd-right"]),
            ("NudgeLeftMore", &["pc:alt-shift-left", "mac:cmd-shift-left"]),
            ("NudgeRightMore", &["pc:alt-shift-right", "mac:cmd-shift-right"]),
            ("Delete", &["backspace", "delete"]),
            ("RippleDelete", &["shift-delete", "alt-backspace", "shift-backspace"]),
            ("SelectAll", &["M-a"]),
            ("Deselect", &["M-shift-a", "escape"]),
            ("AddText", &["M-t"]),
            ("AddMarker", &["m"]),
            ("ToggleSnap", &["s"]),
            ("ZoomIn", &["=", "+"]),
            ("ZoomOut", &["-"]),
            ("ZoomFit", &["\\"]),
            ("Import", &["M-i"]),
            ("Export", &["M-m"]),
            ("Save", &["M-s"]),
            ("NewProject", &["M-alt-n"]),
            ("ShowShortcuts", &["M-alt-k", "?"]),
            ("ShowMedia", &["shift-1"]),
            ("ToggleInspector", &["shift-5"]),
            ("ToggleMixer", &["shift-6"]),
        ],
        taken: &[
            ("M-l", "Link"),
            ("M-g", "Group"),
            ("M-d", "Apply Video Transition"),
            ("shift-r", "Reverse Match Frame"),
            ("mac:cmd-shift-e", "Enable"),
            ("mac:alt-m", "Clear Selected Marker"),
        ],
    },
    Layout {
        id: "finalcut",
        name: "Final Cut Pro",
        app: Some("finalcut"),
        source: "https://support.apple.com/guide/final-cut-pro/keyboard-shortcuts-ver90ba5929/mac",
        notes: "Delete closes the gap as in Final Cut's magnetic timeline (Shift-Delete leaves one), ⌘B blades, comma and period nudge.",
        bindings: &[
            ("PlayPause", &["space"]),
            ("ShuttleBack", &["j"]),
            ("ShuttleStop", &["k"]),
            ("ShuttleForward", &["l"]),
            ("ToggleLoop", &["M-l"]),
            ("StepBack", &["left"]),
            ("StepForward", &["right"]),
            ("StepBackSecond", &["shift-left"]),
            ("StepForwardSecond", &["shift-right"]),
            ("PrevEdit", &["up", ";", "ctrl-;"]),
            ("NextEdit", &["down", "'", "ctrl-'"]),
            ("GoToStart", &["home"]),
            ("GoToEnd", &["end"]),
            ("Undo", &["M-z"]),
            ("Redo", &["M-shift-z"]),
            ("CopyClips", &["M-c"]),
            ("CutClips", &["M-x"]),
            ("PasteClips", &["M-v"]),
            ("Duplicate", &["M-d"]),
            ("Split", &["M-b", "M-shift-b"]),
            ("TrimStart", &["alt-["]),
            ("TrimEnd", &["alt-]"]),
            ("NudgeLeft", &[","]),
            ("NudgeRight", &["."]),
            ("NudgeLeftMore", &["shift-,"]),
            ("NudgeRightMore", &["shift-."]),
            ("Delete", &["shift-backspace", "delete"]),
            ("RippleDelete", &["backspace"]),
            ("SelectAll", &["M-a"]),
            ("Deselect", &["M-shift-a", "escape"]),
            ("AddText", &["ctrl-t"]),
            ("AddMarker", &["m"]),
            ("ToggleSnap", &["n"]),
            ("ZoomIn", &["M-=", "M-+"]),
            ("ZoomOut", &["M--"]),
            ("ZoomFit", &["shift-z"]),
            ("Import", &["M-i"]),
            ("Export", &["M-e"]),
            ("NewProject", &["M-n"]),
            ("ShowShortcuts", &["M-alt-k"]),
            ("OpenSettings", &["M-,"]),
            ("ShowMedia", &["M-1"]),
            ("ToggleInspector", &["M-4"]),
            ("ToggleMixer", &["M-shift-8"]),
            ("SoloTrack", &["alt-s"]),
            ("RecordVoiceOver", &["M-alt-8", "alt-shift-a"]),
        ],
        taken: &[("q", "Connect to the primary storyline"), ("w", "Insert"), ("alt-m", "Add Marker and Modify")],
    },
    Layout {
        id: "resolve",
        name: "DaVinci Resolve",
        app: Some("resolve"),
        source: "https://documents.blackmagicdesign.com/UserManuals/DaVinciResolveReferenceManual.pdf",
        notes: "⌘\\ splits, Shift-[ and Shift-] trim to the playhead, Backspace leaves a gap and Delete closes it.",
        bindings: &[
            ("PlayPause", &["space"]),
            ("ShuttleBack", &["j"]),
            ("ShuttleStop", &["k"]),
            ("ShuttleForward", &["l"]),
            ("ToggleLoop", &["M-/"]),
            ("StepBack", &["left"]),
            ("StepForward", &["right"]),
            ("StepBackSecond", &["shift-left"]),
            ("StepForwardSecond", &["shift-right"]),
            ("PrevEdit", &["up", "shift-up"]),
            ("NextEdit", &["down", "shift-down"]),
            ("GoToStart", &["home"]),
            ("GoToEnd", &["end"]),
            ("Undo", &["M-z"]),
            ("Redo", &["M-shift-z"]),
            ("CopyClips", &["M-c"]),
            ("CutClips", &["M-x"]),
            ("PasteClips", &["M-v"]),
            ("Split", &["M-\\"]),
            ("TrimStart", &["shift-["]),
            ("TrimEnd", &["shift-]"]),
            ("NudgeLeft", &[","]),
            ("NudgeRight", &["."]),
            ("NudgeLeftMore", &["shift-,"]),
            ("NudgeRightMore", &["shift-."]),
            ("Delete", &["backspace"]),
            ("RippleDelete", &["delete"]),
            ("SelectAll", &["M-a"]),
            ("Deselect", &["M-shift-a", "escape"]),
            ("AddMarker", &["m"]),
            ("ToggleSnap", &["n"]),
            ("ZoomIn", &["M-="]),
            ("ZoomOut", &["M--"]),
            ("ZoomFit", &["shift-z"]),
            ("Import", &["M-i"]),
            ("Save", &["M-s"]),
            ("ShowShortcuts", &["M-alt-k", "?"]),
        ],
        taken: &[("t", "Trim Edit Mode"), ("x", "Mark Clip"), ("M-d", "Change Clip Duration")],
    },
    Layout {
        id: "avid",
        name: "Media Composer",
        app: Some("avid"),
        source: "https://www.avid.com/resource-center/media-composer-keyboard-shortcuts",
        notes: "A and S go to the previous and next edit, Z lifts and X extracts, H adds an edit, ⌘3 opens the command palette.",
        bindings: &[
            ("PlayPause", &["space"]),
            ("ShuttleBack", &["j"]),
            ("ShuttleStop", &["k"]),
            ("ShuttleForward", &["l"]),
            ("StepBack", &["left"]),
            ("StepForward", &["right"]),
            ("StepBackSecond", &["M-left"]),
            ("StepForwardSecond", &["M-right"]),
            ("PrevEdit", &["a"]),
            ("NextEdit", &["s"]),
            ("GoToStart", &["home", "M-home"]),
            ("GoToEnd", &["end", "M-end"]),
            ("Undo", &["M-z"]),
            ("Redo", &["M-r"]),
            ("CopyClips", &["c", "M-c"]),
            ("CutClips", &["M-x"]),
            ("PasteClips", &["M-v"]),
            ("Duplicate", &["M-d"]),
            ("Split", &["h"]),
            ("NudgeLeft", &[","]),
            ("NudgeRight", &["."]),
            ("NudgeLeftMore", &["m"]),
            ("NudgeRightMore", &["/"]),
            ("Delete", &["z", "backspace", "delete"]),
            ("RippleDelete", &["x"]),
            ("SelectAll", &["M-a"]),
            ("Deselect", &["M-shift-a", "escape"]),
            ("ZoomIn", &["M-]"]),
            ("ZoomOut", &["M-["]),
            ("ZoomFit", &["M-/"]),
            ("Save", &["M-s"]),
            ("Palette", &["M-3"]),
        ],
        taken: &[("q", "Go to In"), ("w", "Go to Out"), ("t", "Mark Clip"), ("M-l", "Enlarge Track"), ("M-k", "Reduce Track")],
    },
    Layout {
        id: "capcut",
        name: "CapCut",
        app: Some("capcut"),
        source: "CapCut desktop, Settings › Shortcuts",
        notes: "⌘B splits, Q and W delete what's left or right of the playhead, Shift-Z fits the timeline.",
        bindings: &[
            ("PlayPause", &["space"]),
            ("ShuttleBack", &["j"]),
            ("ShuttleStop", &["k"]),
            ("ShuttleForward", &["l"]),
            ("StepBack", &["left"]),
            ("StepForward", &["right"]),
            ("StepBackSecond", &["shift-left"]),
            ("StepForwardSecond", &["shift-right"]),
            ("PrevEdit", &["up", "alt-shift-m"]),
            ("NextEdit", &["down", "shift-m"]),
            ("GoToStart", &["home"]),
            ("GoToEnd", &["end"]),
            ("Undo", &["M-z"]),
            ("Redo", &["M-shift-z"]),
            ("CopyClips", &["M-c"]),
            ("CutClips", &["M-x"]),
            ("PasteClips", &["M-v"]),
            ("Split", &["M-b", "M-shift-b"]),
            ("TrimStart", &["q"]),
            ("TrimEnd", &["w"]),
            ("Delete", &["backspace", "delete"]),
            ("Deselect", &["alt-x", "escape", "M-shift-a"]),
            ("AddMarker", &["m"]),
            ("ToggleSnap", &["n"]),
            ("ZoomIn", &["M-=", "M-+"]),
            ("ZoomOut", &["M--"]),
            ("ZoomFit", &["shift-z"]),
        ],
        taken: &[("M-g", "Group"), ("alt-m", "Add a marker in a new colour")],
    },
    Layout {
        id: "kdenlive",
        name: "Kdenlive",
        app: Some("kdenlive"),
        source: "https://docs.kdenlive.org/en/user_interface/shortcuts.html",
        notes: "Shift-R cuts the clip, ( and ) trim to the playhead, G adds a guide, Ctrl+Enter renders.",
        bindings: &[
            ("PlayPause", &["space"]),
            ("ShuttleBack", &["j"]),
            ("ShuttleStop", &["k"]),
            ("ShuttleForward", &["l"]),
            ("StepBack", &["left"]),
            ("StepForward", &["right"]),
            ("StepBackSecond", &["shift-left"]),
            ("StepForwardSecond", &["shift-right"]),
            ("PrevEdit", &["alt-left", "M-left"]),
            ("NextEdit", &["alt-right", "M-right"]),
            ("GoToStart", &["M-home"]),
            ("GoToEnd", &["M-end"]),
            ("Undo", &["M-z"]),
            ("Redo", &["M-shift-z"]),
            ("CopyClips", &["M-c"]),
            ("CutClips", &["M-x"]),
            ("PasteClips", &["M-v"]),
            ("Duplicate", &["M-d"]),
            ("Split", &["shift-r", "M-shift-r"]),
            ("TrimStart", &["("]),
            ("TrimEnd", &[")"]),
            ("Delete", &["delete"]),
            ("RippleDelete", &["shift-delete"]),
            ("SelectAll", &["M-a"]),
            ("Deselect", &["M-shift-a", "escape"]),
            ("AddMarker", &["g"]),
            ("ZoomIn", &["M-=", "M-+"]),
            ("ZoomOut", &["M--"]),
            ("Save", &["M-s"]),
            ("NewProject", &["M-n"]),
            ("Export", &["M-enter"]),
            ("ShowShortcuts", &["M-alt-,", "?"]),
            ("OpenSettings", &["M-shift-,"]),
        ],
        taken: &[
            ("x", "Razor Tool"),
            ("t", "Switch Monitor"),
            ("shift-z", "Adjust Timeline Zone"),
            ("M-i", "Insert Zone in Project Bin"),
            ("M-g", "Group Clips"),
        ],
    },
    Layout {
        id: "shotcut",
        name: "Shotcut",
        app: Some("shotcut"),
        source: "https://shotcut.org/howtos/keyboard-shortcuts/",
        notes: "I and O trim the clip to the playhead, Z lifts and X ripple deletes, 0 fits the timeline, ⌘D selects none.",
        bindings: &[
            ("PlayPause", &["space"]),
            ("ShuttleBack", &["j"]),
            ("ShuttleStop", &["k"]),
            ("ShuttleForward", &["l"]),
            ("ToggleLoop", &["\\"]),
            ("StepBack", &["left"]),
            ("StepForward", &["right"]),
            ("StepBackSecond", &["pageup"]),
            ("StepForwardSecond", &["pagedown"]),
            ("PrevEdit", &["alt-left", "<"]),
            ("NextEdit", &["alt-right", ">"]),
            ("GoToStart", &["home"]),
            ("GoToEnd", &["end"]),
            ("Undo", &["M-z"]),
            ("Redo", &["M-shift-z", "pc:ctrl-y"]),
            ("CopyClips", &["M-c", "c"]),
            ("CutClips", &["M-x"]),
            ("PasteClips", &["M-v"]),
            ("Split", &["s", "shift-s"]),
            ("TrimStart", &["i"]),
            ("TrimEnd", &["o"]),
            ("NudgeLeft", &[","]),
            ("NudgeRight", &["."]),
            ("Delete", &["z", "delete", "backspace"]),
            ("RippleDelete", &["x", "shift-delete", "shift-backspace"]),
            ("SelectAll", &["M-a"]),
            ("Deselect", &["M-d", "escape"]),
            ("AddMarker", &["m"]),
            ("ToggleSnap", &["M-p"]),
            ("ZoomIn", &["=", "+"]),
            ("ZoomOut", &["-"]),
            ("ZoomFit", &["0"]),
            ("Save", &["M-s"]),
            ("NewProject", &["M-n"]),
            ("Export", &["M-e"]),
            ("ShowShortcuts", &["?", "/"]),
            ("MuteTrack", &["ctrl-m"]),
        ],
        taken: &[("M-i", "Add Video Track"), ("M-g", "Group")],
    },
    Layout {
        id: "vegas",
        name: "VEGAS Pro",
        app: Some("vegas"),
        source: "https://cdn.borisfx.com/borisfx/Documentation/vegas/2026/en/content/topics/13-appendix/shortcutkeys.htm",
        notes: "S splits, Q loops, F8 snaps, ↑ and ↓ zoom, Z and X mute and solo the track; Ctrl+Q adds an audio track there, so quit from the menu.",
        bindings: &[
            ("PlayPause", &["space", "enter"]),
            ("ShuttleBack", &["j"]),
            ("ShuttleStop", &["k"]),
            ("ShuttleForward", &["l"]),
            ("ToggleLoop", &["q", "M-shift-l"]),
            ("StepBack", &["alt-left", "left"]),
            ("StepForward", &["alt-right", "right"]),
            ("PrevEdit", &["M-alt-left", "M-left"]),
            ("NextEdit", &["M-alt-right", "M-right"]),
            ("GoToStart", &["M-home", "w"]),
            ("GoToEnd", &["M-end"]),
            ("Undo", &["M-z", "alt-backspace"]),
            ("Redo", &["M-shift-z", "M-y"]),
            ("CopyClips", &["M-c"]),
            ("CutClips", &["M-x", "shift-delete"]),
            ("PasteClips", &["M-v"]),
            ("Split", &["s"]),
            ("TrimStart", &["alt-["]),
            ("TrimEnd", &["alt-]"]),
            ("Delete", &["delete"]),
            ("SelectAll", &["M-a"]),
            ("Deselect", &["M-shift-a", "escape"]),
            ("AddMarker", &["m"]),
            ("ToggleSnap", &["f8"]),
            ("ZoomIn", &["up"]),
            ("ZoomOut", &["down"]),
            ("Save", &["M-s"]),
            ("NewProject", &["M-n"]),
            ("ShowMedia", &["alt-5"]),
            ("ToggleMixer", &["M-alt-5"]),
            ("MuteTrack", &["z"]),
            ("SoloTrack", &["x"]),
            ("RecordVoiceOver", &["M-r"]),
        ],
        taken: &[
            ("t", "Select Next Take"),
            ("M-d", "Normal Editing Tool"),
            ("M-e", "Open in Audio Editor"),
            ("M-g", "Go To"),
            ("M-q", "Insert Audio Track"),
            ("shift-z", "Mute the Selected Track Only"),
            ("\\", "Center View Around Cursor"),
        ],
    },
    Layout {
        id: "imovie",
        name: "iMovie",
        app: Some("imovie"),
        source: "https://support.apple.com/guide/imovie/movd9d8f91e8/mac",
        notes: "⌘B splits and Delete closes the gap; V records a voice-over.",
        bindings: &[
            ("PlayPause", &["space"]),
            ("ToggleLoop", &["M-l"]),
            ("StepBack", &["left"]),
            ("StepForward", &["right"]),
            ("PrevEdit", &["up"]),
            ("NextEdit", &["down"]),
            ("Undo", &["M-z"]),
            ("Redo", &["M-shift-z"]),
            ("CopyClips", &["M-c"]),
            ("CutClips", &["M-x"]),
            ("PasteClips", &["M-v"]),
            ("Split", &["M-b"]),
            ("RippleDelete", &["backspace"]),
            ("SelectAll", &["M-a"]),
            ("Deselect", &["M-shift-a", "escape"]),
            ("Import", &["M-i"]),
            ("NewProject", &["M-n"]),
            ("OpenSettings", &["M-,"]),
            ("ShowMedia", &["M-1"]),
            ("RecordVoiceOver", &["v"]),
        ],
        taken: &[("q", "Connect the selection"), ("w", "Insert the selection"), ("x", "Select an entire clip")],
    },
];

/// The layout with this id (case-insensitive; an app id or name works too), with a "did you
/// mean" error.
pub fn layout(id: &str) -> Result<&'static Layout, String> {
    let want = id.trim();
    if let Some(l) = LAYOUTS.iter().find(|l| l.id.eq_ignore_ascii_case(want) || l.name.eq_ignore_ascii_case(want) || l.app.is_some_and(|a| a.eq_ignore_ascii_case(want))) {
        return Ok(l);
    }
    let ids: Vec<&str> = LAYOUTS.iter().map(|l| l.id).collect();
    let hint = kimchi_core::closest(want, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("Unknown keyboard layout `{id}`.{hint} Layouts: {}.", ids.join(", ")))
}

/// The action with this name (the window's action name, without `kimchi::`).
pub fn action(name: &str) -> Option<&'static Action> {
    let name = name.trim().trim_start_matches("kimchi::");
    ACTIONS.iter().find(|a| a.name.eq_ignore_ascii_case(name))
}

/// One of kimchi's keys a layout took away from an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dropped {
    pub key: String,
    /// Why: "Snapping in Premiere Pro", "Link in Premiere Pro".
    pub because: String,
}

/// An action's keys in one layout, on one system.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub action: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub scope: Scope,
    /// The keys, first shown (empty: no key in this layout).
    pub keys: Vec<String>,
    /// The keys come from the other app's layout (not kimchi's own).
    pub from_layout: bool,
    /// kimchi's own keys this layout took away, and why.
    pub dropped: Vec<Dropped>,
}

/// The key a table entry stands for on this system (`None`: not on this system), with the
/// `mac:` / `pc:` prefix gone.
pub fn for_system(key: &str, mac: bool) -> Option<&str> {
    match (key.strip_prefix("mac:"), key.strip_prefix("pc:")) {
        (Some(k), _) => mac.then_some(k),
        (_, Some(k)) => (!mac).then_some(k),
        _ => Some(key),
    }
}

/// A key in one form whatever way it was written: modifiers in a fixed order, `M` made ⌘ or
/// Ctrl. Two keys are the same key on that system when these match.
pub fn normal(key: &str, mac: bool) -> String {
    let (mods, last) = split(key);
    let mut set = [false; 4]; // ctrl, alt, shift, cmd
    for m in mods.split('-').filter(|m| !m.is_empty()) {
        match m {
            "ctrl" | "control" => set[0] = true,
            "alt" | "option" => set[1] = true,
            "shift" => set[2] = true,
            "cmd" | "super" | "win" => set[3] = true,
            "M" | "secondary" => set[if mac { 3 } else { 0 }] = true,
            _ => {}
        }
    }
    let names = ["ctrl", "alt", "shift", "cmd"];
    let mut out: Vec<&str> = names.iter().zip(set).filter(|(_, on)| *on).map(|(n, _)| *n).collect();
    out.push(last);
    out.join("-")
}

/// `"M-shift-z"` → (`"M-shift"`, `"z"`); `"M--"` → (`"M"`, `"-"`); `"-"` → (`""`, `"-"`).
fn split(key: &str) -> (&str, &str) {
    if let Some(m) = key.strip_suffix("--") {
        return (m, "-");
    }
    match key.rsplit_once('-') {
        Some((m, k)) if !k.is_empty() => (m, k),
        _ => ("", key),
    }
}

/// Studio keys stay among themselves: they only clash with each other.
fn studio(s: Scope) -> bool {
    s == Scope::Studio
}

/// Every action's keys in `layout` on this kind of system (`mac`: macOS; else Windows and
/// Linux), in [`ACTIONS`] order. See the module doc for the rules.
pub fn resolve(layout: &Layout, mac: bool) -> Vec<Binding> {
    // The keys the layout gives its actions, as they are on this system.
    let given = |name: &str| -> Option<Vec<String>> {
        let (_, keys) = layout.bindings.iter().find(|(n, _)| *n == name)?;
        let keys: Vec<String> = keys.iter().filter_map(|k| for_system(k, mac)).map(str::to_string).collect();
        (!keys.is_empty()).then_some(keys)
    };
    // Who owns each key in the editor: the layout's actions, then its taken keys.
    let mut owner: Vec<(String, String)> = vec![];
    for act in ACTIONS.iter().filter(|a| !studio(a.scope)) {
        if let Some(keys) = given(act.name) {
            for k in keys {
                owner.push((normal(&k, mac), format!("{} in {}", act.label.split(" / ").next().unwrap_or(act.label), layout.name)));
            }
        }
    }
    for (k, what) in layout.taken {
        if let Some(k) = for_system(k, mac) {
            owner.push((normal(k, mac), format!("{what} in {}", layout.name)));
        }
    }
    ACTIONS
        .iter()
        .map(|act| {
            let mut b = Binding { action: act.name, group: act.group, label: act.label, scope: act.scope, keys: vec![], from_layout: false, dropped: vec![] };
            if studio(act.scope) {
                b.keys = act.keys.iter().map(|k| k.to_string()).collect();
                return b;
            }
            if let Some(keys) = given(act.name) {
                b.keys = keys;
                b.from_layout = true;
                return b;
            }
            for k in act.keys {
                match owner.iter().find(|(o, _)| *o == normal(k, mac)) {
                    Some((_, why)) => b.dropped.push(Dropped { key: k.to_string(), because: why.clone() }),
                    None => b.keys.push(k.to_string()),
                }
            }
            b
        })
        .collect()
}

/// [`resolve`] for the system kimchi runs on.
pub fn resolve_here(layout: &Layout) -> Vec<Binding> {
    resolve(layout, cfg!(target_os = "macos"))
}

/// A table key (`"M-shift-z"`, `"alt-left"`, `"?"`) as the system writes it: `⇧⌘Z` on macOS,
/// `Ctrl+Shift+Z` elsewhere.
pub fn label(key: &str, mac: bool) -> String {
    let key = for_system(key, mac).unwrap_or(key);
    let (mods, k) = split(key);
    let has = |m: &str| mods.split('-').any(|x| x == m);
    let k = match k {
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "space" => "Space".to_string(),
        "escape" => "Esc".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "Page Up".to_string(),
        "pagedown" => "Page Down".to_string(),
        "tab" => "Tab".to_string(),
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
        s + &k
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
        if has("cmd") {
            parts.push("Win");
        }
        parts.push(&k);
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_like_each_platform() {
        assert_eq!(label("M-shift-z", true), "⇧⌘Z");
        assert_eq!(label("M-shift-z", false), "Ctrl+Shift+Z");
        assert_eq!(label("alt-shift-left", true), "⌥⇧←");
        assert_eq!(label("M--", false), "Ctrl+−");
        assert_eq!(label("-", true), "−");
        assert_eq!(label("shift-backspace", false), "Shift+Backspace");
        assert_eq!(label("?", true), "?");
        assert_eq!(label("space", true), "Space");
        assert_eq!(label("mac:cmd-left", true), "⌘←");
        assert_eq!(label("M-\\", false), "Ctrl+\\");
    }

    #[test]
    fn keys_compare_whatever_way_they_are_written() {
        assert_eq!(normal("M-shift-z", true), normal("shift-cmd-z", true));
        assert_eq!(normal("M-shift-z", false), "ctrl-shift-z");
        assert_ne!(normal("M-z", true), normal("ctrl-z", true));
        assert_eq!(normal("M--", false), "ctrl--");
        assert_eq!(normal("alt-backspace", true), "alt-backspace");
    }

    #[test]
    fn action_names_and_groups_are_known() {
        let mut names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ACTIONS.len(), "an action is listed twice");
        for a in ACTIONS {
            assert!(a.group.is_empty() || GROUPS.contains(&a.group), "{} has an unknown group", a.name);
            assert!(!a.keys.is_empty(), "{} has no key", a.name);
        }
        let ids: Vec<&str> = LAYOUTS.iter().map(|l| l.id).collect();
        assert_eq!(ids, crate::settings::KEYMAPS, "settings::KEYMAPS lists the layouts in order");
        for l in LAYOUTS {
            for (name, keys) in l.bindings {
                let act = action(name).unwrap_or_else(|| panic!("{}: no action {name}", l.id));
                assert_eq!(act.name, *name, "{}: write {name} as {}", l.id, act.name);
                assert!(act.scope != Scope::Studio, "{}: layouts leave the Studio's keys alone ({name})", l.id);
                assert!(!keys.is_empty(), "{}: {name} has no key", l.id);
            }
            let mut seen = std::collections::HashSet::new();
            assert!(l.bindings.iter().all(|(n, _)| seen.insert(*n)), "{}: an action is bound twice", l.id);
            if let Some(app) = l.app {
                assert!(kimchi_interop::apps::app(app).is_some_and(|a| a.keymap == Some(l.id)), "{}: apps.rs says {app}'s keymap is another", l.id);
            }
        }
        for app in kimchi_interop::apps::APPS {
            if let Some(k) = app.keymap {
                assert!(layout(k).is_ok(), "{} names keymap {k}", app.id);
            }
        }
    }

    /// In every layout, on both kinds of systems, a key does one thing in a scope: the editor's
    /// keys (App and Editing together) and the Studio's apart.
    #[test]
    fn no_two_actions_share_a_key() {
        for l in LAYOUTS {
            for mac in [true, false] {
                let mut seen: std::collections::HashMap<(String, bool), &str> = Default::default();
                for b in resolve(l, mac) {
                    for k in &b.keys {
                        if let Some(other) = seen.insert((normal(k, mac), studio(b.scope)), b.action) {
                            panic!("{} ({}): {k} is bound to both {other} and {}", l.id, if mac { "mac" } else { "pc" }, b.action);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn layouts_take_the_apps_keys_and_say_what_they_dropped() {
        let premiere = resolve(layout("premiere").unwrap(), false);
        let get = |name: &str| premiere.iter().find(|b| b.action == name).unwrap().clone();
        assert_eq!(get("Split").keys, ["M-k", "M-shift-k"]);
        assert!(get("Split").from_layout);
        assert_eq!(get("ToggleSnap").keys, ["s"]);
        // ⌘K is Add Edit there: the palette loses its key, and says so.
        let palette = get("Palette");
        assert!(palette.keys.is_empty());
        assert_eq!(palette.dropped, [Dropped { key: "M-k".into(), because: "Split at the playhead in Premiere Pro".into() }]);
        // ⌘L is Link in Premiere.
        assert_eq!(get("ToggleLoop").dropped[0].because, "Link in Premiere Pro");
        // Keys nobody uses there stay.
        assert_eq!(get("ToggleAgent").keys, ["M-j"]);
        // Per system: ⌘← on a Mac, Alt+← elsewhere.
        assert_eq!(get("NudgeLeft").keys, ["alt-left"]);
        let mac = resolve(layout("Premiere Pro").unwrap(), true);
        assert_eq!(mac.iter().find(|b| b.action == "NudgeLeft").unwrap().keys, ["cmd-left"]);
        // kimchi's own layout is kimchi's keys.
        for b in resolve(layout("kimchi").unwrap(), true) {
            assert_eq!(b.keys, action(b.action).unwrap().keys, "{}", b.action);
            assert!(b.dropped.is_empty());
        }
        // The Studio never changes.
        let fcp = resolve(layout("finalcut").unwrap(), true);
        assert_eq!(fcp.iter().find(|b| b.action == "StudioShape").unwrap().keys, ["q"]);
        assert!(fcp.iter().find(|b| b.action == "TrimStart").unwrap().from_layout);
        assert!(layout("finalcutt").unwrap_err().contains("Did you mean `finalcut`?"));
    }
}
