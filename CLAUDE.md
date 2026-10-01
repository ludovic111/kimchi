# kimchi

Desktop video editor with generation in the cut. Rust workspace (`crates/kimchi-core` project
model and edits, `kimchi-media` ffmpeg, `kimchi-gen` providers, `kimchi-cli`), Tauri shell in
`src-tauri/`, Svelte 5 UI in `ui/` (thin: every edit is applied in Rust). See README.md.

## lsuite: bring kimchi up to the suite standard (next session, decided 2026-10-01)

kimchi is part of **lsuite** (lowercase), the free open-source creative suite with ryolune
(music) and zenith (hub); its page is lsuite.xyz/kimchi (`../lsuite/kimchi/index.html`). The
contract is `STANDARD.md` in ludovic111/lsuite (locally `../lsuite/STANDARD.md`). The owner wants
every lsuite app **100 % drivable by MCP, CLI and its built-in agent**, with automatic updates, and
compatible with the others. ryolune (`../ryolune`) is the reference: read its `CLAUDE.md`,
`engine/src/control.rs` and `docs/AI_CONTROL.md` before designing. What kimchi is missing:

- [ ] **Command registry**: one named, validated registry (`project.*`, `clip.*`, `track.*`,
      `timeline.*`, `generate.*`, `export.*`, `history.*`, `app.*`, `ui.*`) over the existing
      `Edit` data in kimchi-core, with one undo history shared by every client. The UI calls it;
      no Tauri command that a script could want stays private.
- [ ] **CLI**: grow `kimchi-cli` (or a `kimchi-cli` binary) from `generate`/`render` to every
      command, on a file (`--file project.json`) or on the running app.
- [ ] **MCP**: a `kimchi-mcp` binary generated from the registry, `--live` through a local
      token-protected bridge to the running app, one-line install for Claude Code / Codex.
      Ship `docs/AI_CONTROL.md` and a generated `docs/COMMANDS.md`.
- [ ] **Built-in agent**: today only generation; add an agent panel that runs registry commands
      (providers: Claude Code, Codex, API keys, local), one card per command, changes with revert,
      permissions enforced for agent and MCP alike.
- [ ] **Auto-update**: the Tauri updater is there; make "check for updates" a command, add
      `KIMCHI_NO_UPDATE=1` and a setting, and **notarize the macOS build**.
- [ ] **Discovery and hand-offs**: write `~/.lsuite/apps/kimchi.json` (format in STANDARD.md, or
      as ryolune defines it), accept audio from ryolune onto an audio track, and send a cut's
      audio/length/markers to ryolune to score.
- [ ] Support links to `https://lsuite.xyz/kimchi/support`; keep the lsuite page up to date with
      every release (version, screenshots in `../lsuite/assets/img/kimchi/`, downloads).

When done, tick these, and update the status table at the end of `../lsuite/STANDARD.md`.
