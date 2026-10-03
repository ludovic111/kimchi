# Changelog

What changed in each kimchi release. The app shows the newest section in **What's new** after it
updates, and the release workflow puts the section in the update's notes.

## 0.6.0 — 2026-10-03

### New
- **What's new**: after an update, kimchi shows what changed. Open it any time from Help › What's New, the command palette or Settings › Updates.
- **Logs and crash reports**: kimchi keeps a log of each run and writes a report when something goes wrong, including when it didn't quit properly. Settings › Diagnostics lists them and shows the log, and Help › Report a Problem opens an issue with your version and system filled in. Nothing is sent anywhere.
- **Updates on Windows**: the installer is downloaded and verified in the background, then runs when kimchi restarts or quits.
- **Updates while kimchi stays open**: it checks again every few hours, and can download and install updates by itself (Settings › Updates). **Restart now** right from Settings.
- **Window buttons** on Windows and on Linux desktops that don't draw their own, and the top bar moves the window on Windows.
- New commands: `app.whatsNew`, `app.diagnostics`, `app.logs`, `app.crashReports`, `app.clearCrashReports`, `app.restart`; `ui.showPanel` opens `shortcuts`, `whatsNew` and `diagnostics`.

### Better
- **Phone media**: portrait photos stay upright, HDR video from recent phones is tone-mapped, and transparent WebM keeps its transparency.
- **Long recordings**: hundreds of cuts from one file export on every system, and NTSC frame rates (29.97, 59.94, 23.976) are exact.
- **Captions** from other tools: SRT and WebVTT files in UTF-16 or Windows encodings, with odd spacing or no blank lines, import correctly, and text like "I <3 you" stays whole.
- kimchi finds ffmpeg, Claude Code and Codex installed with Homebrew, npm, nvm, Snap, Scoop, Chocolatey or WinGet, also when opened from the Dock or Finder.
- Generations ride out a short network drop instead of failing (and losing a render already paid for); a ComfyUI job removed from its queue stops waiting.
- The built-in Codex agent only reaches kimchi's own tools.
- Wording that matches your system: Ctrl instead of ⌘, Explorer or your file manager instead of Finder, and where your keys are kept.

### Fixed
- An edit that fails halfway (moving several clips, deleting across tracks) no longer leaves the project half-changed.
- Deleting a project no longer removes media its duplicate uses.
- An agent's batch of changes can't stay open after it's stopped, or roll back edits made by someone else meanwhile.
- Typing `?` in a text field types it; Ctrl+Enter sends on Linux and Windows; renaming a track and slider arrow keys keep the keyboard; Space, Delete and other editing keys no longer act on the timeline behind a dialog.
- Delete on a media file asks first, as its menu does; "Clear finished" clears; errors show above dialogs.
- Switching projects stops the agent's run and resets its conversation, the clipboard and the generation target.
- Ruler labels and times no longer repeat or read "1:60"; zoom to fit matches the Fit button.
- Playing without a sound device no longer fills memory; a file without a preview stops showing a loader.
- Reversed clips and clips past the end of their video show a picture instead of nothing.
- Sound cut from AAC recordings starts exactly where it should, whichever ffmpeg kimchi uses.
- Logging out or shutting down is a proper quit.
- Many smaller fixes: unbounded times, huge timeouts and tiny animation lengths can't hang or break a project, project files are written safely, and the CLI sends paths relative to where it runs.

## 0.5.0 — 2026-10-03

### New
- **Motion graphics and 3D**: keyframes with easings on every clip, presets, 2D and 3D motion clips, 15 templates and a scene editor.
- **Transitions** on cuts, with sound crossfades in the export.
- **Colour**: grading, chroma key and LUTs, graded on the media or the layer.
- **Speed**: reverse, freeze frames and speed presets.
- **Captions**: Whisper runs on this computer; import and export SRT and WebVTT; a Captions tab.
- **GPU export**: hardware encoders are tried once, with a CPU fallback.

## 0.4.0 — 2026-10-02

### New
- kimchi is now a native app (GPUI) instead of a web view: faster, smaller, and the same on every system.
- **One command registry** for the window, the built-in agent, `kimchi-cli` and MCP, with one undo history.
- **Built-in agent** (Claude Code, Codex, Anthropic, OpenAI, Ollama) with "Revert this run" and permissions.
- Hand-offs with ryolune, and lsuite discovery.

## 0.1.1 — 2026-10-01

### New
- Signed and notarized macOS builds.

## 0.1.0 — 2026-10-01

### New
- The first kimchi: a timeline editor with image and video generation from cloud and local models, bundled ffmpeg and automatic updates.
