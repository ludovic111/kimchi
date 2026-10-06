# The window

kimchi has two screens: **home**, where your projects are, and the **editor**, where you cut one.
Dialogs, menus and notifications float above either.

## Home

Home opens when no project is open, and with ⌘W (Ctrl+W) from the editor ("All projects").

- **Start from a prompt.** Describe a shot, pick **Video** or **Image** and a format, and press
  Enter (Shift+Enter starts a new line). kimchi creates the project, names it after the first
  five words of the prompt and sends the prompt to the Generate panel, aimed at the start of the
  timeline. **Empty project** creates an "Untitled" project without generating anything.
- **Formats:** 16:9 (1920×1080, the default), 9:16 (1080×1920), 1:1 (1080×1080), 4:5
  (1080×1350) and 21:9 (2560×1080). New projects run at 30 frames per second on a black
  background; change either later in the inspector.
- **Projects.** Click a card to open it. Each card shows its cover, length, size and when it last
  changed, and a ✦ count of the generated items in it. Right-click a card, or use its **…** button,
  to **Open**, **Duplicate** or **Delete project…**. **New**, beside the "Projects" heading, creates
  an empty project. Deleting asks first and also deletes the media generated inside the project; it
  can't be undone.
- **Top bar:** an update button when one is available, **Support**, the model providers you
  have connected (click to open Settings › Models & keys), the command palette and Settings.

If ffmpeg can't be found, home shows a banner: importing and exporting need it. The released
apps bundle it, so this only happens in development builds.

## The editor

```
┌──────────────────────────── top bar ─────────────────────────────┐
│ rail │ left panel │        preview        │ inspector │          │
│      │            │                       │           │  Agent   │
│      ├────────────┴───────────────────────┴───────────┤  panel   │
│      │                    timeline                   │          │
└──────┴───────────────────────────────────────────────┴──────────┘
```

Drag any divider to resize the panel beside it; double-click it to reset. Panel sizes, and whether
the left panel and inspector are open, are remembered between runs.

### Top bar

From left to right: **All projects**; the project's **name** (click it to rename; Enter saves, Esc
cancels); its size and frame rate (in wide windows); an update button when a new version is out
("Update to …", then its progress and "Restart to update"; hidden in narrow windows); **Undo** and
**Redo**; **Generations**, which lists generation jobs and counts the running ones; **Agent**; the
buttons that show and hide the left panel and the inspector; **Settings** (which opens Models & keys
while no model is connected; in narrow windows it moves into the **…** menu); a **…** menu (command
palette, keyboard shortcuts, What's new, Help, Support kimchi); and **Export**. Help opens the guide
to driving kimchi from AI and scripts.

### The rail and the left panel

The rail of tabs on the left edge picks what the left panel shows. Clicking the open tab hides
the panel; the keys only ever show it.

| Tab | Key | What it holds |
| --- | --- | --- |
| Media | ⌘1 | Everything imported or generated into the project. See [Editing](editing.md#media). |
| Generate | ⌘2 | The prompt and the model's options. Its badge counts running jobs. See [Generation](generation.md). |
| Text | ⌘3 | Title presets and fonts. See [Editing](editing.md#titles). |
| Motion | ⌘4 | Templates and new 2D and 3D scenes. See [Motion graphics and 3D](motion.md). |
| Captions | ⌘5 | Transcription, subtitle files and caption style. See [Captions](captions.md). |

### Preview

The preview shows the frame under the playhead, drawn by the same compositor that renders the
export. Below it are the time, the transport buttons (go to start, previous frame, play, next
frame, go to end), the **Loop** toggle and the picture's scale. Small previews hide the less-used
parts. Select a clip to get move and scale
handles on the picture.

### Inspector

The inspector edits what is selected: a clip, several clips, a media item or a track. With
nothing selected it shows the project: canvas size, frame rate, background colour and a few
shortcuts. See [Editing](editing.md#the-inspector).

### Timeline

The timeline's toolbar holds, from left to right: the clock; split, delete and duplicate; snapping
and ripple delete; the mixer button with a level meter and **Record a voice-over**; add a marker and
add a title; add a video or audio track; **Generate at playhead**; and the zoom controls with
**Fit**. Narrow windows drop the less-used buttons first. See [Editing](editing.md#the-timeline).

Press **X** (or the mixer button) to show the mixer. It takes the tracks' place, or sits beside
them with the layout button next to it. See [Sound](sound.md).

### Agent panel

⌘J (Ctrl+J) opens the built-in agent beside the editor. See [The agent](agent.md).

### The Studio

Double-click a motion clip, or select it and press ⇧⌘O (Ctrl+Shift+O), to edit it in the
Studio, which takes the place of the rail and the work area. Esc or **Back to the edit**
returns. See [Motion graphics and 3D](motion.md#the-studio).

## Small windows

The window can be as small as 720×480. When there isn't room for every panel beside the
preview, the left panel and inspector first shrink, then leave the row one at a time (the left
panel first; its rail stays). A panel that doesn't fit opens as a **drawer** over the work when
you call it; click outside it or press Esc to close it. The Agent panel stays docked only while
the editor beside it keeps at least 760 pixels; otherwise it opens as a drawer too. Drawers go
back into the row when the window is wide enough again.

## Menus (macOS)

On macOS, kimchi has a menu bar. Linux and Windows have no menu bar: everything in it is in the
[command palette](#the-command-palette) (⌘K), the **…** menu and the keyboard shortcuts.


| Menu | Items |
| --- | --- |
| kimchi | About kimchi, Check for Updates…, What's New, Settings…, Quit kimchi |
| File | New Project, Save, Import Media…, Export…, All Projects |
| Edit | Undo, Redo, Cut, Copy, Paste, Duplicate, Delete, Ripple Delete, Split at Playhead, Trim Start / End to Playhead, Select All, Deselect All |
| Timeline | Play / Pause, Loop Playback, Go to Start / End, Previous / Next Cut, Add Marker, Add Title, Zoom In / Out / to Fit, Snapping |
| Audio | Mixer, Add Effect…, Mute Track, Solo Track, Arm Track, Record Voice-Over |
| View | Command Palette, the five tabs, Left Panel, Inspector, Agent, Generation Jobs, Toggle Light / Dark |
| Help | Keyboard Shortcuts, What's New, Driving kimchi from AI and scripts, Logs and Crash Reports, Report a Problem…, Support kimchi |

**Save** only confirms that the project is saved: kimchi writes every change as you make it.

## The command palette

⌘K (Ctrl+K) opens the palette. Type to filter; ↑ and ↓ (or Ctrl+P and Ctrl+N) move, Enter runs, Esc
closes. It lists the editing, playback and timeline commands, the panels, settings pages, your other
projects ("Open “…”"; the five most recent until you type), and commands for the selection (for
example **Open in the Studio** on a motion clip).

Whatever you type can also be a prompt. In a project, the last rows are **Generate video:
“…”** and **Generate image: “…”**, which start the generation right away when a model is
connected (otherwise they open the Generate panel with the prompt filled in). On home, the row
is **New project from “…”**.

## Notifications

Confirmations and errors appear at the bottom right; click one to dismiss it. Short status messages
("Undid split", "Loop on") replace each other and fade quickly; errors stay a few seconds longer,
and notices with a button stay longest.

After an update, **What's new** opens once with the release notes (turn this off in Settings ›
Updates). If kimchi crashed or didn't quit properly the last time, a notice offers to show the
report. See [Settings and troubleshooting](settings.md#logs-and-crash-reports).

## Light and dark

Settings › Appearance › **Mode** is System (follow the computer, and change with it), Dark or Light.
**Toggle light / dark** (in the command palette, and the View menu on macOS) switches to the
opposite of what is showing and fixes the mode there. **Transparency** turns the glass panels, menus
and dialogs opaque when off; on macOS kimchi also follows the system's "Reduce transparency".

kimchi reads the system's reduced-motion preference when it starts (macOS accessibility
settings, or GNOME's animations setting on Linux) and then skips its animations.
