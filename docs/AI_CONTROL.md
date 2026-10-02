# Controlling kimchi from AI and scripts

Everything a person can do in the kimchi window, an AI or a script can do too, through one
command registry. This page explains the four ways in, what an agent should call first, and how
to edit, generate and export. The full list of commands and parameters is the generated
[command reference](COMMANDS.md).

## Four ways in, one registry

| Way in | For | How it connects |
| --- | --- | --- |
| The window | people | buttons, menus, shortcuts and drags call the registry by name |
| The built-in agent | asking kimchi in your own words | the Agent panel, with the model you choose (Claude Code, Codex, an API key, a local model) |
| `kimchi-cli` | scripts, terminals, other programs | the running app, or a project file |
| `kimchi-mcp` | Claude Code, Codex, Cursor, Claude Desktop, any MCP client | the running app, or a project file |

Commands are named `family.verb` (`clip.addText`, `generate.submit`, `export.start`), take a
JSON object of parameters and answer with JSON. The window, the agent, the CLI and MCP are peers:
they all go through the same door, so validation, permissions and the undo history behave the
same whoever made the change. In the running app they share **one undo history**: every edit is
an ordinary undo step (a `project.batch` is one step for many edits), so anything an AI does can
be undone with ⌘Z or `history.undo`, and `history.list` says who made each step (`window`,
`agent`, `cli`, `mcp`).

## Start with the overview

`project.overview` returns the whole open project in one bounded answer: the canvas (size, frame
rate, aspect ratio, length) and where the project lives; every track from top to bottom with its
clips (times, media, text, and transforms that differ from the defaults); the media with how
each generated item was made (prompt, model, seed, inputs); markers; running generation jobs;
the last undo and redo steps; what the window shows; and **problems** worth noticing (media
missing on disk, placeholders still generating, hidden or muted tracks that hold clips).

```sh
kimchi-cli project.overview
```

Then drill down only where needed: `clip.get`, `media.get`, `track.list`, `timeline.markers`,
`generate.jobs`, `ui.state` for the window and `ui.screenshot` to see it.

## Conventions

- Times are **seconds on the timeline**. `start` is where a clip begins; `clip.trim` moves one
  edge to a timeline time, as dragging it would.
- Parameters are **camelCase** (`clipId`, `fadeIn`, `aspectRatio`); results use the project
  file's field names.
- **Names work wherever ids do**: `clipId`, `trackId`, `assetId`, `markerId` and `projectId`
  accept a unique name (`--clipId Title`, `--trackId "Video 1"`). A wrong or ambiguous name is
  refused with what exists and the closest match; so is an unknown command or parameter
  (`clip.addText` has no parameter `txt`. Did you mean `text`?).
- Track 0 is drawn on top. New video tracks go on top, new audio tracks at the bottom.
- Positions (`x`, `y`) are offsets of the centre from the canvas centre in project pixels
  (positive `y` is down); `scale` 1 fits the canvas.
- Commands that edit the project also accept `coalesce`: edits with the same key within about a
  second fold into one undo step (drags, sliders).
- Errors are written for the reader: they say what was expected and how to fix the call.

## The CLI

`kimchi-cli` runs every command. Without `--file` it talks to the running app over a local
bridge: the app listens on 127.0.0.1 only and writes its port and a random token to
`control.json` in its data folder (readable only by you; `KIMCHI_CONTROL` overrides the path),
and accepts only clients that present the token.

```sh
kimchi-cli commands                      # every command (add --json for the full schema)
kimchi-cli help generate.submit          # one command's parameters and permission
kimchi-cli doctor                        # ffmpeg, the running app, versions, folders, lsuite entry
kimchi-cli project.overview
kimchi-cli clip.addText --text "Hello" --start 0 --duration 3 --style '{"fontSize":160}'
kimchi-cli media.import --paths ~/clips/a.mp4 --paths ~/clips/b.mp4 --place
```

Parameters are `--name value`, `--name=value` or `name=value`. Values take the parameter's type:
strings stay strings, booleans may stand alone (`--wait`) or be `true`/`false`, arrays and
objects are JSON (a single value is a one-item array, and repeating an array parameter adds to
it). `--args '{"…": …}'` gives several parameters as one JSON object. Output is pretty JSON;
`--compact` prints one line. The exit status is 0 on success, 1 when a command fails (the message
goes to stderr) and 2 on a usage error.

### Working on a file

`--file project.json` works on a project file in this process, without the app: the file is
opened in place and every change is saved back to it atomically (written beside it, then
renamed). `project.create` makes the file when it doesn't exist yet.

```sh
kimchi-cli --file cut.json project.create name=Trailer width=1080 height=1920
kimchi-cli --file cut.json media.import --paths ~/clips/a.mp4 --place
kimchi-cli --file cut.json clip.addText text="Coming soon" start=0
kimchi-cli --file cut.json export.start path=cut.mp4
kimchi-cli render cut.json cut.webm --format webm   # the same, as one line
```

A project open in the window belongs to the window: `--file` on the same file is refused (the CLI
asks the running app which file it has open). Use the live commands instead. With `--file`,
exports and generations wait until they finish (`wait` is always true), and commands that need
the window (playback, selection, panels, screenshots) are refused. The undo history lives as long
as the process: use `batch` or MCP to undo across several commands on a file.

`--headless` works on the library (`<data folder>/projects`) in this process; it is refused while
the app runs. `--agent` marks requests as coming from an agent, so the agent permissions below
apply (on the bridge and on files alike).

### Batch

`kimchi-cli batch` reads one `{"command": "…", "params": {…}}` per line from standard input
(`method`/`name` and `arguments` work too; blank lines and `#` comments are skipped), runs them
on one connection or one file session, and prints one JSON result per line. It stops at the first
error unless `--continue`.

```sh
kimchi-cli --file cut.json batch <<'EOF'
{"command":"clip.addSolid","params":{"color":"#101010","start":0,"duration":4}}
{"command":"clip.addText","params":{"text":"Chapter one","start":0.5,"duration":3}}
{"command":"timeline.addMarker","params":{"time":4,"label":"Act 1"}}
EOF
```

To make many edits **one undo step**, use `project.batch` instead: with `atomic` (the default) a
failing command rolls back the ones before it.

```sh
kimchi-cli project.batch --commands '[
  {"command":"clip.update","params":{"clipId":"Intro","fadeOut":0.5}},
  {"command":"clip.trim","params":{"clipId":"Intro","edge":"end","time":6}},
  {"command":"timeline.addMarker","params":{"time":6,"label":"Cut"}}
]'
```

### Other tools

- `kimchi-cli docs [--out PATH]` writes this reference's companion, `docs/COMMANDS.md`, from the
  registry (`--out -` prints it). A test fails when the checked-in file differs; regenerate it
  with `cargo run -p kimchi-cli -- docs`.
- `kimchi-cli mcp-config` prints the MCP install lines below with this computer's paths.
- `kimchi-cli generate <provider> <model> "<prompt>" [--video] [--image PATH] [--end PATH]
  [--aspect 16:9] [--duration 5] [--seed N] [--count N] [--out DIR]` makes one image or video
  straight to files, without a project (handy for trying a model). Inside a project use
  `generate.submit`, which places the result on the timeline.
- `kimchi-cli providers` and `kimchi-cli models <provider>` are `generate.providers` and
  `generate.models provider=…`.

## MCP

`kimchi-mcp` is a stdio Model Context Protocol server. Each command is a tool, with the dot
replaced by an underscore (`clip.addText` is `clip_addText`); its description and JSON schema
come from the registry, with a note when it needs a permission or the window.

```sh
# Claude Code
claude mcp add kimchi -- /Applications/kimchi.app/Contents/MacOS/kimchi-mcp --live
# Codex CLI
codex mcp add kimchi -- /Applications/kimchi.app/Contents/MacOS/kimchi-mcp --live
```

```json
{"mcpServers": {"kimchi": {"command": "/Applications/kimchi.app/Contents/MacOS/kimchi-mcp", "args": ["--live"]}}}
```

The block above suits Cursor (`~/.cursor/mcp.json`), Claude Desktop
(`claude_desktop_config.json`) and most other clients. `kimchi-cli mcp-config` prints all three
with the real path of `kimchi-mcp` on this computer (and `KIMCHI_CONTROL` when you have set it).

- `--live` drives the running app. The server starts even while the app is closed; each call
  then answers with a hint until it is open.
- `--file <project.json>` hosts that project file in the server and saves after every change
  (`project_create` makes it). A file open in the app is refused.
- `--headless` hosts a session on the library. Without a flag, the server drives the app when it
  runs and otherwise hosts a session on the library.
- **Resources**: `kimchi://project/overview` (read it first), `kimchi://project` (the project
  file), `kimchi://commands`, `kimchi://settings` and `kimchi://app`.
- **Prompts**: `rough-cut` (assemble media into a cut), `title-and-captions`, `generate-b-roll`
  and `review-the-cut`.
- A failing tool answers with `isError: true` and the message, so the model can read it and try
  again. MCP requests are always held to the agent permissions.

### Undoing what a terminal agent did

Every edit an MCP client (or `kimchi-cli --agent`) makes is an ordinary undo step. On top of that,
the bridge takes a checkpoint before a connection's first change: the Agent panel's **Changes**
tab lists each such session ("From a terminal") with **Revert this session**, which puts the
project back as it was in one step (`history.revertTo`; one undo brings the session back). The
command records the panel shows also name the clips each command created.

## Permissions

Settings › Agent › Permissions decide what an agent may do: the built-in agent, every MCP client
and `kimchi-cli --agent`. Editing the open project is always allowed (and always undoable); these
are switches for the rest. The window and plain `kimchi-cli` are the person's own tools and are
not checked.

| Permission | Setting | Default | Covers |
| --- | --- | --- | --- |
| Agents | `agent.permissions.enabled` | on | the master switch: off refuses every agent and MCP request |
| Files | `agent.permissions.files` | on | `media.import`, `export.start`, `project.saveAs`, `handoff.toRyolune`, `handoff.fromRyolune` |
| Projects | `agent.permissions.projects` | on | `project.create`, `project.open`, `project.close`, `project.duplicate`, `project.delete` |
| Generate | `agent.permissions.generate` | on | `generate.submit`, `generate.animateFrame`, `generate.extendClip`, `generate.bridge`, `generate.restyleFrame`, `generate.regenerate` (they spend the provider's credits) |
| Settings | `agent.permissions.settings` | off | `app.setSetting`, `generate.setProvider` |
| App control | `agent.permissions.appControl` | off | `app.quit`, `app.installUpdate` |

`kimchi-cli help <command>` and the [command reference](COMMANDS.md) name the permission each
command needs. A refused command says which switch is off.

## Generation

Generation happens in the cut: a placeholder clip appears on the timeline straight away and
becomes the result when the job is done (`place: "library"` only adds it to the media). Every
generated item keeps its prompt, model, seed and inputs, shown by `media.get` and
`project.overview`. Providers and their models: `generate.providers` (which are ready: enabled,
and with a key when they need one) and `generate.models` (tasks, aspect ratios, durations,
frames, sound). Without `provider`/`model`, a command uses `settings.generate.imageModel` or
`videoModel`, else the first featured ready model.

Pass `wait: true` to get the finished job back; otherwise the command returns the job at once and
`generate.wait`, `generate.jobs` and `generate.cancel` follow it. With `--file`, commands always
wait.

```sh
# An image or a video from words
kimchi-cli generate.submit --prompt "slow pan over a misty harbour at dawn" --video --duration 5 --wait
kimchi-cli generate.submit --prompt "poster frame, bold type" --aspectRatio 9:16 --place library
# Image-to-video from any frame: a path, a media item, or the frame a clip shows at a time
kimchi-cli generate.submit --prompt "the camera pushes in" --video \
  --images '[{"role":"start_frame","clipId":"Harbour","time":2.5}]'
```

| Command | What it does |
| --- | --- |
| `generate.animateFrame` | turns the frame a clip shows at `time` into a moving shot (image-to-video), placed right after the clip |
| `generate.extendClip` | continues a clip from its last frame; the new shot lands right after it on the same track |
| `generate.bridge` | a transition from the last frame of `fromClipId` to the first frame of `toClipId`, filling the gap between them |
| `generate.restyleFrame` | edits the frame a clip shows at `time` with an image model; the still lands at that time |
| `generate.regenerate` | runs a generated clip's request again (same prompt, model and settings); `variation` uses a new seed, `prompt` changes the words; the result lands after the clip |

```sh
kimchi-cli generate.animateFrame --clipId Harbour --time 2 --prompt "gulls take off" --wait
kimchi-cli generate.bridge --fromClipId "Shot 1" --toClipId "Shot 2" --prompt "match cut through fog"
kimchi-cli generate.regenerate --clipId "Shot 3" --variation
```

Keys: each provider's key lives in the OS keychain, saved by the person in the app's settings
(`generate.setKey`), or comes from the usual environment variable (`FAL_KEY`,
`OPENROUTER_API_KEY`, `RUNWAYML_API_SECRET`, `LUMAAI_API_KEY`, `GEMINI_API_KEY`,
`STABILITY_API_KEY`, `BFL_API_KEY`, `TOGETHER_API_KEY`…). Local providers (ComfyUI, for one)
need no key; `generate.setProvider` points them at another address.

## Export

`export.start` renders the open project through one ffmpeg graph; text is drawn exactly as in the
preview. Formats are `mp4` (default), `hevc`, `prores`, `webm`, `gif`, `audio` (AAC) and `wav`; qualities
`draft`, `standard` (default) and `high` (`export.formats` lists them with their extensions).
`width`, `height` and `fps` override the project's; `from` and `to` export a range.

```sh
kimchi-cli export.start --path ~/Movies/cut.mp4 --quality high --wait
kimchi-cli export.start --path ~/Movies/teaser.gif --format gif --from 10 --to 16 --width 640
kimchi-cli export.status            # progress of every export; export.cancel stops one
```

## Hand-offs with ryolune

kimchi and [ryolune](https://lsuite.xyz/ryolune) (music) are both lsuite apps. Each writes
`~/.lsuite/apps/<app>.json` when it starts (`LSUITE_HOME` overrides `~/.lsuite`), so they find
each other and agents find both.

- `handoff.apps` lists the lsuite apps installed on this computer and whether they are running.
- `handoff.toRyolune` sends the cut to ryolune to score it: it renders the audio (24-bit WAV) into
  `~/.lsuite/handoff/ryolune/` with `<name>.kimchi-cut.json` beside it (length, range, fps, markers);
  when ryolune is running, it imports the audio at bar 0 through ryolune's own bridge and adds the
  markers, placed at the song's starting tempo. `from`, `to` and `name` narrow it.
- `handoff.fromRyolune` puts ryolune's music on an audio track: a file ryolune exported (`path`),
  or, when ryolune is running, a fresh bounce of its open song.

```sh
kimchi-cli handoff.toRyolune
kimchi-cli handoff.fromRyolune --start 0
```

## What only a person does

A few things deliberately stay with the person: API keys (`generate.setKey` and
`app.setAgentKey` are refused for agents), the agent's own settings and permissions (`app.setSetting` refuses `agent.*` keys from
an agent), and the choice of which model runs the built-in agent. Agents never see keys:
`generate.providers` reports only where a key comes from and its last four characters.

## Environment

| Variable | Meaning |
| --- | --- |
| `KIMCHI_CONTROL` | the bridge's control file (default `<data folder>/control.json`) |
| `KIMCHI_DATA_DIR` | the data folder: library, generated media, caches |
| `KIMCHI_CONFIG_DIR` | the settings folder (`settings.json`, `providers.json`) |
| `KIMCHI_FFMPEG`, `KIMCHI_FFPROBE` | ffmpeg and ffprobe to use instead of the bundled or installed ones |
| `KIMCHI_NO_UPDATE=1` | never check for updates |
| `KIMCHI_KEYCHAIN` | `1` reads API keys from the OS keychain, `0` only from environment variables (default: on in release builds, off in debug builds) |
| `RYOLUNE_CONTROL` | ryolune's control file, for the hand-offs (default `~/.ryolune/control.json`) |
| `LSUITE_HOME` | where lsuite apps register (default `~/.lsuite`) |

## Limits

Playback, selection, panels, notifications and screenshots need the running window
(`--file` and `--headless` refuse them with a hint). In file and headless mode the undo history
lasts as long as the process. One request at a time per MCP server; a long `wait` holds the next
call until the job finishes, so prefer `generate.jobs` and `generate.wait` for long generations.
