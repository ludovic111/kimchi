# Architecture

How kimchi's code fits together: the crates, the path every edit takes, and the renderer, mixer,
generation harness and agent around them. [DEVELOPMENT.md](DEVELOPMENT.md) covers building,
testing and common changes.

## Principles

Four rules shape the code. Most design decisions follow from them.

1. **A feature is a command first.** Every change to a project is a named command in one
   registry (`kimchi-control`). The window, the built-in agent, `kimchi-cli` and `kimchi-mcp` are
   peers that call it, so validation, permissions and undo behave the same for all of them. The
   window never changes the project directly.
2. **A project is plain data, and every change is an `Edit`.** `kimchi-core` owns the document,
   applies `Edit`s, and keeps a snapshot undo history shared by every client.
3. **Every frame comes from the compositor.** The preview and the export use one renderer
   (`kimchi-media/src/render`). ffmpeg only decodes sources and encodes the result.
4. **Every sample comes from the mixer.** Playback, scrubbing, exports and transcription all mix
   through `kimchi-audio`'s `Mixer`.

## Crates

```
        kimchi (desktop)          kimchi-cli ◄── kimchi-mcp
          │        │                   │
          │   kimchi-agent             │
          ▼        ▼                   ▼
   ┌──────────────── kimchi-control ──────────────────┐
   │ registry · commands · session · bridge · …       │
   └────┬─────────────┬─────────────┬─────────────┬───┘
        ▼             ▼             ▼             ▼
   kimchi-gen   kimchi-captions   kimchi-media ──► kimchi-audio ──► ryolune-engine
                                    │             │
                                    └──► kimchi-core ◄──┘
```

| Crate | Role | Main modules |
| --- | --- | --- |
| `kimchi-core` | The project document and everything that changes it. No I/O beyond the library on disk. | `model` (the document), `edit` (`Edit` and `Project::apply`), `history` (undo, batches, checkpoints), `anim` (keyframes, easings), `effects`, `transition`, `presets`, `templates`, `audio` (the mix model), `motion/` (2D and 3D scenes, stacks, particles, evaluation), `mesh/` (polygon modelling), `expr/` (expressions), `store` (the library on disk) |
| `kimchi-audio` | The mixer and everything sound. | `mixer`, `chain` and `plugins` (effects through ryolune's engine), `dsp`, `meter`, `loudness` (EBU R128), `beats`, `devices` (cpal), `record`, `song` (`.ryolune` renders and sessions) |
| `kimchi-media` | ffmpeg, and the compositor. | `render/` (the compositor: 2D, 3D, grading, transitions, render cache), `export`, `preview`, `accel` (hardware encoders), `audio/` (decoding sources for the mixer, mixdown), `text` (titles), `probe`, `speech` |
| `kimchi-captions` | Subtitles and speech to text. | `lib` (SRT and WebVTT, splitting and timing captions), `whisper` (Whisper on candle) |
| `kimchi-gen` | The generation harness. | `provider` (the `Provider` trait), `harness` (jobs, keys, concurrency), `providers/` (15 providers) |
| `kimchi-control` | The command registry and the running session. | `registry`, `commands/` (one file per family; `mod.rs` lists every spec), `session`, `resolve` (names or ids), `bridge`, `settings`, `secrets` (keychain), `harness/` (the agents' brief, skills and live context), `lsuite` (the `~/.lsuite` folder and the lsuite server), `discovery` (lsuite), `diagnostics` (logs, crash reports), `update`, `release_notes`, `renders` |
| `kimchi-agent` | The built-in agent. | `host` (the conversation and runs, behind `agent.*`), `cli` (Claude Code and Codex as subprocesses), `api/` (Anthropic, OpenAI and Ollama tool loops), `tools` |
| `kimchi` (`crates/kimchi-desktop`) | The window, on GPUI. | `app` (`Workspace`), `store` (`Store`), `playback`, `preview`, `actions` (shortcuts and menus), `theme`, `ui/` (components), `views/` |
| `kimchi-cli` | `kimchi-cli`: any command against the running app, a file, or a headless library. | `lib` (`Backend`), `main` |
| `kimchi-mcp` | `kimchi-mcp`: the registry as MCP tools and prompts over stdio. | `main`, `prompts` |
| `kimchi-release` | Signs updates and writes `latest.json`. Not shipped. | `main` |

The diagram shows the main layering; the desktop app and `kimchi-cli` also use `kimchi-media`,
`kimchi-gen`, `kimchi-audio` and `kimchi-core` directly, and `kimchi-control` uses ryolune's engine
for song hand-offs. GPUI comes straight from Zed's repository, pinned to one commit; ryolune's engine
is pinned to one commit of the ryolune repository.

## The life of an edit

Splitting a clip, from a key press to the screen:

1. **The window** handles the action (`Workspace::split` in `app.rs`) and calls
   `Store::run_then("clip.split", {time, clipIds})`, which also selects the new clips afterwards.
   `Store::call` runs the command on the Tokio runtime as `Source::Window`; an error becomes a toast.
2. **The registry** (`registry::call`) finds the spec, then:
   - checks permissions, for agents and MCP clients only (`allowed`);
   - coerces values sent as strings (`"0.5"`, `"[1,2]"`) to their types;
   - validates parameters (unknown names get a "did you mean", types, required ones);
   - waits for the window to attach for window-only commands, and for an open batch to finish
     before a mutating command from outside it;
   - dispatches to the family's handler (`commands::dispatch`).
3. **The handler** (`commands/clip.rs`) resolves names or ids (`resolve.rs`), fills defaults from
   the window's state (the playhead, the selection) and calls
   `Session::apply(label, source, &Edit::Split { … }, None)` (the last argument is the coalesce key).
4. **The session** locks the document, labels the coming undo step with the command and its
   source, and passes the edit to the history.
5. **The history** (`Editor::apply` in `kimchi-core/src/history.rs`) snapshots the project,
   applies the edit (`Project::apply` → `Project::split`), restores the snapshot if it fails, and
   records an undo step if anything changed.
6. **The session saves** the project at once (no autosave timer; a temporary file, then a rename)
   and emits `Event::ProjectChanged`.
7. **The registry** emits `Event::Command` with a record of the call (the Agent panel's cards).
8. **The window** receives the events on its pump (`Store::on_event`), takes a fresh
   `Arc<Project>`, prunes its selection, syncs its UI state back to the session, and redraws.

From `kimchi-cli` or `kimchi-mcp`, the first step differs: the client connects to the app's
[bridge](CONFIGURATION.md#the-bridge), authenticates with the token, and sends the command; the
bridge calls the registry with `Source::Cli`, `Source::Mcp` or `Source::Agent`. With `--file` or
`--headless`, the client hosts its own `Session` instead.

### Undo history

The history keeps a snapshot of the project before each step: simple, and correct for every kind
of edit.

- **Coalescing**: edits with the same `coalesce` key within 1.2 s fold into one step (drags,
  sliders, nudges, typing). An edit that changes nothing makes no step. At most 300 steps.
- **Batches**: `project.batch` validates every command first, then runs them as one step; an
  atomic batch rolls back if any command fails, even when the caller goes away mid-way. Undo is
  refused while a batch is open.
- **Checkpoints**: a saved copy of the project (the latest 64 are kept) that `history.revertTo`
  returns to, as one undoable step. The agent takes one before its first change in each run
  ("Revert this run"); the bridge takes one before an agent or MCP connection's first change, and
  again after the open project changes.
- **Background edits** (`Edit::is_background`): adding or updating an asset and landing a
  generation are applied to every snapshot, so undoing never loses a finished generation or a
  computed thumbnail.
- Each step records its label and source (`window`, `agent`, `cli`, `mcp`); `history.list` shows
  them.

The history lives in memory; reopening a project starts a new one.

## Commands

Each command is a `Spec` in `kimchi-control/src/commands/mod.rs` (`SPECS`, 196 of them):

```rust
edit("clip.split", "Split clips at a time. Without clipIds, every clip under that time on unlocked tracks (or the selection in the window).", &[
    opt("time", Number, "Timeline time in seconds. Defaults to the playhead."),
    opt("clipIds", Array, "Clips to split (ids or names).").of(String),
]),
```

- `query(…)` reads and `edit(…)` changes the project. `.perm(Perm::Files)` (or `Projects`,
  `Generate`, `Settings`, `AppControl`, `PersonOnly`) gates it for agents; the default `Edit` is
  always allowed. `.window()` marks commands only the running window can do.
- Parameters have a name, a `Kind` (string, number, integer, boolean, array, object, any), whether
  they're required, and a description. Commands that drags and sliders call list `coalesce`; the
  registry accepts it on any mutating command.
- The same specs generate `docs/COMMANDS.md` (`registry::markdown`), the MCP tools and the agent's
  tools (`registry::input_schema`, JSON Schema with `additionalProperties: false`), and
  `kimchi-cli help`.
- Handlers live in one file per family, larger families split over several (`commands/clip.rs`,
  `audio.rs`; `motion.rs`, `motion_edit.rs`, `motion_mesh.rs`, `motion_camera.rs`), and
  return `Result<Value, String>`, where the error is a sentence written for the caller.

**Window commands.** `ui.*`, playback (`timeline.seek`, `timeline.play`…), some `audio.*` and
`app.quit`-like commands need the window: the session sends them over a channel to
`Workspace::ui_command` (`app.rs`), which acts as a key press or click would. `ui.action` runs any
action from the shortcut table by name. `agent.*` commands go to the agent host instead.

## Rendering

`kimchi-media/src/render/mod.rs` composes a frame at time *t*:

1. Fill the background.
2. For each visible video track, bottom first: take the clip under *t*, or a **mix** of two clips
   inside a transition's span (`transition::spans`).
3. Draw each clip:
   - **placement**: `Clip::placement_at(t)` evaluates the transform keyframes, times the fades;
   - **picture**: a still, a frame from the clip's decoder, a title (`text.rs`), a solid, or a motion
     scene (`flat::draw` for 2D, `space` for 3D; or the clip's render-ahead file when it is
     current);
   - **grade**: chroma key, corrections, LUT, sharpen and vignette (`grade.rs`), and blur;
   - **composite** with opacity.
4. Transitions draw both clips into layers and blend them (`mix.rs`).

**Decoding.** Each playing video clip has its own ffmpeg process sending raw RGBA frames
(`source.rs`), started shortly before it is needed; reversed clips decode in chunks backwards.
Single frames for scrubbing are decoded in parallel.

**3D.** One process-wide `Space` draws 3D scenes on the GPU through wgpu (`space/gpu.rs` and
`gpu.wgsl`), or with the CPU rasteriser (`space/cpu.rs`) when there is no hardware adapter
(`KIMCHI_GPU=any` accepts a software one such as llvmpipe), when `KIMCHI_GPU` is `0`, `off`, `false`
or `cpu`, or when the GPU fails (it then stays on the CPU). The shading exists twice and must match:
`space::shade` for the CPU and `gpu.wgsl`; `gpu_matches_cpu` renders the same scenes on both and
compares them. The path tracer (`space/trace.rs`, with a BVH and a denoiser) runs on the CPU, for
final frames and progressively in the Studio's Rendered view.

**Quality.** `Quality::Preview` (the window) and `Quality::Final` (exports and renders ahead)
differ in motion blur sampling, the path tracer and similar costs.

**Render cache.** `render/cache.rs` renders a motion clip ahead into a lossless FFV1 Matroska file
with alpha. Its key hashes the scene, the project's size and frame rate, and the metadata of the
media it uses; when the key changes, the clip is drawn live again.

**Preview.** Scrubbing renders single frames (`Renderer::still`, at most 1280 pixels on the longer
side) on a blocking task, dropping requests while one is in flight. Playback runs a `PreviewStream`:
one task renders frames ahead into a short queue, and a mixer thread fills the audio buffer. The
audio device's position is the clock: the window shows the latest frame that is due.

**Export.** `export::export` mixes the sound first (offline), then spawns ffmpeg with the mixed
sound as a file input and raw frames on stdin, from `Renderer::for_export` (originals rather than
proxies; errors instead of skipping). With the hardware encoder chosen automatically, a failing
encoder is retried on the CPU, reusing the mix. Output goes to a temporary file that is renamed when
complete.

## Sound

`kimchi_audio::mixer::Mixer` is built from the project (`Mixer::new(project, opener, rate, mode)`) and
renders stereo frames. In `Mode::Realtime` a source that isn't ready plays silence; in
`Mode::Offline` it waits. `Mixer::update` takes a new project while playing and keeps effect
instances whose slot ids still exist, so edits are heard without a restart.

Sources are opened through `SourceOpener`: `kimchi_media::audio::FfmpegOpener` starts one ffmpeg
per playing clip, with speed, reverse and pitch already applied.

- **Preview**: a `kimchi-mixer` thread renders 512-frame chunks at 48 kHz, eight ahead; a cpal
  output stream (`kimchi-desktop/src/preview.rs`) plays them, resampling for the device.
- **Export, loudness, speech**: the same mixer, offline, through `kimchi_media::audio`. Beat
  detection reads a media item's own sound through the same ffmpeg opener, without the mixer.

Effects and plugins come from ryolune's engine: its stock effects, CLAP, VST3 and Audio Units.
Plugins are scanned in a child process (`plugins::scan_child`), so a crashing plugin doesn't take
the app down. The mixer holds plugin instances and must be used on the thread that created it.

## Generation

```rust
#[async_trait]
pub trait Provider {
    fn info(&self) -> ProviderInfo;
    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>>;
    async fn check(&self, cx: &Ctx) -> GenResult<String>;
    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput>;
}
```

The `Harness` owns the providers, their settings and keys, a model-list cache, and the jobs. Each
provider has a semaphore: six jobs at a time in the cloud, one locally. A job goes Queued →
Running → Succeeded, Failed or Cancelled; every change is broadcast.

`generate.submit` (`commands/generate.rs`) first places a **pending** clip (a dry run on a copy,
then the real edit); if it can't be placed, the job is cancelled, so nothing runs without a place
to land. A listener started with the session forwards job updates as events and, when a job
finishes, **lands** it: it probes the outputs, adds them as assets with their provenance, and
replaces the pending clip (`Edit::ResolvePending`), or removes it (`Edit::DropPending`). Both are
background edits.

## The agent

`kimchi_agent::Host` holds the conversation and its runs and answers the `agent.*` commands; the
Agent panel draws its snapshots. A run either:

- **drives an API model** (`api.rs`): a tool loop over streaming Anthropic, OpenAI or Ollama
  clients, with the registry's commands as tools (`tools.rs`; never `PersonOnly` or `agent.*`).
  Each tool call is `kimchi_control::call(…, Source::Agent, …)`; or
- **drives Claude Code or Codex** (`cli.rs`): a subprocess per turn with only kimchi's MCP server
  attached (`kimchi-mcp --live` pointed at this app's bridge), its own tools and the person's other
  MCP servers turned off. Its calls arrive through the bridge as `Source::Mcp`.

Either way, the agent takes a history checkpoint before its first change, so **Revert this run**
is one `history.revertTo`. An API run stops after 40 model steps.

## The window

`kimchi-desktop` is a GPUI app.

- `main.rs` starts logging and crash reports, a Tokio runtime and the `Session`, the bridge, and
  the lsuite registration, then opens the window.
- `Workspace` (`app.rs`) is the root view: home or the editor, dialogs, and every action handler.
  Handlers that change the project end in `Store::run`.
- `Store` (`store.rs`) mirrors the session (the project, jobs, exports, renders, providers,
  settings) and adds the window's own state (selection, zoom, panels, dialogs, toasts, clipboard).
  Views observe it.
- Views are entities that keep their retained state (text fields, scrubs, subscriptions). The
  side panels and the timeline body render cached.
- `actions.rs` holds `SHORTCUTS`, the one table of keys that binds them, fills the shortcut sheet
  and the palette, and names keys in tooltips (`tip`, `hint`). `ui/layout.rs` solves the editor's
  layout and its drawers. `theme.rs` reads the lsuite design tokens (`assets/tokens.json`).
- Playback is its own entity, so time ticking redraws only what shows time.

## The other clients

- **`kimchi-cli`** runs one command (or a batch) against the running app over the bridge, against a
  project file (`--file`, refused if the app has that file open), or on a headless session over the
  library (`--headless`). It also generates `docs/COMMANDS.md` (`docs`) and checks the setup
  (`doctor`).
- **`kimchi-mcp`** serves the registry over MCP on stdio: tools named `family_verb`, annotated as
  read-only or destructive, plus prompts that start common jobs (`rough-cut`, `title-and-captions`,
  `generate-b-roll`, `review-the-cut`, `motion-design`, `3d-scene`). On Unix it points standard
  output at standard error once the protocol is set up, so a stray print can't corrupt the protocol.
