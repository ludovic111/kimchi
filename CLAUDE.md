# kimchi

Desktop video editor with generation in the cut. A native Rust app: the window is **GPUI** (direct
upstream, pinned to Zed commit `7733b99…`, `runtime_shaders` on macOS so no Metal toolchain is needed),
and every action goes through one command registry. See README.md.

```
crates/kimchi-core      project model, edits, undo history (labels, sources, batches, checkpoints),
                        keyframes/easings (anim.rs), motion scenes 2D+3D (motion.rs), presets, templates,
                        clip effects and looks (effects.rs), transitions (transition.rs)
crates/kimchi-audio     Rust mixer, loudness, beats, devices/recording, Ryolune effects/plugins/songs
crates/kimchi-media     ffmpeg probe/decode/encode; the compositor (render/: clips, flat.rs 2D motion,
                        space/ 3D on wgpu or the CPU rasteriser, grade.rs colour/key/LUT, mix.rs
                        transitions, source.rs decoders incl. reverse), export, preview, speech mix
crates/kimchi-captions  SRT / WebVTT, caption splitting, Whisper speech to text (candle, CPU)
crates/kimchi-gen       generation providers and job queue
crates/kimchi-control   registry (commands/mod.rs lists every spec), session, permissions, bridge,
                        lsuite discovery, hand-offs with ryolune, updater
crates/kimchi-agent     the built-in agent (Claude Code, Codex, Anthropic, OpenAI, Ollama)
crates/kimchi-desktop   the window (package/binary `kimchi`): store.rs, app.rs, views/, ui/, theme.rs
crates/kimchi-cli       `kimchi-cli`;  crates/kimchi-mcp: `kimchi-mcp`;  crates/kimchi-release: signing
crates/kimchi-plugin    the video plugin SDK (frozen repr(C) ABI 1 in ffi.rs, GUIDE.md, template/)
plugins/examples        Halftone, Chromatic aberration, Gradient, Radial wipe: stock plugins on the SDK
```

Rules that keep it working:

- **Every frame comes from the compositor** (`kimchi-media/src/render/mod.rs`), for the preview and
  the export alike: ffmpeg only decodes (one raw-RGBA stream per playing clip) and encodes (frames on
  stdin). Clip keyframes are applied through `Clip::placement_at`; motion scenes are evaluated with
  `Layer::at` / `Object3d::at`. The 3D shading exists twice, `space::shade` and `space/gpu.wgsl`:
  change both (`gpu_matches_cpu` compares them; `KIMCHI_GPU=any` runs the GPU path on llvmpipe).
- **Motion scenes are JSON agents write**: `Scene::from_json` checks unknown fields, types, ids,
  property names and colours with "did you mean" errors; keep `motion_guide.md` (what `motion.guide`
  returns) in step with `motion.rs`. Enum struct variants need their own `rename_all = "camelCase"`.

- **A feature is a command first.** Add the spec in `kimchi-control/src/commands/mod.rs`, the handler
  in its family file, then call it from the window with `Store::run`. Never change the project from
  the window directly. Regenerate `docs/COMMANDS.md` with `cargo run -p kimchi-cli -- docs` (a test
  fails otherwise). Commands only the window can do (`ui.*`, playback) are handled in
  `Workspace::ui_command` (app.rs).
- **Docs follow the code** (index: `docs/README.md`). A change people see in the window updates its
  chapter of the user guide (`docs/guide/`); a new setting, environment variable or file updates
  `docs/CONFIGURATION.md`; a new project field updates `docs/PROJECT_FORMAT.md`. Same plain voice.
- GPUI API: grep the pinned source in `~/.cargo/git/checkouts/zed-*/7733b99/crates/gpui`; the
  `build-gpui-apps` skill (installed in `.claude/skills/`) covers this revision. Views keep retained
  state (text fields, scrubs, subscriptions) in their entity; drags use `ui::drag::track`; icons
  are `ui::icon` (inherits the text colour); panels in the editor are cached views.
- Shortcuts: one table, `actions::SHORTCUTS` (binds the keys, fills the `?` sheet and the palette);
  tooltips and menus name keys with `actions::tip` / `hint`, never a hard-coded ⌘ (Linux and Windows
  show Ctrl). Things that appear animate in with `ui::motion` (GPUI skips it under reduce motion).
- Look: black and white, square corners, grain (`theme.rs`, `ui/grain.rs`). The accent is white in the
  dark mode and black in the light one (a selected choice is inverted, paper on ink); red only for
  danger. Radii in `theme::size` are zero, floating surfaces cast a hard offset shadow
  (`glass_shadow`, `chip_shadow` for the primary button), dialogs and the home composer sit in corner
  brackets (`grain::brackets`). Every area is titled like the side panels (`ui::panel_title`: Viewer,
  Timeline), and toolbars are boxed groups of what goes together (`ui::group` with `Button::flush`,
  `ui::tool` for icon + label that drops its label when room runs short). Logos of other services keep
  their own colours; kimchi's mark and app icon are one colour (`scripts/gen-mark.py` writes
  `brand/mark.svg`, `brand/icon.svg` and the window's `mark.svg`; then `scripts/make-icons.sh`). The page is film grain and dithered light drawn at device pixels
  (`grain::backdrop`, `grain::dither`; fixed sizes, each cached). Interface face: Chakra Petch
  (`crates/kimchi-desktop/fonts/chakrapetch`), IBM Plex Mono for numbers and labels. Colours no
  longer come from `assets/tokens.json` (still the lsuite copy). GPUI has no backdrop blur: tier 1
  is translucent over the window's own backdrop (native blur behind on macOS), tiers 2–3 are their
  tint over the raised surface. `theme.rs` has the contrast test.
- **Releases have notes**: the version's section at the top of `CHANGELOG.md` (a test checks it matches the
  workspace version) is what "What's new" shows after an update and what the release workflow puts in
  `latest.json`. Write it for people, in the same plain voice.
- Logs and crash reports (`kimchi-control/src/diagnostics.rs`): `tracing` goes to `<data>/logs/kimchi.log` too, a
  panic writes `logs/crashes/crash-*.txt`, a run that ends without quitting leaves `unclean-*.txt` at the next
  start. Log with `tracing`, never `println!` (stdout is MCP's protocol in `kimchi-mcp`).
- Testing the app: `source` an env that sets `KIMCHI_DATA_DIR`, `KIMCHI_CONFIG_DIR`, `LSUITE_HOME`,
  `KIMCHI_NO_UPDATE=1` to scratch folders, run `target/debug/kimchi`, drive it with
  `target/debug/kimchi-cli …`, look with `kimchi-cli ui.screenshot path=…`. Debug builds don't read
  the keychain (`KIMCHI_KEYCHAIN=1` forces it; an unsigned build makes macOS ask for the login
  password). On Linux without a desktop: `Xvfb :77` + `openbox`, `DISPLAY=:77`, screenshots with
  `import -window <id>` (ImageMagick; `ui.screenshot` is macOS-only) and clicks with `xdotool`. UI tests run the real views headless (`crates/kimchi-desktop/src/tests.rs`,
  `views/timeline/tests.rs`). On the Linux box the `rustc-low-priority` wrapper is a `/bin/sh` script and dash drops
  env vars with hyphens, so the `kimchi-cli` / `kimchi-mcp` integration tests (`CARGO_BIN_EXE_kimchi-cli`) need
  `RUSTC_WRAPPER= cargo test …` (workspace crates rebuild, dependencies don't).

## lsuite (notes updated 2026-10-07, 0.11.0)

kimchi is part of **lsuite** with ryolune (music) and zenith (code); its page is lsuite.xyz/kimchi
(`../lsuite/kimchi/index.html`). Contract: `../lsuite/STANDARD.md` and `../lsuite/design/DESIGN.md`.

- [x] **Command registry**: 200 `family.verb` commands (project, media, track, clip, transition,
      captions, audio, motion, timeline, history, generate, export, handoff, app, agent, ui), one undo history for every client,
      batches as one step, `project.overview`, names or ids everywhere. Everything the window does has a command:
      `agent.*` drives the Agent panel's conversation (`kimchi_agent::Host`), `ui.action` runs any shortcut or
      menu item by name, `ui.setTimeline` / `ui.setLayout` / `ui.reveal` cover the window's own options.
- [x] **CLI**: `kimchi-cli <command>` on the running app or `--file project.json`; `batch`, `doctor`,
      `mcp-config`, `docs`.
- [x] **MCP**: `kimchi-mcp --live | --file | --headless`, tools generated from the registry; docs in
      `docs/AI_CONTROL.md` and generated `docs/COMMANDS.md`.
- [x] **Built-in agent**: Agent panel (⌘J), Claude Code / Codex / Anthropic / OpenAI / Ollama, one card
      per command, changes with "Revert this run", permissions (`settings.agent.permissions`) checked in
      `registry::call` for agent and MCP alike. Codex untested (not installed on the dev Mac).
- [x] **Auto-update**: `app.checkUpdates` / `app.installUpdate`, `KIMCHI_NO_UPDATE=1`, setting
      `updates.checkOnStart`; signed with the Tauri-era minisign key, `latest.json` in the Tauri
      format so 0.1.x installs update to this app. Since 0.11.0 through lsuite (below).
- [x] **Design system v2** (2026-10-06): black and white, square, grain, Chakra Petch + IBM Plex Mono
      (bundled), tiers over the grain, solid work surfaces, macOS window blur, dark and light, contrast
      test, one-ink mark and icon (`scripts/gen-mark.py`). See "Look" above and `../lsuite/design/DESIGN.md`.
- [x] **Discovery and hand-offs**: `~/.lsuite/apps/kimchi.json` (format 1, defined here, see
      `kimchi-control/src/discovery.rs`), `handoff.toRyolune` / `handoff.fromRyolune` through
      ryolune's bridge.
- [x] Support links go to `https://lsuite.xyz/kimchi/support`.
- [x] **lsuite AI** (lsuite `AI.md`, 0.10.0): `kimchi-control/src/account.rs` (the shared `~/.lsuite/account.json`,
      0600, atomic; loopback sign-in on 127.0.0.1 with `state`, or a pasted `lsk_` key; `/api/account/me` for plan and
      allowance; `LSUITE_ACCOUNT_SERVER`), `account.status/plans/signIn/signOut` (signing in and out is person-only),
      `kimchi-agent/src/lsuite.rs` + the `lsuite` provider (first in `ProviderKind::ALL`, group "No setup"; the
      Anthropic wire at `<server>/api/ai` with the token; errors explained in one line, the allowance one starts with
      `lsuite::ALLOWANCE` and the panel adds Manage plan), Claude Code on lsuite AI with `agent.claudeCodeOnLsuite`
      (`ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`). Window: `views/lsuite.rs` (the account card, used in Settings ›
      Agent, the Agent panel's notice and the setup's assistant step), `Store::account`. Decided: new installs default to
      `agent.provider = "lsuite"` (old settings keep theirs); the lsuite mark is the site's `assets/img/lsuite.svg`.
      Tests: `account.rs` (wiremock: loopback, key, revoked) and `kimchi-agent` (`lsuite_ai_runs_on_the_account…`).
- [x] **Plugins** (lsuite `PLUGINS.md`, 0.10.0): SDK `crates/kimchi-plugin` (frozen ABI 1 modelled on ryolune's
      `sdk/src/ffi.rs`: entry + `repr(C)` vtables, JSON manifest, panics caught and the instance poisoned), host
      `kimchi-media/src/render/plugins/` (`native.rs` lsuite bundles + stock linked in, `frei0r.rs`, `catalogue.rs` scan
      in a child process with `--scan-video-plugin`, cache, folder watch; `pool.rs` instances per clip slot, remade when
      `plugins::generation()` changes = hot reload; libraries are loaded from a copy per build in `<data>/plugins/loaded`),
      `bundle.rs` (`plugin.toml`). Commands in `commands/plugins.rs` and `plugin_dev.rs` (toolchain, `plugin.new` from
      `crates/kimchi-plugin/template/`, `writeSource` confined to the crate, `build` with errors as data,
      `publishLocal`), permission `agent.permissions.plugins` (off by default). A plugin that fails is switched off and
      saved in `plugins.disabled`. Window: the Plugins dialog (`views/dialogs/plugins.rs`: Stock, Installed, Formats, Build
      with your agent) and the inspector's Plugins section. No OpenFX (the WIP on `t3code/090-hosts` was dropped: not
      loaded, not listed). Format logos: lsuite, frei0r, CLAP, VST3, ryolune; LUTs and Audio Units have none.
- [x] **Agent harness** (lsuite `HARNESS.md`, 0.11.0): `kimchi-control/src/harness/` (`brief.md` = the expert brief, one
      source for the built-in agent's system prompt (`kimchi_agent::system_prompt`) and `kimchi-mcp`'s instructions, with
      command names rewritten to tool names by `harness::as_tools`; `skills/*.md` = 13 playbooks, each `# Title`, `When:`,
      `## Steps`, `## Checks`; tests check every command they name exists and the brief stays 800–1,500 words;
      `context.rs` = the live context, moved from `kimchi-agent`), `commands/harness.rs` (`harness.brief/skills/skill/
      context/look`; `harness.look` is in `vision::LOOKS`). Live context before every model step: `api::refresh_context`
      appends an updated `<context>` block to the tool results when it changed, with what others changed
      (`Session::edits_since`, fed by `registry::call_in`; `harness.context` and checkpoints aren't recorded). Over MCP the
      same block ends a tool result when something changed, edits add a finish-routine reminder until the agent looks or
      measures, and both also go in `structuredContent.harnessNotes` (Claude Code shows the structured result *instead of*
      the text; without it the finish routine was skipped) (`KIMCHI_MCP_CONTEXT=0`); prompts are the skills (old prompt
      names are aliases), resources `kimchi://brief`, `kimchi://skills/<name>`. Per-run checkpoint unchanged.
      `kimchi-cli --file x.json ask "…" [--provider --model --json]` runs the built-in agent headless (CLI providers get a
      private bridge, `Session::bridge_path`). Evals: `evals/run.py` (12 jobs in `evals/jobs.json`, fixtures made with
      ffmpeg, scores in `evals/RESULTS.md`); add a job when a skill is added, and run them before a release.
- [x] **Distribution through lsuite** (lsuite `DISTRIBUTION.md`, 0.11.0): the updater reads
      `<server>/api/apps/kimchi/latest.json` with the account's token (`update::source`; the token only goes to the lsuite
      server's origin, also for downloads); signed out or a refused token → `UpdateStatus.sign_in` and `update::SIGN_IN`,
      not an error; `KIMCHI_UPDATE_URL` still overrides. `release.yml` only makes a draft; `scripts/publish-build.sh
      <version>` copies it to `ludovic111/lsuite-builds` as `kimchi-v<version>` (checks SHA256SUMS, refuses a published
      release or an existing target) and deletes the draft. Never publish the draft in this repository.
- [x] **Real logos** of the editors kimchi really works with (`ui/logos.rs`, `assets/logos/SOURCES.md`): setup, Home,
      the export menu, Settings › Keyboard. OpenShot, Natron and Audacity keep initials (kimchi opens nothing of theirs).

## Animation, motion graphics and 3D (released in 0.5.0, 2026-10-03)

Keyframes with easings on every clip, presets, motion clips (2D layers and 3D scenes), 15 templates,
`motion.*` commands, `motion.guide`, `project.renderFrame`, the MCP prompts `motion-design` and
`3d-scene`; in the window the Motion tab (templates with drawn previews, new 2D/3D scene), the
inspector's Animation section (keyframe toggles at the playhead, easing, presets), template values,
and a scene editor (pick a layer/object/camera, edit and keyframe it, add shapes, text, pictures, 3D
objects, lights). The compositor replaced the ffmpeg overlay graph. Linux: `gpui` now gets its
`x11`/`wayland` features (it ran headless before, with no window).

- [ ] 3D on a real GPU: only checked on llvmpipe (Vulkan) on Linux; check Metal on an Apple Silicon Mac.
- [ ] Speed: 1080p export of heavy 3D on the CPU is ~3 frames/s in release (fine on a GPU); big blurs
      ~5 frames/s at 1080p. The preview (≤ 1280 px) keeps up.
- [ ] Not done: dragging motion layers on the canvas (only clips), audio waveform for volume keyframes.

## Transitions, colour, speed and captions (released in 0.5.0, 2026-10-03)

Built on `t3code/verify-prs` (motion + shortcuts + export fixes). Transitions live on the incoming
clip (`Clip.transition`), centred on a cut with both clips playing past it (no overlap on the track,
spans never overlap: `transition::effective_length`), sound crossfades in the export graph
(`export::with_crossfades`). Effects are `Clip.effects` (keyframable numbers), graded on the media
picture or on the layer. `Clip.reverse` plays the source range backwards (chunked reverse decoder,
`areverse`). Captions are titles on a track with `captions: true`; Whisper models download to
`<data>/models/whisper-{tiny,base,small}` from Hugging Face. Window: inspector Colour / Transition in
/ Timing extras, timeline transition badges (drag edges, right-click kinds) and + on cuts, the
Captions tab (⌘5), a Captions row in the export dialog.

- [ ] Look at all of it on the Mac (on Linux, `ui.screenshot` comes back blank but `vscreen shot` works): inspector sections, badges,
      the Captions tab, transitions and colour in the preview.
- [ ] Whisper runs on the CPU (base: ~5 s for 11 s of speech on the 4-core Linux box, release); Metal
      through candle would be faster on Apple Silicon.
- [ ] Not done: an eyedropper for the chroma key colour, word-level caption timing (captions are
      timed per segment, tightened to the speech's energy), a reversed waveform on reversed clips.

## Stability pass, logs, what's new, updates everywhere (0.6.0, released 2026-10-03)

A bug hunt over every crate (data loss in failed edits and batches, duplicates sharing media, panics on odd
input, providers giving up on one network blip, Codex reaching the person's own MCP servers), media
compatibility (EXIF photos, alpha WebM, HDR, NTSC, hundreds of cuts, odd paths), window fixes (shortcuts while
typing or behind dialogs, Ctrl+Enter off macOS, focus, window buttons on Windows / client-decorated Linux), logs
and crash reports (Settings › Diagnostics, Help menu, `app.logs` / `app.crashReports` / `app.diagnostics`),
What's new (`app.whatsNew`, `CHANGELOG.md`), updates every 6 h with optional auto-install, Windows installs through
the verified NSIS installer on restart or quit (`update::apply_on_quit`), `app.restart`.

- [ ] Windows: launch the 0.6.0 build; check the window buttons, dragging by the top bar, and an update from 0.6.0
      to the next release through the installer (untested: no Windows machine here).
- [ ] macOS: look at What's new, Diagnostics and the toasts; check a SIGTERM (log out) leaves no "didn't quit
      properly" notice.

## Blender-like 3D, After Effects-like 2D, the Studio, render ahead (0.7.0, released 2026-10-04)

The motion model is `kimchi-core/src/motion/` (mod.rs: Scene, validation, `ItemMut`; space.rs 3D; flat.rs 2D;
stack.rs: typed stacks with parameter tables — modifiers, constraints, patterns, effects, operators, masks,
animators — that drive validation, `motion.stackTypes`, the Studio's forms and keyframe names
`<field>.<id>.<param>`; particles.rs stateless particles; curve.rs; eval.rs keyframes → expressions → constraints,
`Scene3d::evaluate_at`). `kimchi-core/src/expr/` is the expression language (its `GUIDE` is spliced into
`motion_guide.md` at `EXPRESSIONS_GUIDE`); `kimchi-core/src/mesh/` the polygon meshes, primitives, modifiers, CSG and
edit ops (`ops::EDIT_OPS`). Rendering: `render/flat.rs` + `effects2d.rs`, `masks.rs`, `shapeops.rs`, `textfx.rs`,
`particles2d.rs`; `render/space/` (env, pattern, post, shapes, particles, models, viewport) and the path tracer
(`trace.rs`, `bvh.rs`, `denoise.rs`); `render/cache.rs` renders motion clips ahead (FFV1 with alpha, `Clip.rendered`,
valid while `cache::key` matches). Commands: `commands/motion_edit.rs`, `motion_mesh.rs`, `renders.rs` (background
renders, `Event::Render`). Window: `views/studio/` (`ui.studio`).

- [ ] The Studio by hand with a mouse: orbit, gizmo drags, outliner drag-reorder, dope-sheet and graph drags were
      only exercised by UI tests (vscreen can't drag); feel on a real GPU (llvmpipe/CPU here).
- [ ] 3D on Metal (Apple Silicon) and DirectX 12: the new shaders (spot/area lights, environment mips, shadow array)
      were checked on llvmpipe/Vulkan only.
- [ ] Path tracer: CPU only (about 30 s for 960×540 at 64 samples on 4 cores); a GPU path tracer would be the next step.
      The standard engine now supports point-light cube shadows; area lights remain a representative point there.
- [ ] Not done: sculpting, rigging/armatures, physics, 2D puppet/mesh warp, 3D layers in 2D, extruding a picture's
      outline, audio-driven expressions.

## Responsive UI, camera navigation and audio (0.8.0)

All sound for preview, export, scrub and transcription comes from `kimchi-audio::mixer::Mixer`.
FFmpeg decodes individual sources and encodes the mix; Ryolune's pinned engine supplies the stock
and external effects, song rendering and `.ryolune` document format. Core audio settings are in
`kimchi-core/src/audio.rs`; commands in `commands/audio.rs`; the window's mixer, effects, recorder
and export options in `views/mixer/`; clip controls in `views/inspector/audio.rs`. `recording.rs`
connects the microphone recorder to count-in, placement and undo. Imported media intentionally
stays in the media library after undoing its placement, as other imports do.

Editor layout is solved in `ui/layout.rs`. Studio panels become drawers in narrow windows, and
its toolbar wraps. The navigation gizmo provides orbit, pan and zoom drags; the Camera menu
controls the scene camera and editable move presets (`motion.cameraMove`). `ui.studio` exposes
navigation and the narrow-window drawers. Headless window tests must pump GPUI for window
commands such as `timeline.seek`; calling them through a blocking fixture call deadlocks.

Renderer work includes parallel row compositing, cached colour/noise calculations, faster large
blurs, GPU buffer reuse, path-tracer improvements and point-light cube shadows (up to two point
lights). Both CPU and GPU implementations must remain in agreement. Use the `render_bench`
example and `docs/PERFORMANCE.md` for measurements; don't infer hardware GPU performance from
llvmpipe. Live recording, placement and undo were checked against the virtual audio device;
a 48 kHz mono WAV completed with no dropped samples. Physical microphones/speakers, macOS
Audio Units/Metal and Windows devices still need checks on those systems.

## Design v2, agents that see, conversations, sound generation, Studio workbench (0.9.0, released 2026-10-06)

Integrated by a coordinator session from parallel sessions: #10 design system v2 (black/white, square,
grain `ui/grain.rs`, Chakra Petch, new mark), #8 docs (`docs/guide/`, configuration, project format,
architecture), #12 harness (`kimchi_control::vision`, `Part::Image`, `media.look`, `kimchi_agent::context`
block per request, MCP image content), #11 Studio/agent (two sidebars: everything left, agents right;
conversations per project with memory, `agent.steer`, model selector, zenith provider in
`kimchi-agent/src/zenith.rs` using zenith-cli `thread.new/send/get/interrupt/steer`, ElevenLabs and Stable
Audio sound generation, Studio modelling workbench and animation tools). CI and releases fetch
`ryolune-engine` over SSH with the `RYOLUNE_DEPLOY_KEY` deploy key (ryolune was private for a day; it's public again).

## Compatibility with other editors, first-run setup, more agent providers (0.9.1, released 2026-10-06)

`kimchi-interop` (apps people switch from, OTIO/FCPXML/xmeml/EDL readers and writers, look formats, `Report`),
`project.formats/importFrom/exportTo`, `media.relink`, `looks.*` + `export.presets`, `app.onboarding/finishOnboarding/keymaps`
(first-run setup, Settings › Keyboard), the OpenAI-compatible family, Gemini and Bedrock agent providers
(`agent.models`), `docs/COMPATIBILITY.md` (generated, a test checks it), `docs/SWITCHING.md`. Plus #13's fixes.

- [x] Video plugins shipped in 0.10.0 (built from `t3code/090-plugins` and the frei0r half of `t3code/090-hosts`;
      OpenFX dropped). Still on their branches: ryolune LV2/LADSPA (`claude/lv2-ladspa`), extra sound tasks (`t3code/090-gen`,
      fold onto #11's ElevenLabs), Kdenlive/Shotcut/OpenShot/CapCut/.prproj files, new export formats.
- [ ] The new windows (setup, import/export, look picker) were only checked by headless UI tests; look at them.
- [ ] Docs-found, still open: `kimchi-cli` bypasses agent permissions; no menu bar on Linux/Windows.

## Plugins and lsuite AI (0.10.0, 2026-10-07, overnight)

Done overnight by an agent (see the lsuite notes above for the pieces). Checked: the workspace tests (the plugin recipe
end to end in `kimchi-control` tests: new → build with an error → fix → publish → on a clip in a rendered frame → rebuilt
and used at once → switched off → removed), the loopback sign-in and an agent run against the site's real demo server
(`../lsuite` `server.js` + `ai.js`, port 4331), 158 real frei0r filters scanned and drawn, hot reload in the running
window on vscreen.

- [ ] macOS: the loopback sign-in (`open` the browser), loading copies of a plugin's `.dylib` (hot reload; dyld and
      install names), frei0r from Homebrew (`/opt/homebrew/lib/frei0r-1`), `plugin.build` with the Dock's bare PATH.
- [ ] Windows: none of it was run (DLL copies, `rundll32` for the browser).
- [ ] "Build with your agent" with a real model: the demo server answers with a canned line, and no API key is on this
      machine, so the agent never wrote a plugin by itself; the recipe commands are tested directly.
- [ ] The inspector's Plugins section: no keyframe toggles yet (commands only), colours/points/text shown read-only.
- [ ] A run sends ~38k input tokens (every command as a tool): on lsuite AI that is ~20 credits a message before caching.
      Consider the trimmed tool set for lsuite AI, or caching the tool list across turns.
- [ ] frei0r: `defish0r` gives no answer in the scan child (listed as failed); three-input mixers are refused.

## Next session

- [ ] A pass with real mouse input in the running app: clicks, drags and typing are covered by GPUI
      UI tests and the app was driven through `kimchi-cli`, but nobody has used the window by hand yet.
- [ ] Windows builds were produced by CI but never launched. Linux: launched on Xvfb (2026-10-02),
      which found the missing `gpui` x11/wayland features; not yet on a real Linux desktop.
- [x] Codex as an agent provider: real turns run (2026-10-03, Codex 0.160), with deletes allowed by kimchi's own permissions.
- [ ] Each release: update `../lsuite/kimchi/index.html` (what changed, and screenshots saved as
      WebP in `../lsuite/assets/img/kimchi/`, dark and `-light`). On Linux take them with `vscreen`
      (`vscreen size 2000x1250`, `KIMCHI_WINDOW_SIZE=2000x1250`, `vscreen shot`); the steps are in
      `../lsuite/README.md`, "Updating an app's page". On the Mac, `kimchi-cli ui.screenshot`.
