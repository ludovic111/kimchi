<p align="center">
  <img src="brand/icon.png" width="112" alt="kimchi" />
</p>
<p align="center">
  <a href="https://lsuite.xyz/kimchi">Website</a> ·
  <a href="https://github.com/ludovic111/kimchi/releases/latest">Download</a> ·
  <a href="https://github.com/sponsors/ludovic111">Sponsor</a>
</p>

<h1 align="center">kimchi</h1>

<p align="center"><strong>An open-source video editor where generative models are part of the cut.</strong><br/>
Rust core · Tauri UI · bring your own keys, or run everything locally.<br/>
Part of <a href="https://lsuite.xyz">lsuite</a>, the free, open-source creative suite your AI can drive.</p>

---

kimchi is a desktop video editor with a built-in harness for image and video
generation models. Prompt a shot straight onto the timeline, animate a frame you
like, extend a clip from its last frame, or bridge two shots with a generated
transition, then cut, trim and export it like any other footage.

It started as a fork of [OpenCut](https://github.com/OpenCut-app/OpenCut) and was
rewritten from the ground up in Rust.

## What it does

**Editing**
- Multi-track timeline: video, image, text, solid and audio clips
- Split, trim, ripple delete, duplicate, cross-track moves, snapping, markers
- Per-clip transform (position, scale, rotation, opacity, fit), speed, fades, volume
- Live preview compositor, on-canvas move/scale handles
- Snapshot undo/redo for every edit, autosave
- Export to MP4 (H.264), HEVC, ProRes, WebM, GIF or audio-only, through one ffmpeg filter graph

**Generation, woven into the edit**
- **Generate at the playhead.** A placeholder clip appears where the shot will go and turns into the result when it's done
- **Generate in a gap.** Right-click empty space on a track; the shot is sized to fill it
- **Animate this frame.** Turn the frame under the playhead into a moving shot (image-to-video)
- **Extend shot.** Continue a clip from its last frame, landing right after it
- **Bridge two shots.** First frame and last frame, from the clips on either side
- **Restyle frame.** Edit a still with an image model
- **Provenance.** Every generated asset remembers its prompt, model, seed and inputs: regenerate or make variations in one click
- Start a project from a prompt on the home screen, or type one into the command palette (⌘K)

## Models

Keys live in your OS keychain (or come from the usual environment variables) and
are only sent to the provider they belong to.

| Provider | Kind | Images | Video |
|---|---|:-:|:-:|
| [OpenRouter](https://openrouter.ai) | cloud | ✓ | ✓ |
| [fal](https://fal.ai) | cloud | ✓ | ✓ |
| [Replicate](https://replicate.com) | cloud | ✓ | ✓ |
| [OpenAI](https://platform.openai.com) | cloud | ✓ | |
| [Google Gemini](https://ai.google.dev) | cloud | ✓ | ✓ |
| [xAI](https://x.ai/api) | cloud | ✓ | ✓ |
| [Runway](https://dev.runwayml.com) | cloud | ✓ | ✓ |
| [Luma AI](https://lumalabs.ai/api) | cloud | ✓ | ✓ |
| [Black Forest Labs](https://bfl.ai) | cloud | ✓ | |
| [Stability AI](https://platform.stability.ai) | cloud | ✓ | |
| [Together AI](https://together.ai) | cloud | ✓ | ✓ |
| [ComfyUI](https://www.comfy.org) | local | ✓ | ✓ |
| Stable Diffusion WebUI (A1111, Forge, SD.Next, Draw Things) | local | ✓ | |
| [Ollama](https://ollama.com) | local | ✓ | |
| Any OpenAI-compatible images server (LocalAI, vLLM-Omni, sd-server…) | local | ✓ | |

Model lists are fetched live where the provider exposes them, and every model
declares what it can do (aspect ratios, durations, resolutions, first/last frame,
reference images, sound), so the generate panel only shows controls that apply.

### Your own ComfyUI workflows

Export a workflow in API format into `~/Documents/kimchi/comfyui-workflows` (or
set another folder in Settings → Models & keys → ComfyUI). Each file becomes a
model. Put placeholders in any string input and kimchi fills them in:

`{{prompt}}` `{{negative_prompt}}` `{{seed}}` `{{width}}` `{{height}}`
`{{steps}}` `{{cfg}}` `{{denoise}}` `{{duration}}` `{{fps}}` `{{frames}}`
`{{image}}` / `{{start_image}}` `{{end_image}}` (uploaded for you)

A value that is exactly one numeric placeholder becomes a number. Workflows with
a video save node are treated as video models; an optional
`<name>.kimchi.json` sidecar can set the name, tasks and defaults.

## Architecture

```
crates/
  kimchi-core    project model, edits, undo history, on-disk library   (no I/O beyond JSON)
  kimchi-media   ffprobe/ffmpeg: probing, thumbnails, filmstrips, waveforms, proxies, export
  kimchi-gen     the generation harness: Provider trait, 15 providers, keys, job queue
src-tauri/       desktop shell: commands, keychain, job → timeline pipeline
ui/              Svelte 5 interface (thin: every edit is applied in Rust)
```

- Every change is an `Edit` (plain data) applied by `kimchi-core`. The UI never mutates the project; it renders what Rust returns.
- TypeScript types are generated from the Rust types with [ts-rs](https://github.com/Aleph-Alpha/ts-rs) (`npm run bindings`), so both sides can't drift.
- Providers implement one trait (`info`, `models`, `check`, `generate`). The harness handles keys, concurrency (one job at a time on local GPUs), cancellation, downloads and progress events.
- Text is rasterised by the UI with the same canvas code for preview and export, so what you see is what renders.

## Install

Grab the build for your system from the [latest release](https://github.com/ludovic111/kimchi/releases/latest):
`.dmg` for macOS (Apple Silicon or Intel), `.exe`/`.msi` for Windows, `.AppImage`/`.deb`/`.rpm` for Linux.
ffmpeg is bundled. kimchi checks for updates on launch and installs them in one click (updates are signed).

The macOS build is signed with a Developer ID and notarized by Apple (from 0.1.1), so it opens like any other app.

## Development

Requirements: Rust (stable) and Node 22+. `npm run dev` downloads a static ffmpeg for your platform the first time
(`scripts/fetch-ffmpeg.mjs`); on Linux you also need the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

```bash
npm install
npm run dev          # desktop app with hot reload
```

```bash
npm run ui:dev       # UI only, in a browser, with demo data (run scripts/demo-media.sh once)
cargo test           # core, media and provider tests (providers are tested against mock servers)
npm run bindings     # regenerate TypeScript types after changing Rust types
npm run build        # release bundle
```

Releases: bump the version in `Cargo.toml`, tag `vX.Y.Z` and push. The release workflow builds every platform,
signs the update and drafts the GitHub release; publishing the draft rolls the update out.

## License

[MIT](LICENSE). Originally forked from OpenCut. The bundled ffmpeg is distributed under its own licence (GPL),
included in the app as `FFMPEG-LICENSE.txt`.

If kimchi is useful to you, [sponsoring](https://github.com/sponsors/ludovic111) keeps it going.
