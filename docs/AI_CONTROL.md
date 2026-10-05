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

## The built-in agent from a script

The Agent panel's conversation is one, whoever drives it: the panel, `kimchi-cli` or an MCP
client. `agent.send` asks the agent in words (with the model chosen in Settings › Agent, or with
`agent.setProvider`), `agent.status` follows a run (what it is doing, its reply, every command it
ran), `agent.stop` stops it, `agent.revert` puts the project back as it was before the run (one
undo step), and `agent.newConversation` starts afresh. A request sent from a terminal shows in the
panel like one typed there. One run at a time; these commands need the running app.

```sh
kimchi-cli agent.send --prompt "Add a title saying Hello at 0 s and fade it in" --wait
kimchi-cli agent.status                     # the latest run, with its commands
kimchi-cli agent.revert                     # undo the whole run
```

## The window's shortcuts by name

Every keyboard shortcut and menu item of the window can also be run by name with `ui.action`
(`PlayPause`, `CopyClips`, `PasteClips`, `TrimStart`, `NextEdit`, `ToggleSnap`…): it acts on the
window's selection, playhead and clipboard as the key would. The window's own options have
commands too: `ui.setTimeline` (snapping, ripple delete, loop), `ui.setLayout` (panel sizes),
`ui.showPanel` (open, or close with `open` false), `timeline.play --speed` (the J/L shuttle) and
`ui.reveal` (show a file in the file manager). `ui.state` reports all of it.

```sh
kimchi-cli ui.select --clipIds Title && kimchi-cli ui.action --action CopyClips
kimchi-cli timeline.seek --time 12 && kimchi-cli ui.action --action PasteClips
```

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
| Generate | `agent.permissions.generate` | on | `generate.submit`, `generate.animateFrame`, `generate.extendClip`, `generate.bridge`, `generate.restyleFrame`, `generate.regenerate` (they spend the provider's credits), and `agent.send` (it uses the person's model account) |
| Settings | `agent.permissions.settings` | off | `app.setSetting`, `generate.setProvider` |
| App control | `agent.permissions.appControl` | off | `app.quit`, `app.restart`, `app.installUpdate` |

`ui.action` runs a shortcut as the window would, so an agent needs the permission of what the
shortcut does: `NewProject` and `CloseProject` need projects, `Import` and `Export` files,
`ToggleTheme` settings, `Quit` and `RestartApp` app control.

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

## Animation, motion graphics and 3D

kimchi draws animation itself, the same in the preview and the export, and every part of it is a
command: what an agent makes, a person can open in the inspector and change, and the other way round.
`motion.guide` is the full reference (scene formats, every property, easings, reveals); read it before
writing a scene.

- **Any clip**: `clip.setKeyframes {clipId, property, keyframes}` (x, y, position, scale, scaleX, scaleY,
  rotation, opacity, blur, volume; text: fontSize, color, letterSpacing), `clip.addKeyframe` /
  `clip.removeKeyframe` at a time, or a ready-made `clip.animate {clipIds, preset}` (`motion.presets`).
- **Templates**: `motion.templates` lists them with their values; `motion.addTemplate` adds one,
  `motion.setTemplate` changes its values later.
- **Your own scene**: `motion.add {scene}` with a 2D scene (layers) or a 3D one (camera, lights, objects);
  `motion.get`, `motion.update`, `motion.setLayer`, `motion.removeLayer`, `motion.updateLayer` (some
  properties of one layer: animated ones get a keyframe at that time) and `motion.setKeyframes` /
  `motion.addKeyframe` / `motion.removeKeyframe` for one property.
- **Look at it**: `project.renderFrame {times: [...]}` returns a PNG (several times give one labelled
  sheet). Check entrances, overlaps and legibility, then fix what you see.

```sh
kimchi-cli motion.addTemplate --template lowerThird --values '{"title": "Grace Hopper", "subtitle": "Rear admiral"}' --start 4
kimchi-cli clip.animate --clipIds '["Logo"]' --preset popIn
kimchi-cli motion.add --start 10 --scene '{"type": "3d", "objects": [{"id": "logo", "type": "text", "text": "KIMCHI",
  "material": {"color": "#ff5a36"}, "keyframes": {"rotation.y": [[0, -40], [1.5, 0, "easeOutBack"]]}}]}'
kimchi-cli project.renderFrame --times '[10.2, 10.8, 11.5]'
```

3D draws on the GPU when there is one (Metal on Macs, Apple Silicon included; Vulkan or DirectX 12
elsewhere) and on the CPU otherwise; `app.info` says which (`renderer3d`). `KIMCHI_GPU=0` forces the CPU.

## Colour, transitions, speed and captions

- **Colour**: `clip.setEffects {clipIds, look?, brightness, contrast, saturation, temperature, tint,
  vignette, sharpen, chromaKey, lut, lutStrength, reset}`; only the fields given change. Corrections run
  -1 to 1 (vignette and sharpen 0 to 1) and take keyframes like any clip property (`clip.setKeyframes
  {property: "saturation"}`). `clip.looks` lists the looks. `chromaKey` takes `true`, the screen's colour
  or `{color, similarity, softness, spill}`; `lut` the path of a 3D `.cube` file.
- **Transitions**: a clip's transition is how it comes in at its start. `transition.set {clipIds | trackId,
  kind, duration, easing}` (`trackId` puts one on every cut of a track); on a cut it is centred on the cut
  and both clips play on past it, so nothing moves; the sound crossfades. `transition.kinds`,
  `transition.list` (where each plays, and whether the clips made it shorter), `transition.remove`.
- **Speed and time**: `clip.update {speed}` (0.1–16, the sound keeps its pitch), `clip.update {reverse: true}`
  plays the same part of the media backwards, `clip.freezeFrame {clipId, time, duration}` holds a frame and
  pushes the rest of the track later.
- **Captions** are titles on the captions track. `captions.transcribe` listens to the mix (or `clipId`'s
  sound) with Whisper on this computer and captions it (`language`, `model`: tiny / base / small;
  the model downloads once, `captions.status` follows it). `captions.import` / `captions.export` read and
  write SRT or WebVTT, `captions.add`, `captions.setStyle` (all at once), `captions.list`, `captions.clear`.
  Edit one caption like any title (`clip.update {style: {content}}`, `clip.trim`).

```sh
kimchi-cli clip.setEffects --clipIds '["Interview"]' --look warm --contrast 0.2
kimchi-cli transition.set --trackId "Video 1" --kind dissolve --duration 0.6
kimchi-cli captions.transcribe --language en
kimchi-cli export.start --path ~/Movies/cut.mp4 --captions both --wait
```

## Sound: the mix, effects and ryolune

Every sample of the preview and the export comes from kimchi's mixer. Read the mix first:
`audio.overview` gives every track's fader, pan, mute, solo, routing, sends, ducking and effects
(with their parameters as ryolune shows them: `"-6.0 dB"`, `"Hall"`), the buses, the master, clips
whose sound was changed, ryolune songs (and whether they were saved since), the beats found in music,
and problems (a solo left on, an effect missing on this computer, a bus nothing feeds).

- **Tracks and clips**: `audio.setTrack {trackId, gainDb, pan, muted, solo, output, duck, armed}`
  (`output` is `"master"` or a bus; `duck` is `true`, `false` or `{under, amountDb, thresholdDb, attack,
  release}`); `audio.setClip {clipIds, gainDb | volume, pan, fadeIn, fadeOut, fadeCurve, channels, pitch,
  preservePitch, muted}`. Fade curves are `linear`, `equalPower`, `exponential`, `sCurve`; channels
  `stereo`, `mono`, `left`, `right`, `swap`. A clip's level and pan over time are clip keyframes
  (`clip.setKeyframes {property: "volume" | "pan"}`).
- **Buses and the master**: `audio.addBus {name, effect}` (a shared reverb: `effect: "Space"`),
  `audio.setBus`, `audio.removeBus`, `audio.setSend {trackId, busId, levelDb, preFader}`,
  `audio.removeSend`; `audio.setMaster {gainDb, limiter, ceilingDb, loudness}` where `loudness` is LUFS,
  `"youtube"` (−14), `"podcast"` (−16), `"broadcast"` (−23) or `null`: exports are brought to it.
- **Effects** are ryolune's: its stock effects (Channel EQ, ryolune Comp, Space, Echo, De-Esser, Limiter…)
  and the CLAP, VST3, Audio Unit and ryolune plugins on the computer, in ryolune's own insert format, so a
  chain moves between the two apps unchanged. `audio.effects {query}` lists them, `audio.effectParams
  {effect | target, slot}` their parameters. A `target` is a track, a bus or a clip (id or name; prefix
  `track:`, `bus:` or `clip:` when names clash) or `"master"`; a `slot` is a slot id, the effect's name or
  its position from 1. `audio.addEffect {target, effect, params, index}`, `audio.setEffect {target, slot,
  params, bypassed, reset}` (values as numbers in the parameter's unit or as text: `"-6 dB"`, `"2.5k"`,
  `"Hall"`), `audio.moveEffect`, `audio.removeEffect`, `audio.copyEffects {from, to}`,
  `audio.effectPresets` / `audio.applyPreset`, `audio.rescanPlugins`.
- **Automation** on tracks, buses and the master: `audio.setAutomation {target, property, keyframes}`,
  `audio.addAutomationKey`, `audio.removeAutomationKey`; `property` is `gainDb`, `pan` or
  `"<effect>.<parameter>"` (`"Space.Mix"`). Times are timeline seconds. An automated parameter set with
  `audio.setEffect` gets a keyframe at the playhead.
- **Analysis and quick fixes**: `audio.measure {clipId | trackId | from, to}` (EBU R128: integrated LUFS,
  range, true peak), `audio.normalize {clipIds, target, mode}` (speech: −16 LUFS by default),
  `audio.detectBeats {assetId | clipId}` (stored with the media; the timeline draws the beats and can
  snap to them, `app.setSetting audio.snapToBeats`), `audio.beatCut {musicClipId, every, trackId, from,
  to, markers}` (cut the picture on the music), `audio.autoDuck {music, dialogue, amountDb}` (the
  music tracks go down while the dialogue speaks; guessed from names and content when not given).
- **ryolune songs**: `audio.importSong {path, as: "mix" | "stems", markers}` (or `media.import` of a
  `.ryolune` file) renders the song with ryolune's own engine onto the timeline, with its beats;
  `audio.refreshSongs` renders it again once it was saved in ryolune (the window does it by itself when a
  project opens and when it comes back to the front); `audio.openInRyolune {clipId}` opens the song there.
  `handoff.toRyolune {as: "session"}` sends the whole audio timeline to ryolune as a multitrack session
  (one track per kimchi track, with its clips, fades, gains, effects, fader and pan).
- **The window** (needs the running app): `audio.meters` (levels playing now), `audio.devices`,
  `audio.record {action: start | stop | cancel | status}` (a voice-over take on the armed track at the
  playhead, after a count-in), `audio.showMixer {open, layout: "replace" | "beside", target, slot}`;
  `ui.state` reports the mixer as `audio`. `audio.scrub {on}` and the `audio.*` settings
  (`outputDevice`, `inputDevice`, `defaultLoudness`, `pluginFolders`, `snapToBeats`, `countIn`,
  `refreshSongs`) are the person's preferences.

```sh
kimchi-cli audio.overview
kimchi-cli audio.addEffect --target "Voice" --effect "De-Esser"
kimchi-cli audio.addEffect --target "Voice" --effect "Channel EQ" --params '{"Low Gain": "-4 dB", "High Gain": "+3 dB"}'
kimchi-cli audio.addBus --name Reverb --effect Space
kimchi-cli audio.setSend --trackId Voice --busId Reverb --levelDb -14
kimchi-cli audio.autoDuck
kimchi-cli audio.normalize --clipIds '["interview.mov"]'
kimchi-cli audio.setAutomation --target Music --property gainDb --keyframes '[[0, -30], [2, 0, "easeOut"]]'
kimchi-cli audio.setMaster --loudness youtube
kimchi-cli audio.importSong --path ~/Music/theme.ryolune --as stems --markers true
kimchi-cli audio.beatCut --musicClipId theme --every 4
```

## Export

`export.start` renders the open project: the compositor draws every frame (exactly as in the
preview) and ffmpeg encodes them with the mixed sound. Formats are `mp4` (default), `hevc`, `prores`, `webm`, `gif`, `audio` (AAC) and `wav`; qualities
`draft`, `standard` (default) and `high` (`export.formats` lists them with their extensions).
`width`, `height` and `fps` override the project's; `from` and `to` export a range. With captions,
`captions` is `burn` (default, in the picture), `file` (an `.srt` beside the video instead), `both` or `none`.
The sound: `audioFormat` (`wav`, `aiff`, `flac`, `mp3`, `aac`, `opus`, `vorbis`) for sound-only exports,
`sampleRate` (44100, 48000, 96000), `bitDepth` (16, 24, or 32-bit float WAV), `bitrate` (lossy kbit/s),
`stems` (one file per track and bus into the folder `path` names; `stemsMaster` runs them through the
master) and `loudness` (LUFS or `youtube` / `podcast` / `broadcast`; by default the master's target).

```sh
kimchi-cli export.start --path ~/Movies/cut.mp4 --quality high --wait
kimchi-cli export.start --path ~/Movies/teaser.gif --format gif --from 10 --to 16 --width 640
kimchi-cli export.start --path ~/Music/cut.flac --format audio --audioFormat flac --sampleRate 96000
kimchi-cli export.start --path ~/Music/stems --format audio --audioFormat wav --stems true
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
an agent), and the choice of which model runs the built-in agent (`agent.setProvider`). The built-in
agent doesn't see the `agent.*` commands: it can't drive itself. Agents never see keys:
`generate.providers` reports only where a key comes from and its last four characters.

## Environment

| Variable | Meaning |
| --- | --- |
| `KIMCHI_CONTROL` | the bridge's control file (default `<data folder>/control.json`) |
| `KIMCHI_DATA_DIR` | the data folder: library, generated media, caches |
| `KIMCHI_CONFIG_DIR` | the settings folder (`settings.json`, `providers.json`) |
| `KIMCHI_FFMPEG`, `KIMCHI_FFPROBE` | ffmpeg and ffprobe to use instead of the bundled or installed ones |
| `KIMCHI_NO_UPDATE=1` | never check for updates |
| `KIMCHI_WINDOW_SIZE` | the window's size when it opens, e.g. `2000x1250` (screenshots) |
| `KIMCHI_GPU` | `0` draws 3D on the CPU; `any` accepts a software GPU adapter (default: a hardware GPU when there is one) |
| `KIMCHI_KEYCHAIN` | `1` reads API keys from the OS keychain, `0` only from environment variables (default: on in release builds, off in debug builds) |
| `RYOLUNE_CONTROL` | ryolune's control file, for the hand-offs (default `~/.ryolune/control.json`) |
| `LSUITE_HOME` | where lsuite apps register (default `~/.lsuite`) |

## Limits

Playback, selection, panels, notifications, screenshots and the built-in agent (`agent.*`) need
the running window (`--file` and `--headless` refuse them with a hint). In file and headless mode the undo history
lasts as long as the process. One request at a time per MCP server; a long `wait` holds the next
call until the job finishes, so prefer `generate.jobs` and `generate.wait` for long generations.
