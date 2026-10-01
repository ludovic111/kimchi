# kimchi

Desktop video editor with generation in the cut. Rust workspace (`crates/kimchi-core` project
model and edits, `kimchi-media` ffmpeg, `kimchi-gen` providers, `kimchi-cli`), Tauri shell in
`src-tauri/`, Svelte 5 UI in `ui/` (thin: every edit is applied in Rust). See README.md.

## lsuite: bring kimchi up to the suite standard (next session; notes updated 2026-10-01)

kimchi is part of **lsuite** (lowercase), the free open-source creative suite with ryolune
(music) and zenith (hub); its page is lsuite.xyz/kimchi (`../lsuite/kimchi/index.html`). Two
documents in ludovic111/lsuite (locally `../lsuite/`) are the contract: `STANDARD.md` and
`design/DESIGN.md` (shared design system, live at lsuite.xyz/design). The owner wants every lsuite
app **100 % drivable by MCP, CLI and its built-in agent**, with automatic updates, compatible with
the others, and with one common look. ryolune (`../ryolune`) is the reference: read its
`CLAUDE.md`, `engine/src/control.rs` and `docs/AI_CONTROL.md` before designing.

Done: 0.1.1 (2026-10-01) is the first release signed with the Developer ID and notarized on macOS
(`scripts/prepare-apple-signing.sh` in the release workflow; secrets set with
`../lsuite/scripts/set-apple-secrets.sh`, API key "kimchi notarization"). Updates are signed
(Tauri updater, `latest.json`).

Still to do:

- [ ] **Command registry**: one named, validated registry (`project.*`, `clip.*`, `track.*`,
      `timeline.*`, `generate.*`, `export.*`, `history.*`, `app.*`, `ui.*`) over the existing
      `Edit` data in kimchi-core, with one undo history shared by every client. The UI calls it;
      no Tauri command that a script could want stays private.
- [ ] **CLI**: grow `kimchi-cli` from `generate`/`render` to every command, on a file
      (`--file project.json`) or on the running app.
- [ ] **MCP**: a `kimchi-mcp` binary generated from the registry, `--live` through a local
      token-protected bridge to the running app, one-line install for Claude Code / Codex.
      Ship `docs/AI_CONTROL.md` and a generated `docs/COMMANDS.md`.
- [ ] **Built-in agent**: today only generation; add an agent panel that runs registry commands
      (providers: Claude Code, Codex, API keys, local), one card per command, changes with revert,
      permissions enforced for agent and MCP alike.
- [ ] **Auto-update**: make "check for updates" a command, add `KIMCHI_NO_UPDATE=1` and a setting.
- [ ] **Design system** (`../lsuite/design/`): kimchi's signature color is **chili coral, hue 32**
      (`--ls-kimchi-*`, accent `#f7806a` dark / `#c3513d` light), matching its icon. Replace the
      Svelte UI's own colors with `tokens.css` (`data-app="kimchi"`), move to Manrope + IBM Plex Mono
      (from Instrument Sans/Serif and Geist Mono), put the chrome (top bar, generate panel,
      inspector, timeline toolbar, menus, ⌘K palette, dialogs) on the glass tiers over
      `.ls-backdrop`, keep the preview canvas and the timeline solid, use macOS window vibrancy
      (Tauri `window-vibrancy`), ship dark **and light**, add a contrast test, and redraw the icon
      from the lsuite template.
- [ ] **Discovery and hand-offs**: write `~/.lsuite/apps/kimchi.json` (format in STANDARD.md, or
      as ryolune defines it), accept audio from ryolune onto an audio track, and send a cut's
      audio/length/markers to ryolune to score.
- [ ] Support links to `https://lsuite.xyz/kimchi/support`; keep the lsuite page up to date with
      every release (screenshots in `../lsuite/assets/img/kimchi/`; the version shown comes from
      the latest GitHub release).

When done, tick these, and update the status table at the end of `../lsuite/STANDARD.md`.
