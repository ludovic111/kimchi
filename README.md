<p align="center">
  <img src="brand/icon.png" width="112" alt="kimchi" />
</p>
<p align="center">
  <a href="https://lsuite.xyz/kimchi">Website</a> ·
  <a href="https://github.com/ludovic111/kimchi/releases/latest">Download</a> ·
  <a href="https://lsuite.xyz/kimchi/support">Sponsor</a>
</p>

<h1 align="center">kimchi</h1>

<p align="center"><strong>An open-source video editor where generative models are part of the cut.</strong><br/>
Native Rust app (GPUI) · drivable by your AI (MCP, CLI, built-in agent) · bring your own keys, or run everything locally.<br/>
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
- Hardware encoding where the computer has it (Apple VideoToolbox, NVIDIA NVENC, AMD AMF, Intel Quick Sync, VA-API), checked with a test encode and redone on the CPU if it fails; 4K, HEVC and ProRes sources decode in hardware

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

## Drive it from AI and scripts

Everything you can do in the window is a named command (`clip.split`, `generate.extendClip`,
`export.start`…, see [docs/COMMANDS.md](docs/COMMANDS.md)), and the window, the built-in agent,
`kimchi-cli` and `kimchi-mcp` all go through the same registry and share one undo history.

```bash
claude mcp add kimchi -- /Applications/kimchi.app/Contents/MacOS/kimchi-mcp --live   # Claude Code
kimchi-cli project.overview                                                           # the running app
kimchi-cli --file cut.json clip.addText text="Opening title"                           # a project file
```

The **Agent** panel (⌘J) runs the model you already have (Claude Code, Codex, an Anthropic or OpenAI key, or a
local Ollama model), shows one card per command and lets you revert a whole run. What agents may do
(files, projects, generation, settings, quitting) is set in Settings › Agent, for the built-in agent and MCP alike.
kimchi hands cuts to [ryolune](https://lsuite.xyz/ryolune) to score them and takes its audio back
(`handoff.toRyolune`, `handoff.fromRyolune`). Details: [docs/AI_CONTROL.md](docs/AI_CONTROL.md).

## Architecture

```
crates/
  kimchi-core      project model, edits, undo history (shared by every client), on-disk library
  kimchi-media     ffprobe/ffmpeg: probing, previews, export graph, live preview stream, text rendering
  kimchi-gen       the generation harness: Provider trait, 15 providers, keys, job queue
  kimchi-control   the command registry, session, permissions, loopback bridge, lsuite discovery, updater
  kimchi-agent     the built-in agent (Claude Code, Codex, Anthropic, OpenAI, Ollama)
  kimchi-desktop   the window (GPUI), binary `kimchi`
  kimchi-cli       `kimchi-cli`: any command, on the running app or a project file
  kimchi-mcp       `kimchi-mcp`: the registry as MCP tools (`--live`, `--file`)
  kimchi-release   signs updates and writes latest.json for releases
```

- Every change is an `Edit` (plain data) applied by `kimchi-core`, behind a named command in `kimchi-control`.
  The window never mutates the project; it renders what the session holds.
- The preview is rendered by the export's own ffmpeg graph (smaller, from the playhead), and text layers are drawn
  in Rust for both, so what you see is what renders.
- Providers implement one trait (`info`, `models`, `check`, `generate`). The harness handles keys, concurrency (one job at a time on local GPUs), cancellation, downloads and progress events.
- The interface follows the [lsuite design system](https://lsuite.xyz/design): chili coral, glass chrome over a
  tinted backdrop, solid work surfaces, Manrope and IBM Plex Mono, dark and light, tested contrast.

## Install

Grab the build for your system from the [latest release](https://github.com/ludovic111/kimchi/releases/latest)
(or [lsuite.xyz/kimchi](https://lsuite.xyz/kimchi)):

| System | File |
| --- | --- |
| macOS, Apple Silicon | `kimchi_aarch64.dmg` |
| macOS, Intel | `kimchi_x64.dmg` |
| Windows | `kimchi_x64-setup.exe` (installer) or `kimchi_x64-portable.zip` |
| Linux | `kimchi_amd64.AppImage` or `kimchi_amd64.deb` |

ffmpeg is bundled, and so are `kimchi-cli` and `kimchi-mcp`. The macOS build is signed with a Developer ID and
notarized by Apple, so it opens like any other app.

**Updates.** kimchi checks GitHub Releases when it starts and installs a new version in one click (also
`kimchi-cli app.checkUpdates` / `app.installUpdate`). Every update is signed and its signature checked before
anything is replaced; on macOS and with the AppImage the previous copy is kept until the new one has started.
On Windows and with the `.deb`, kimchi tells you about the update and links to the file. Turn the check off with
the setting `updates.checkOnStart` or `KIMCHI_NO_UPDATE=1`. Installs of 0.1.x (the Tauri builds) update to the
new app through their own updater.

## Development

Requirements: Rust (stable). On Linux, GPUI's libraries:
`sudo apt install pkg-config clang libdbus-1-dev libfontconfig-dev libfreetype-dev libssl-dev libvulkan-dev libwayland-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev`
(other distributions: see `script/linux` in the [Zed repository](https://github.com/zed-industries/zed)).
In development kimchi uses the ffmpeg on your `PATH`, or the static build `scripts/fetch-ffmpeg.sh` puts in
`target/ffmpeg/<target>/` if you point `KIMCHI_FFMPEG` and `KIMCHI_FFPROBE` at it.

```bash
cargo run -p kimchi             # the desktop app
cargo run -p kimchi-cli -- help # the CLI
cargo test --workspace
```

Packaging (each writes to `target/dist/`):

```bash
scripts/bundle-macos.sh aarch64-apple-darwin   # kimchi.app, .dmg, .app.tar.gz (ad-hoc signed without a Developer ID)
scripts/bundle-linux.sh                        # .AppImage and .deb (on Linux)
scripts/bundle-windows.sh                      # setup .exe (NSIS) and portable .zip (Git Bash on Windows)
scripts/make-icons.sh                          # .icns, .ico and PNGs from brand/icon.svg
```

The packaging resources (Info.plist template, entitlements, icons, `.desktop` file, NSIS script) are in
`crates/kimchi-desktop/resources/`.

### Releases

Bump the version in `Cargo.toml` (`[workspace.package]`), tag `vX.Y.Z` and push the tag. The release workflow builds
macOS (Apple Silicon and Intel; signed and notarized), Windows and Linux, signs the update files, writes
`latest.json` and drafts the GitHub release; publishing the draft rolls the update out to everyone.

Update signatures use the minisign key of the Tauri builds, through `crates/kimchi-release` (no Node or Tauri CLI):

```bash
cargo run -p kimchi-release -- sign <file> --version X.Y.Z     # key in TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)
cargo run -p kimchi-release -- verify <file>                   # against kimchi's update key
cargo run -p kimchi-release -- manifest target/dist --version X.Y.Z --base-url <release download URL>
cargo run -p kimchi-release -- keygen /tmp/test --password pw  # a throwaway key pair for local testing
```

Secrets the workflow uses: `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (update key), and
`APPLE_CERTIFICATE_P12_BASE64`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY_P8_BASE64`,
`APPLE_API_KEY_ID`, `APPLE_API_ISSUER` (Developer ID and notarization, shared with the other lsuite apps).

## License

[MIT](LICENSE). Originally forked from OpenCut. The bundled ffmpeg is distributed under its own licence (GPL),
included in the app as `FFMPEG-LICENSE.txt`.

If kimchi is useful to you, [sponsoring](https://lsuite.xyz/kimchi/support) keeps it going.
