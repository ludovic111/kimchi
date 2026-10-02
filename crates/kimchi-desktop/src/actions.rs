//! Keyboard shortcuts and the menu bar. Every action ends in a registry
//! command (or a pure view change such as zoom or a panel).

use gpui::{App, KeyBinding, Menu, MenuItem, OsAction, actions};

actions!(
    kimchi,
    [
        Quit,
        About,
        NewProject,
        CloseProject,
        OpenSettings,
        CheckUpdates,
        Undo,
        Redo,
        Import,
        Export,
        Palette,
        Duplicate,
        Split,
        FocusGenerate,
        SelectAll,
        PlayPause,
        StepBack,
        StepForward,
        StepBackSecond,
        StepForwardSecond,
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
        ToggleAgent,
        ToggleJobs,
        ToggleTheme,
        OpenHelp,
        OpenSupport,
    ]
);

#[cfg(target_os = "macos")]
const M: &str = "cmd";
#[cfg(not(target_os = "macos"))]
const M: &str = "ctrl";

pub fn bind(cx: &mut App) {
    // Shortcuts that also work while typing.
    let anywhere = Some("Workspace");
    // Single keys: not while a text field has focus.
    let timeline = Some("Workspace && !TextInput");
    let k = |s: &str| s.replace("M", M);
    let mut b = vec![
        KeyBinding::new(&k("M-q"), Quit, None),
        KeyBinding::new(&k("M-n"), NewProject, anywhere),
        KeyBinding::new(&k("M-w"), CloseProject, anywhere),
        KeyBinding::new(&k("M-,"), OpenSettings, anywhere),
        KeyBinding::new(&k("M-z"), Undo, timeline),
        KeyBinding::new(&k("M-shift-z"), Redo, timeline),
        KeyBinding::new(&k("M-y"), Redo, timeline),
        KeyBinding::new(&k("M-i"), Import, anywhere),
        KeyBinding::new(&k("M-e"), Export, anywhere),
        KeyBinding::new(&k("M-k"), Palette, anywhere),
        KeyBinding::new(&k("M-d"), Duplicate, timeline),
        KeyBinding::new(&k("M-b"), Split, timeline),
        KeyBinding::new(&k("M-g"), FocusGenerate, anywhere),
        KeyBinding::new(&k("M-a"), SelectAll, timeline),
        KeyBinding::new(&k("M-j"), ToggleAgent, anywhere),
        KeyBinding::new("space", PlayPause, timeline),
        KeyBinding::new("left", StepBack, timeline),
        KeyBinding::new("right", StepForward, timeline),
        KeyBinding::new("shift-left", StepBackSecond, timeline),
        KeyBinding::new("shift-right", StepForwardSecond, timeline),
        KeyBinding::new("home", GoToStart, timeline),
        KeyBinding::new("end", GoToEnd, timeline),
        KeyBinding::new("backspace", Delete, timeline),
        KeyBinding::new("delete", Delete, timeline),
        KeyBinding::new("shift-backspace", RippleDelete, timeline),
        KeyBinding::new("shift-delete", RippleDelete, timeline),
        KeyBinding::new("escape", Deselect, timeline),
        KeyBinding::new("s", Split, timeline),
        KeyBinding::new("n", ToggleSnap, timeline),
        KeyBinding::new("=", ZoomIn, timeline),
        KeyBinding::new("+", ZoomIn, timeline),
        KeyBinding::new("-", ZoomOut, timeline),
        KeyBinding::new("shift-z", ZoomFit, timeline),
        KeyBinding::new("m", AddMarker, timeline),
        KeyBinding::new("t", AddText, timeline),
    ];
    b.extend(crate::ui::input::bindings());
    cx.bind_keys(b);
}

pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new("kimchi").items([
            MenuItem::action("About kimchi", About),
            MenuItem::action("Check for Updates…", CheckUpdates),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit kimchi", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Project", NewProject),
            MenuItem::action("Import Media…", Import),
            MenuItem::action("Export…", Export),
            MenuItem::separator(),
            MenuItem::action("Close Project", CloseProject),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::action("Split at Playhead", Split),
            MenuItem::action("Duplicate", Duplicate),
            MenuItem::action("Delete", Delete),
            MenuItem::action("Ripple Delete", RippleDelete),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Timeline").items([
            MenuItem::action("Play / Pause", PlayPause),
            MenuItem::action("Go to Start", GoToStart),
            MenuItem::action("Go to End", GoToEnd),
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
            MenuItem::action("Generate", FocusGenerate),
            MenuItem::action("Agent", ToggleAgent),
            MenuItem::action("Generation Jobs", ToggleJobs),
            MenuItem::separator(),
            MenuItem::action("Toggle Light / Dark", ToggleTheme),
        ]),
        Menu::new("Help").items([MenuItem::action("Driving kimchi from AI and scripts", OpenHelp), MenuItem::action("Support kimchi", OpenSupport)]),
    ]
}
