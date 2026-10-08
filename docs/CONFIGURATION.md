# Configuration

Every setting, environment variable and file kimchi uses, and where it keeps them. For the
settings as they appear in the window, see [Settings and troubleshooting](guide/settings.md).

## Settings

Settings live in `settings.json` in the [settings folder](#files-and-folders). The Settings dialog
writes it, and so do `app.setSetting` and `kimchi-cli app.setSetting`:

```sh
kimchi-cli app.settings                                           # all of them, as JSON
kimchi-cli app.setSetting --key audio.countIn --value 2
kimchi-cli app.setSetting --key appearance.mode --value light
```

A new value must have the same type as the default, and some settings accept only certain values
or a range; anything else is refused with the reason. Agents and MCP clients may change settings
only with the **Settings** permission, and never the `agent.*` ones.

| Setting | Type | Default | Values | Meaning |
| --- | --- | --- | --- | --- |
| `appearance.mode` | string | `system` | `system`, `dark`, `light` | Light or dark. `system` follows the computer. |
| `appearance.transparency` | boolean | `true` | | Glass panels and menus; `false` makes every surface opaque. |
| `audio.outputDevice` | string | `""` | a device name | Where the preview plays. Empty is the system default. |
| `audio.inputDevice` | string | `""` | a device name | The microphone for voice-overs. Empty is the system default. |
| `audio.defaultLoudness` | number | `-16` | −40 to −5 LUFS | The target of **Normalize**. |
| `audio.scrub` | boolean | `true` | | Hear the sound while moving the playhead. |
| `audio.snapToBeats` | boolean | `false` | | Snap to detected music beats on the timeline. |
| `audio.countIn` | number | `3` | 0 to 10 seconds | The voice-over count-in. |
| `audio.refreshSongs` | boolean | `true` | | Render ryolune songs again when kimchi comes to the front after they changed. |
| `audio.pluginFolders` | array of strings | `[]` | folder paths | More folders the plugin scan (**Rescan plugins**, `audio.rescanPlugins`) searches, for every format, besides the standard ones. |
| `generate.imageModel` | string | `""` | `provider::model` | The image model used when a command names none. The Generate panel sets it. |
| `generate.videoModel` | string | `""` | `provider::model` | The same, for video. |
| `updates.checkOnStart` | boolean | `true` | | Check for updates at start and every 6 hours. |
| `updates.autoInstall` | boolean | `false` | | Download and install updates found by the automatic check without asking; the new version runs from the next start. |
| `updates.showWhatsNew` | boolean | `true` | | Show the release notes once after an update. |
| `diagnostics.logLevel` | string | `debug` | `info`, `debug`, `trace` | How much goes in the log (Normal, Detailed, Everything). `RUST_LOG` overrides it. |
| `agent.provider` | string | `lsuite` | `lsuite`, `zenith`, `claude-code`, `codex`, `gemini-cli`, `anthropic`, `openai`, `gemini`, `openrouter`, `groq`, `mistral`, `deepseek`, `xai`, `together`, `fireworks`, `cerebras`, `azure-openai`, `bedrock`, `ollama`, `lmstudio`, `openai-compatible` | Who runs the built-in agent. `lsuite` is lsuite AI (the lsuite account); settings from before 0.10 keep the one they name. |
| `agent.model` | string | `""` | a model id | Empty uses the provider's default. |
| `agent.baseUrl` | string | `""` | a URL | For the Anthropic, OpenAI and Ollama choices: another server or a proxy. Empty uses the provider's. |
| `agent.permissions.enabled` | boolean | `true` | | Let agents and MCP clients act in kimchi at all. |
| `agent.permissions.files` | boolean | `true` | | Import, export, write files, save a copy of the project. |
| `agent.permissions.projects` | boolean | `true` | | Create, open, close, duplicate or delete projects. |
| `agent.permissions.generate` | boolean | `true` | | Generate images and video. |
| `agent.permissions.settings` | boolean | `false` | | Change settings (other than `agent.*`). |
| `agent.permissions.appControl` | boolean | `false` | | Quit kimchi, install an update. |
| `agent.permissions.plugins` | boolean | `false` | | Write, build, install, remove and switch plugins. Turned on by sending a request from Plugins › Build with your agent. |
| `agent.claudeCodeOnLsuite` | boolean | `false` | | Claude Code runs on lsuite AI (the lsuite account's plan, through `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`) instead of its own sign-in, when signed in to lsuite. |
| `plugins.videoFolders` | array of strings | `[]` | folder paths | More folders to look for video plugins in (lsuite bundles and frei0r), besides the standard ones. |
| `plugins.disabled` | array of strings | `[]` | plugin ids | Plugins switched off (Plugins' switches, `plugin.disable`), and plugins that crashed and were switched off. |

If `settings.json` can't be parsed, kimchi renames it `settings.json.bad` and starts with the
defaults. Missing keys take their defaults; unknown keys are ignored.

### Other files in the settings folder

| File | Holds |
| --- | --- |
| `providers.json` | Each generation provider's switch, server address and options (`enabled`, `base_url`, `options`), as set in Settings › Models & keys or with `generate.setProvider`. |
| `window-layout.json` | Panel sizes and which panels are open. |
| `last-version` | The version that ran last, to know when to show What's new. |
| `reports-seen` | When crash reports were last looked at. |

### API keys

Keys are never written to these files. kimchi stores them in the system's keychain (the macOS
Keychain, the Windows Credential Manager, or the Secret Service on Linux) under the service
`app.kimchi.editor`, one entry per provider id. When the keychain has no key for a provider, kimchi
reads it from the provider's environment variable (see [below](#provider-keys)).

The OpenAI image provider and the agent's OpenAI choice share one keychain entry, `openai`.

## Environment variables

### For everyone

| Variable | Read by | Meaning |
| --- | --- | --- |
| `KIMCHI_DATA_DIR` | all | The data folder: library, logs, models, recordings, bridge file. |
| `KIMCHI_CONFIG_DIR` | all | The settings folder. |
| `KIMCHI_CONTROL` | all | The bridge file (default `<data folder>/control.json`). |
| `KIMCHI_FFMPEG`, `KIMCHI_FFPROBE` | all | The ffmpeg and ffprobe to use. Otherwise kimchi looks next to its own program, then on `PATH`, then in the usual install folders. |
| `KIMCHI_GPU` | all | `0` (or `off`, `false`, `cpu`) draws 3D on the processor. `any` also accepts a software graphics adapter such as llvmpipe. Default: the graphics card when there is a hardware one. |
| `KIMCHI_HARDWARE` | all | `0` (or `off`, `false`, `no`) never uses hardware video encoders. |
| `KIMCHI_KEYCHAIN` | all | `1` (or `on`, `true`, `yes`) uses the system keychain; any other value keeps keys entered in the window in memory until quit. Default: on in release builds, off in debug builds. Keys from environment variables work either way. |
| `KIMCHI_NO_UPDATE` | app | Any non-empty value other than `0` stops the automatic update checks. **Check now** still works. |
| `KIMCHI_NO_WHATS_NEW` | app | Any value stops What's new from opening after an update. |
| `KIMCHI_NO_SYSTEM_FONTS` | all | Any value: text in the picture (titles, captions, motion text) uses only the fonts bundled with kimchi. |
| `KIMCHI_WINDOW_SIZE` | app | The window's size when it opens, as `WIDTHxHEIGHT` (for example `2000x1250`). |
| `KIMCHI_UPDATE_URL` | app | Another `latest.json` to check for updates (tests). By default kimchi asks the lsuite server, `<server>/api/apps/kimchi/latest.json`, with the signed-in lsuite account's token; signed out, the check says to sign in to lsuite in the lsuite app. The token is only sent to the lsuite server. |
| `KIMCHI_MCP_CONTEXT` | `kimchi-mcp` | `0` stops tool results from ending with an updated `<context>` block when the project or the window changed, and edits' results from reminding the agent to look at its work. |
| `KIMCHI_MCP` | app | The `kimchi-mcp` program given to Claude Code and Codex, if the file exists. Otherwise kimchi looks next to itself, then on `PATH`. |
| `LSUITE_HOME` | all | Where lsuite apps register, hand files over, keep the shared lsuite account and lsuite plugins (default `~/.lsuite`). |
| `LSUITE_ACCOUNT_SERVER` | all | The lsuite account server for lsuite AI and updates (default: the signed-in account's, else `https://lsuite.xyz`), for a local demo server such as `http://127.0.0.1:4321`. |
| `KIMCHI_NO_BROWSER` | all | Any value other than `0`: signing in to lsuite AI doesn't open a browser (scripts). |
| `KIMCHI_PLUGIN_PATH` | all | More folders (a list, separated like `PATH`) of lsuite plugin bundles. |
| `KIMCHI_PLUGIN_SDK` | all | A local `kimchi-plugin` folder that `plugin.new` points new plugin crates at, instead of the repository's tag. |
| `FREI0R_PATH` | all | The folders to look for frei0r plugins in (a list); replaces the usual ones. |
| `RYOLUNE_CONTROL` | all | ryolune's bridge file, for the hand-offs. Default: `~/.ryolune/control.json`, then the file named in ryolune's lsuite registration. |
| `RYOLUNE_DATA_DIR` | app | ryolune's data folder, which holds the plugin list kimchi shares with it. |
| `CODEX_HOME` | app | Where Codex keeps its settings and sign-in (default `~/.codex`); see `agent-codex-home/` below. |
| `RUST_LOG` | app | Log filter in the `tracing` syntax; overrides `diagnostics.logLevel`. |
| `CLAP_PATH`, `VST3_PATH`, `RYOLUNE_PLUGIN_PATH` | app | Extra folders (a list, separated like `PATH`) to look for CLAP, VST3 and ryolune native plugins in. Read by ryolune's plugin scan, which also uses the extra folders in ryolune's own settings. |

"all" is the app, `kimchi-cli` and `kimchi-mcp`.

### Provider keys

Read when the keychain has no usable key for the provider. Where two are listed, the first that is
set wins.

| Provider | Variables |
| --- | --- |
| OpenRouter | `OPENROUTER_API_KEY` |
| fal | `FAL_KEY`, `FAL_API_KEY` |
| Replicate | `REPLICATE_API_TOKEN` |
| OpenAI (images, and the agent) | `OPENAI_API_KEY` |
| Google Gemini | `GEMINI_API_KEY`, `GOOGLE_API_KEY` |
| xAI | `XAI_API_KEY` |
| Runway | `RUNWAYML_API_SECRET`, `RUNWAY_API_KEY` |
| Luma AI | `LUMAAI_API_KEY`, `LUMA_API_KEY` |
| Black Forest Labs | `BFL_API_KEY` |
| Stability AI | `STABILITY_API_KEY` |
| Together AI | `TOGETHER_API_KEY` |
| Anthropic (the agent) | `ANTHROPIC_API_KEY` |

### For development and tests

| Variable | Meaning |
| --- | --- |
| `KIMCHI_UPDATE_PUBKEY` | In debug builds, the minisign public key updates are checked against. Also read by `kimchi-release verify` and `manifest`. |
| `KIMCHI_MCP_BUILTIN_AGENT` | Set by kimchi for the `kimchi-mcp` it gives Claude Code and Codex: hides the `agent_*` tools. |
| `KIMCHI_DUMP` | A folder where render tests write their images. |
| `KIMCHI_TEXT_SAMPLES` | A folder for the text rasteriser's sample images (an ignored test). |
| `KIMCHI_SPEECH_WAV`, `KIMCHI_WHISPER_DIR` | A speech recording, and a folder for Whisper models (`whisper-base/` is downloaded into it if missing), for the speech tests. |
| `KIMCHI_BENCH_MEDIA`, `KIMCHI_BENCH_PTRACE` | Inputs of the `render_bench` example (see [PERFORMANCE.md](PERFORMANCE.md)). |

Release builds and signing use more; see [DEVELOPMENT.md](DEVELOPMENT.md#releases).

## Files and folders

kimchi has two folders: the **data folder** and the **settings folder**. On macOS and Windows they
are the same folder.

| System | Data folder | Settings folder |
| --- | --- | --- |
| macOS | `~/Library/Application Support/kimchi` | the same |
| Windows | `%APPDATA%\kimchi` | the same |
| Linux | `$XDG_DATA_HOME/kimchi`, or `~/.local/share/kimchi` | `$XDG_CONFIG_HOME/kimchi`, or `~/.config/kimchi` |

`KIMCHI_DATA_DIR` and `KIMCHI_CONFIG_DIR` move them. Pointing both at empty folders (and
`LSUITE_HOME` too) gives a clean, separate kimchi, which is how to test without touching your
own projects.

### The data folder

```
projects/<project id>/      one folder per project in the library
  project.json              the project (see PROJECT_FORMAT.md)
  generated/                generated media; generated/songs/ holds rendered ryolune songs
  cache/                    thumbnails, filmstrips, waveforms, proxies, grabbed frames
  cache/rendered/           motion clips rendered ahead
  cache/renders/            frames rendered by project.renderFrame and motion views
logs/                       kimchi.log, kimchi.1.log … kimchi.3.log; running-<pid>.json while the app runs
logs/crashes/               crash-*.txt and unclean-*.txt reports (the latest 25)
models/whisper-<size>/      speech models for captions (tiny, base, small)
plugins/video.json          the video plugins a scan found (lsuite plugins, frei0r), by file
plugins/loaded/             the copies of lsuite plugin libraries the app loads (one per build: hot reload)
recordings/                 voice-over takes
thumbs/                     the Motion tab's template previews
agent-workspace/            the working folder of Claude Code and Codex when they run as the agent
agent-codex-home/           Codex's settings when it runs as the agent, sharing your Codex sign-in (only when you are signed in)
control.json                the bridge file, while the app runs
```

Imported media isn't copied into the library: projects refer to files where they are. A project
opened from a file elsewhere (`project.open --path`, `kimchi-cli --file`) is saved back to that
file.

### Elsewhere

| Path | What |
| --- | --- |
| `~/Documents/kimchi/comfyui-workflows/` | ComfyUI workflows that become models (changeable in Settings). |
| `~/.lsuite/apps/kimchi.json` | kimchi's registration with lsuite, so other apps and agents find it. See below. |
| `~/.lsuite/account.json` | The lsuite account (lsuite AI), shared by every lsuite app: `{format: 1, server, email, name, plan, token, signedInAt}`, readable by you only (0600). The token is a secret: kimchi never logs it or shows it (only `lsk_…` and its last four characters). Signing out removes the file. |
| `~/.lsuite/plugins/kimchi/<id>/` | Installed lsuite plugins (bundles: `plugin.toml` and the library). |
| `~/.lsuite/plugins-src/kimchi/<name>/` | Plugin crates made by `plugin.new` (and the agent); `.target/` is their shared build folder. |
| `~/.lsuite/handoff/ryolune/`, `~/.lsuite/handoff/kimchi/` | Files handed to and from ryolune. |
| ryolune's data folder, `plugins.json` | The list of audio plugins found, shared with ryolune. |

## The lsuite registration

When it starts, kimchi writes `~/.lsuite/apps/kimchi.json` (under `$LSUITE_HOME` if set), and
updates it when it quits:

```json
{
  "format": 1,
  "app": "kimchi",
  "version": "0.8.0",
  "kind": "video",
  "appPath": "/Applications/kimchi.app",
  "executable": "/Applications/kimchi.app/Contents/MacOS/kimchi",
  "cli": "/Applications/kimchi.app/Contents/MacOS/kimchi-cli",
  "mcp": "/Applications/kimchi.app/Contents/MacOS/kimchi-mcp",
  "dataDir": "/Users/you/Library/Application Support/kimchi",
  "documents": { "extensions": ["json"], "description": "kimchi project (JSON)" },
  "running": { "pid": 4242, "controlFile": "…/control.json", "port": 50123, "since": "2026-10-05T09:00:00Z" },
  "updatedAt": "2026-10-05T09:00:00Z"
}
```

`appPath` is set only for the macOS app; `cli` and `mcp` are `null` when those programs aren't
installed beside kimchi. `running` is `null` when kimchi isn't running; on macOS and Linux, readers
also treat an entry whose process has gone as not running.

## The bridge

While the app runs, it listens on `127.0.0.1` only, on a port chosen at start, and writes the port
and a random token to `control.json` (readable only by you on macOS and Linux). `kimchi-cli` and
`kimchi-mcp --live` read that file and present the token; connections without it are refused. The
protocol is JSON-RPC 2.0, one message per line: an `auth` call with the token first, then any
command by name. See [AI_CONTROL.md](AI_CONTROL.md#the-cli).
