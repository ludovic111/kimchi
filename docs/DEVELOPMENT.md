# Developing kimchi

Building, running and testing kimchi from source, and how to make the common kinds of change.
Read [ARCHITECTURE.md](ARCHITECTURE.md) first for how the pieces fit.

> kimchi isn't taking pull requests at the moment, except for critical bug fixes that were
> discussed in an issue first (see the
> [pull request template](../.github/pull_request_template.md)). This guide is for anyone who wants
> to build, study or fork it.

## Setting up

You need a recent stable Rust (CI uses the current stable; on Linux the dependencies need at least
1.92) and ffmpeg.

**macOS**: `brew install ffmpeg`. GPUI compiles its Metal shaders at run time, so the Xcode
command-line tools are enough.

**Linux** (Debian and Ubuntu; the same packages CI installs):

```sh
sudo apt install pkg-config clang libasound2-dev libdbus-1-dev libfontconfig-dev libfreetype-dev \
  libssl-dev libvulkan-dev libwayland-dev libx11-xcb-dev libxcb1-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libzstd-dev libglib2.0-dev
```

For other distributions, see `script/linux` in the [Zed
repository](https://github.com/zed-industries/zed). Older distributions' ffmpeg can be too old for
the media tests (Ubuntu 22.04's 4.4 is); fetch the static build the app ships instead:

```sh
scripts/fetch-ffmpeg.sh                     # into target/ffmpeg/<this machine's target>/
export KIMCHI_FFMPEG=$PWD/target/ffmpeg/x86_64-unknown-linux-gnu/kimchi-ffmpeg     # on x86-64
export KIMCHI_FFPROBE=$PWD/target/ffmpeg/x86_64-unknown-linux-gnu/kimchi-ffprobe
```

**Windows**: build in Git Bash; the packaging script expects it, along with NSIS (`makensis`) and
`7z`.

Dependencies are compiled with optimisations even in debug builds (and `kimchi-audio` too, for
real-time mixing), so the first build takes a while and later ones are quick.

## Running

```sh
cargo run -p kimchi                 # the app
cargo run -p kimchi-cli -- help     # the CLI
cargo run -p kimchi-mcp -- --help   # the MCP server
```

**Use a scratch environment** so development runs don't touch your own projects, settings or
keychain. Put this in a file and `source` it:

```sh
export KIMCHI_DATA_DIR=/tmp/kimchi-dev/data
export KIMCHI_CONFIG_DIR=/tmp/kimchi-dev/config
export LSUITE_HOME=/tmp/kimchi-dev/lsuite
export KIMCHI_NO_UPDATE=1
```

Debug builds keep API keys in memory instead of the keychain; `KIMCHI_KEYCHAIN=1` uses the keychain
(an unsigned build makes macOS ask for your login password). Keys in the providers' environment
variables work either way.

**Driving the running app**: with the app open in the same environment, `target/debug/kimchi-cli`
reaches it over the bridge:

```sh
target/debug/kimchi-cli doctor
target/debug/kimchi-cli project.create --name Test
target/debug/kimchi-cli ui.action --action ToggleMixer
target/debug/kimchi-cli ui.screenshot --path /tmp/shot.png     # macOS only
```

**Linux without a desktop**: run an X server and a window manager, then the app on it:

```sh
Xvfb :77 -screen 0 1920x1200x24 & DISPLAY=:77 openbox &
DISPLAY=:77 KIMCHI_WINDOW_SIZE=1600x1000 target/debug/kimchi
```

`ui.screenshot` is macOS-only; use `import -window <id>` (ImageMagick) for screenshots and `xdotool`
for clicks. If captures come back black (it depends on the graphics stack), run the app under a
headless Wayland compositor such as sway instead. On llvmpipe (no GPU), set `KIMCHI_GPU=any` to
exercise the GPU 3D path anyway.

## Testing

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

CI runs exactly these two on Ubuntu 22.04 and macOS, and `bash -n` on every script. There is no
`rustfmt` check; `crates/kimchi-media` has its own `rustfmt.toml` (120 columns).

| Where | What |
| --- | --- |
| `kimchi-core/src/tests.rs`, `mesh/tests.rs`, `expr/tests.rs`, `motion/eval/tests.rs` | The model, edits and history; meshes; expressions; motion evaluation |
| `kimchi-control/src/tests.rs` | Commands end to end on a headless `Session` in a temporary folder: undo, batches, permissions, file mode, the bridge, revert, motion, export, captions, audio |
| `kimchi-audio/src/mixer/tests.rs`, `song/tests.rs` | The mixer and ryolune songs |
| `kimchi-media/src/**/tests.rs`, `render/flat_tests.rs`, `tests/*.rs` | Rendering (pixels, regions, GPU against CPU), titles, export, preview, the render cache. Tests that need ffmpeg skip without it. |
| `kimchi-gen/tests/*.rs` | Each provider against a mock server (wiremock), and the harness with a fake provider |
| `kimchi-cli/tests/cli.rs`, `kimchi-mcp/tests/mcp.rs` | The real binaries, in temporary folders |
| `kimchi-agent/src/tests.rs` | The agent against mock APIs |
| `kimchi-captions/tests/speech.rs` | Whisper on real speech (ignored without a recording) |
| `kimchi-desktop/src/tests.rs`, `views/timeline/tests.rs`, `views/studio/tests.rs`, `views/mixer/tests.rs` | The real views, headless (`#[gpui::test]`): clicks, drags, keys |

Most crates also have unit tests next to the code. Render tests compare pixels, regional averages,
or the GPU against the CPU; there are no stored reference images. `KIMCHI_DUMP=<folder>` writes the
images out to look at.

**Ignored tests** need something CI doesn't have; run them with `-- --ignored`:

- each cloud provider's live test, with its API key (`cargo test -p kimchi-gen --test openai --
  --ignored`);
- local providers (ComfyUI, the SD WebUI, Ollama, an OpenAI-compatible server) running;
- Whisper on real speech (`KIMCHI_SPEECH_WAV` and `KIMCHI_WHISPER_DIR`);
- the agent with a real Claude Code;
- benchmarks and image dumps.

**Tests that guard documentation and invariants:**

| Test | Fails when |
| --- | --- |
| `commands_md_matches_the_registry` (`kimchi-cli`) | `docs/COMMANDS.md` doesn't match the registry. Run `cargo run -p kimchi-cli -- docs`. |
| `the_changelog_has_this_version` (`kimchi-control`) | The first section of `CHANGELOG.md` isn't the workspace version. |
| `every_command_has_a_handler`, `names_are_unique_and_well_formed` (`kimchi-control`) | A spec has no handler, a name is repeated or isn't `family.verb`, or a description is empty. |
| `window_actions_are_named_and_checked` (`kimchi-control`) | An action `ui.action` accepts isn't listed in its description. |
| `text_contrast_holds_on_every_surface` (`kimchi-desktop`) | A theme colour falls below 4.5:1 for text (3:1 for the accent). |
| `gpu_matches_cpu` (`kimchi-media`) | The GPU and CPU 3D shading differ. Without a GPU it passes without checking anything; use `KIMCHI_GPU=any` on llvmpipe. |

**Agent evals** (`evals/`) score the built-in agent on 12 scripted video jobs (title card, rough
cut, lower third, vertical edit, grade, dissolves, loudness, music ending, chapters, 3D title, Ken
Burns, trim to length). Each builds a project from fixtures made with ffmpeg, runs
`kimchi-cli --file cut.json ask "…" --json` with a real model, and checks the project it left (its
structure, loudness, rendered frames, and whether the agent looked at its work before finishing):

```sh
cargo build -p kimchi-cli -p kimchi-mcp
evals/run.py title-card rough-cut              # some jobs, through Claude Code (no key needed)
evals/run.py --record                          # all of them, added to evals/RESULTS.md
evals/run.py --provider anthropic --model claude-sonnet-5-5   # with ANTHROPIC_API_KEY
```

Add a job when you add a skill (`kimchi-control/src/harness/skills/`).

**UI tests** run the real views without a screen. Window commands such as `timeline.seek` must be
started and then pumped (the `remote` helper in `kimchi-desktop/src/tests.rs`): calling them through a
blocking fixture call deadlocks, since they wait for the window that the test is blocking.

## Common changes

### A new command

Every feature starts here.

1. **The spec**: add it to `SPECS` in `kimchi-control/src/commands/mod.rs`, in its family's section:
   `query` to read, `edit` to change the project; `.perm(…)` if agents need a permission;
   `.window()` if only the window can do it. Write the description for a reader who has only it: say
   what it does, what it returns, and the defaults.
2. **The handler**: add a match arm in the family's file (`commands/<family>.rs`). Resolve ids or
   names with `resolve::…`, change the project only through `Session::apply` with an `Edit` (add a
   variant in `kimchi-core/src/edit.rs` if none fits), and return JSON. Errors are sentences that say
   what was expected and how to fix the call. For example, from `commands/timeline.rs`:

   ```rust
   "timeline.addMarker" => {
       let time = a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead);
       let label = a.opt_str("label").unwrap_or("").to_string();
       s.apply(cx.label(), cx.source, &Edit::AddMarker { time, label: label.clone() }, None)?;
       …
   }
   ```

3. **The window**: call it with `Store::run` (or `run_then` to act on the result) from a button, menu
   or action. Commands only the window can carry out are handled in `Workspace::ui_command`
   (`app.rs`).
4. **Docs**: `cargo run -p kimchi-cli -- docs`, from the repository's root, regenerates
   `docs/COMMANDS.md`. Mention notable
   commands in [AI_CONTROL.md](AI_CONTROL.md) and, if people use it in the window, in the
   [user guide](guide/README.md).
5. **Tests**: a test in `kimchi-control/src/tests.rs` that calls the command on a headless session,
   and a UI test if the window does something new.

The MCP tool, the agent's tool and the CLI's help come from the spec; nothing else to register.

### A keyboard shortcut or menu item

Add the action to the `actions!` list and a row to `SHORTCUTS` in `kimchi-desktop/src/actions.rs`
(group, label, keys, scope), then handle it in `Workspace`. `M-` in a key means ⌘ on macOS and Ctrl
elsewhere. The row binds the key, shows it in the shortcut sheet, and gives the key hint to menus
and the palette; a new group also goes in `GROUPS`. Menu items (`actions::menus`, macOS only) and
palette entries (`views/dialogs/palette.rs`) are separate lists. For scripts to run it with
`ui.action`, add its name to `ACTIONS` in `kimchi-control/src/commands/ui.rs` and to the `ui.action`
description, and regenerate `docs/COMMANDS.md`. In tooltips and menus, name keys with `actions::tip`
and `hint`, never a hard-coded ⌘.

### A setting

Add the field with its default to the right struct in `kimchi-control/src/settings.rs` (they are
`#[serde(default)]`, so old files keep loading), and its allowed values or range in `choices()` or
`range()`. Then add the control to the Settings dialog
(`kimchi-desktop/src/views/dialogs/settings.rs` and its `settings/` pages) and a row to
[CONFIGURATION.md](CONFIGURATION.md#settings).

### A field in the project

Add it with `#[serde(default)]`, and `skip_serializing_if` its default, so older files open and
projects that don't use the feature are written exactly as before. There is no format version; this
is how compatibility is kept. Update [PROJECT_FORMAT.md](PROJECT_FORMAT.md).

### Motion and 3D

- Motion scenes are JSON that agents write. `Scene::from_json` checks unknown fields, types, ids,
  property names and colours, with "did you mean" errors; keep those checks complete. Enum struct
  variants need their own `#[serde(rename_all = "camelCase")]`.
- `kimchi-control/src/commands/motion_guide.md` is what `motion.guide` returns: keep it in step with
  `kimchi-core/src/motion/`. The expressions section comes from `kimchi-core/src/expr`'s `GUIDE`.
- 3D shading exists twice, in `space::shade` (CPU) and `space/gpu.wgsl` (GPU): change both, and run
  `KIMCHI_GPU=any cargo test -p kimchi-media --lib gpu_matches_cpu`.

### A generation provider

Implement `Provider` (`info`, `models`, `check`, `generate`) in a new module of
`kimchi-gen/src/providers/`, add it to `providers::all()`, give it a logo (`LOGOS` in
`kimchi-desktop/src/ui/logos.rs`, the image in `assets/logos/` and its source in
`assets/logos/SOURCES.md`; tests check this), and test it against a wiremock server in
`kimchi-gen/tests/`, with an ignored live test. Declare each model's capabilities (aspect ratios,
lengths, resolutions, inputs) so the Generate panel shows only what applies. Long jobs should poll
with `util::poll` (which retries network errors, 429 and 5xx responses). Cancelling drops the
future; use `cx.cancel` only to tell the remote service to stop.

### Window code

- The pinned GPUI is in `~/.cargo/git/checkouts/zed-*/7733b99/crates/gpui`; grep it for the API.
- Views keep retained state (text fields, scrubs, subscriptions) in their entity.
- Drags use `ui::drag::track`; icons are `ui::icon` (they take the text colour); things that appear
  animate in with `ui::motion`, which is skipped under reduced motion.
- `crates/kimchi-desktop/assets/tokens.json` is a copy of lsuite's design tokens; re-copy it when
  they change.

### Logging

Log with `tracing`, never `println!`: in `kimchi-mcp`, standard output is the MCP protocol.

## Measuring performance

The `render_bench` example measures compositing, effects, 3D, path tracing, export, playback and
seeking. [PERFORMANCE.md](PERFORMANCE.md) has the method and the latest results.

## Packaging

The bundle scripts write to `target/dist/`:

```sh
scripts/bundle-macos.sh aarch64-apple-darwin   # kimchi.app, .dmg, .app.tar.gz (ad-hoc signed without a Developer ID)
scripts/bundle-linux.sh                        # .AppImage and .deb
scripts/bundle-windows.sh                      # setup .exe (NSIS) and portable .zip, in Git Bash
```

The app bundles ffmpeg (fetched by `scripts/fetch-ffmpeg.sh`), `kimchi-cli` and `kimchi-mcp`.
`KIMCHI_SKIP_BUILD=1` packages what is already built. The Linux script downloads appimagetool (or
uses `APPIMAGETOOL`) and needs `dpkg-deb`.

`scripts/make-icons.sh` redraws the icons from `brand/icon.svg` into the repository
(`brand/icon.png` and `crates/kimchi-desktop/resources/kimchi.{icns,ico,png}`), to be committed. It
needs resvg or rsvg-convert and python3, and makes the `.icns` only on a Mac. Packaging resources
(the Info.plist template, entitlements, icons, the `.desktop` file, the NSIS script) are in
`crates/kimchi-desktop/resources/`.

## Releases

1. Bump `version` in `[workspace.package]` in `Cargo.toml`, run `cargo check` so `Cargo.lock` follows
   (CI builds with `--locked`), and commit both.
2. Add the version's section at the top of `CHANGELOG.md`. Write it for people, in plain words: the
   app shows it in What's new after updating, and it begins the release notes. A test checks that it
   is there.
3. Run the agent evals (`evals/run.py --record`, below) and commit `evals/RESULTS.md`. A harness
   change that lowers the pass rate doesn't ship.
4. Tag `vX.Y.Z` and push the tag.
5. When the workflow is done, `scripts/publish-build.sh X.Y.Z` (needs `gh` with access to both
   repositories).

The release workflow checks that the tag matches the version, builds Linux (macOS, Apple Silicon and
Intel, signed and notarized when the Apple secrets are set, and Windows are paused for now: their lines
are commented out in the workflow's matrix),
signs the update files, writes `latest.json` and `SHA256SUMS`, and makes a **draft** GitHub release.
Never publish the draft: kimchi's builds come only through the lsuite app and lsuite.xyz, with no
account (lsuite `DISTRIBUTION.md`). `scripts/publish-build.sh` checks the draft's files
against `SHA256SUMS`, copies them to the private `ludovic111/lsuite-builds` as `kimchi-vX.Y.Z`, then
deletes the draft. That rolls the update out: installed copies read
`<server>/api/apps/kimchi/latest.json` (no token), and download through the server.

Updates are signed with minisign, with the key of the original Tauri builds, through
`kimchi-release`:

```sh
cargo run -p kimchi-release -- sign <file> --version X.Y.Z     # key in TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)
cargo run -p kimchi-release -- verify <file>                   # against kimchi's update key
cargo run -p kimchi-release -- manifest target/dist --version X.Y.Z --base-url <release download URL>
cargo run -p kimchi-release -- keygen /tmp/test --password pw  # a throwaway key pair for testing
```

The workflow's secrets: `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (the
update key), and `APPLE_CERTIFICATE_P12_BASE64`, `APPLE_CERTIFICATE_PASSWORD`,
`APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY_P8_BASE64`, `APPLE_API_KEY_ID` and `APPLE_API_ISSUER`
(Developer ID signing and notarization, shared with the other lsuite apps).

After a release, update kimchi's page on lsuite.xyz (`../lsuite/kimchi/index.html`) with what
changed and new screenshots; `../lsuite/README.md` has the steps.
