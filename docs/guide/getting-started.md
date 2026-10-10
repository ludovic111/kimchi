# Getting started

## Install

kimchi is in beta on **Linux** (x86-64) and **macOS** (Apple Silicon and Intel); **Windows is coming soon**. Get it in the
[lsuite app](https://lsuite.xyz/launcher), with no account: it installs and updates
every lsuite app.

| System | File the lsuite app installs |
| --- | --- |
| Linux | `kimchi_amd64.AppImage` or `kimchi_amd64.deb` |
| macOS, Apple Silicon | `kimchi_aarch64.dmg` |
| macOS, Intel | `kimchi_x64.dmg` |
| Windows | Coming soon |

ffmpeg comes with kimchi, along with the `kimchi-cli` and `kimchi-mcp` command-line tools. (The
speech model for captions downloads the first time you transcribe.) The macOS app is signed with a
Developer ID and notarized by Apple.

kimchi checks for updates and offers to install them; see [Updates](settings.md#updates).

## Your first cut

1. **Create a project.** On the home screen, click **Empty project**, or describe a shot in the
   prompt box and press Enter to create a project with the prompt ready in the Generate panel
   (connect a model first; see below). New projects are 1920×1080 at 30 frames per second; pick
   another format (vertical, square…) under the prompt.
2. **Bring in media.** Drop video, image and audio files onto the window, or press ⌘I (Ctrl+I).
   They appear in the **Media** tab.
3. **Place them.** Drag media onto the timeline, or double-click it to put it at the playhead.
   Video and pictures go on video tracks, sound on audio tracks.
4. **Cut.** Press Space to play. Press **S** to split at the playhead, drag a clip's edges to trim
   it, and press ⌫ to delete what's selected (⇧⌫ also closes the gap). Drag a clip to move it; a
   clip dropped over another replaces the part it covers.
5. **Add a title.** Press **T** for a title at the playhead, then type its words in the inspector
   on the right.
6. **Export.** Press ⌘E, choose a format (MP4 plays everywhere), click **Export 1920×1080** and
   choose where to save the file.

There is no Save step: kimchi saves every change as you make it. ⌘Z undoes.

## Generating shots

To make images and video with AI models, connect one first: open **Settings › Models & keys** and
paste an API key from a provider such as OpenRouter or fal, or point kimchi at ComfyUI or another
model server on your computer. Then open the **Generate** tab (⌘2), write a prompt and press
Generate; the result lands on the timeline at the playhead. See [Generation](generation.md).

## Asking the agent

Press ⌘J to open the Agent panel and describe what you want done to the cut. It uses your Claude Code,
Codex, Gemini CLI, an API key, or a local Ollama model. It shows each change it makes, and can undo its whole
run in one click. See [The agent](agent.md).

## Where to go next

- [The window](window.md): every panel, menu and the command palette.
- [Editing](editing.md): the timeline and the inspector in detail.
- [Keyboard shortcuts](shortcuts.md).
