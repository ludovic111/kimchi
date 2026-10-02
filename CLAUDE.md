# kimchi

Desktop video editor with generation in the cut. A native Rust app: the window is **GPUI** (direct
upstream, pinned to Zed commit `7733b99…`, `runtime_shaders` on macOS so no Metal toolchain is needed),
and every action goes through one command registry. See README.md.

```
crates/kimchi-core      project model, edits, undo history (labels, sources, batches, checkpoints),
                        keyframes/easings (anim.rs), motion scenes 2D+3D (motion.rs), presets, templates
crates/kimchi-media     ffmpeg probe/decode/encode; the compositor (render/: clips, flat.rs 2D motion,
                        space/ 3D on wgpu or the CPU rasteriser), export, preview, text rendering
crates/kimchi-gen       generation providers and job queue
crates/kimchi-control   registry (commands/mod.rs lists every spec), session, permissions, bridge,
                        lsuite discovery, hand-offs with ryolune, updater
crates/kimchi-agent     the built-in agent (Claude Code, Codex, Anthropic, OpenAI, Ollama)
crates/kimchi-desktop   the window (package/binary `kimchi`): store.rs, app.rs, views/, ui/, theme.rs
crates/kimchi-cli       `kimchi-cli`;  crates/kimchi-mcp: `kimchi-mcp`;  crates/kimchi-release: signing
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
- GPUI API: grep the pinned source in `~/.cargo/git/checkouts/zed-*/7733b99/crates/gpui`; the
  `build-gpui-apps` skill (installed in `.claude/skills/`) covers this revision. Views keep retained
  state (text fields, scrubs, subscriptions) in their entity; drags use `ui::drag::track`; icons
  are `ui::icon` (inherits the text colour); panels in the editor are cached views.
- Design system: `crates/kimchi-desktop/assets/tokens.json` is a copy of `../lsuite/design/tokens.json`
  (re-copy when it changes). GPUI has no backdrop blur: tier 1 is translucent over the window's own
  backdrop (native blur behind on macOS), tiers 2–3 are their tint over the raised surface.
  `theme.rs` has the contrast test.
- Testing the app: `source` an env that sets `KIMCHI_DATA_DIR`, `KIMCHI_CONFIG_DIR`, `LSUITE_HOME`,
  `KIMCHI_NO_UPDATE=1` to scratch folders, run `target/debug/kimchi`, drive it with
  `target/debug/kimchi-cli …`, look with `kimchi-cli ui.screenshot path=…`. Debug builds don't read
  the keychain (`KIMCHI_KEYCHAIN=1` forces it; an unsigned build makes macOS ask for the login
  password). On Linux without a desktop: `Xvfb :77` + `openbox`, `DISPLAY=:77`, screenshots with
  `import -window <id>` (ImageMagick; `ui.screenshot` is macOS-only) and clicks with `xdotool`. UI tests run the real views headless (`crates/kimchi-desktop/src/tests.rs`,
  `views/timeline/tests.rs`).

## lsuite (notes updated 2026-10-02)

kimchi is part of **lsuite** with ryolune (music) and zenith (code); its page is lsuite.xyz/kimchi
(`../lsuite/kimchi/index.html`). Contract: `../lsuite/STANDARD.md` and `../lsuite/design/DESIGN.md`.

- [x] **Command registry**: 103 `family.verb` commands (project, media, track, clip, motion,
      timeline, history, generate, export, handoff, app, ui), one undo history for every client,
      batches as one step, `project.overview`, names or ids everywhere.
- [x] **CLI**: `kimchi-cli <command>` on the running app or `--file project.json`; `batch`, `doctor`,
      `mcp-config`, `docs`.
- [x] **MCP**: `kimchi-mcp --live | --file | --headless`, tools generated from the registry; docs in
      `docs/AI_CONTROL.md` and generated `docs/COMMANDS.md`.
- [x] **Built-in agent**: Agent panel (⌘J), Claude Code / Codex / Anthropic / OpenAI / Ollama, one card
      per command, changes with "Revert this run", permissions (`settings.agent.permissions`) checked in
      `registry::call` for agent and MCP alike. Codex untested (not installed on the dev Mac).
- [x] **Auto-update**: `app.checkUpdates` / `app.installUpdate`, `KIMCHI_NO_UPDATE=1`, setting
      `updates.checkOnStart`; signed with the Tauri-era minisign key, `latest.json` in the Tauri
      format so 0.1.x installs update to this app.
- [x] **Design system**: tokens, chili coral, Manrope + IBM Plex Mono (bundled), glass tiers over the
      backdrop, solid work surfaces, macOS window blur, dark and light, contrast test, icon redrawn
      from the template.
- [x] **Discovery and hand-offs**: `~/.lsuite/apps/kimchi.json` (format 1, defined here, see
      `kimchi-control/src/discovery.rs`), `handoff.toRyolune` / `handoff.fromRyolune` through
      ryolune's bridge.
- [x] Support links go to `https://lsuite.xyz/kimchi/support`.

## Animation, motion graphics and 3D (2026-10-02, not released yet)

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

## Next session

kimchi 0.4.0 (the GPUI app) is released (2026-10-02): notarized macOS for Apple Silicon and Intel,
Windows and Linux; 0.1.x installs update to it. The lsuite page and STANDARD.md's kimchi column are
up to date.

- [ ] A pass with real mouse input in the running app: clicks, drags and typing are covered by GPUI
      UI tests and the app was driven through `kimchi-cli`, but nobody has used the window by hand yet.
- [ ] Windows builds were produced by CI but never launched. Linux: launched on Xvfb (2026-10-02),
      which found the missing `gpui` x11/wayland features; not yet on a real Linux desktop.
- [ ] Codex as an agent provider: run one real turn once Codex is installed.
- [ ] Each release: update `../lsuite/kimchi/index.html` (what changed, screenshots with
      `KIMCHI_WINDOW_SIZE=2000x1250` and `kimchi-cli ui.screenshot`, saved as WebP in
      `../lsuite/assets/img/kimchi/`, dark and `-light`).
