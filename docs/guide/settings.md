# Settings and troubleshooting

Open Settings with ⌘, (Ctrl+,). The pages on the left are kimchi's own (Appearance, Audio, Agent,
Updates, Diagnostics, About & AI control), then one page per model provider, cloud first and then
those on your machine. A dot beside each provider shows whether it is ready.

Every setting is saved as soon as you change it (API keys when you press **Save**).
[Configuration](../CONFIGURATION.md#settings) lists them all with their names, for `app.setSetting`.

| Page | What's on it |
| --- | --- |
| Appearance | Mode (System, Dark, Light) and Transparency. See [The window](window.md#light-and-dark). |
| Audio | Output and input devices, the Normalize target, scrubbing, snap to beats, the voice-over count-in, plugins, and ryolune. See [Sound](sound.md). |
| Agent | Who runs the agent (lsuite AI first: sign in, your plan and allowance, Manage plan, Sign out), its model, address and key, and the permissions. See [The agent](agent.md). |
| Updates | Check now, and how updates happen. See below. |
| Diagnostics | Crash reports, this run's log, how much detail goes in the log, and reporting a problem. See below. |
| About & AI control | Links, and the lines that connect Claude Code, Codex, Cursor, Claude Desktop or VS Code to kimchi. |
| Providers | One page per generation provider. See [Generation](generation.md#connecting-a-model). |

## Updates

kimchi checks lsuite for a new version when it starts and every 6 hours, with your free lsuite
account (sign in from the lsuite app, lsuite.xyz/launcher; one sign-in covers every lsuite app).
Signed out, Settings › Updates says to sign in instead. When a new version is out, a button in the
top bar offers it. Every update is signed, and the signature is checked before anything is
replaced.

| Setting | Default | |
| --- | --- | --- |
| Check for updates automatically | On | Off, kimchi only checks when you click **Check now**. |
| Download and install updates by themselves | Off | Installs a found update without asking; it runs from the next start. Needs the automatic check. |
| Show what's new after an update | On | The release notes, once, after updating. |

How an update installs depends on how kimchi was installed:

| Install | Update |
| --- | --- |
| macOS app | Replaced in place; the previous copy is kept until the new one has started. |
| AppImage | The same. |
| Windows installer | The verified installer runs when kimchi restarts or quits. |
| `.deb` or the Windows portable `.zip` | kimchi says an update is out and links to the lsuite app, which gets it with your account. |

**What's new** (in the **…** menu, the command palette, or Help on macOS) shows this version's notes;
**Earlier versions** shows the rest.

## Logs and crash reports

kimchi keeps a log of each run and writes a report when something goes wrong. Nothing is sent
anywhere unless you send it.

**Settings › Diagnostics** shows them:

- **Crash reports**: each crash (with what the program was doing) and each run that ended without
  quitting properly (when the computer shut down or the app was killed). **View** one to copy or
  reveal it; **Delete all** clears them.
- **This run's log**: open the logs folder, or copy the latest lines.
- **Detail in the log**: Normal, **Detailed** (the default) or Everything. If the `RUST_LOG`
  environment variable is set, it decides instead.
- **Report a problem**: copy the system details, or open a GitHub issue with the version and system
  filled in (also **Report a problem** in the command palette, or Help on macOS).

When the last run crashed or didn't quit properly, a notice at the next start offers to show the
report.

The logs are in the `logs/` folder of kimchi's data folder: `kimchi.log` for this run and
`kimchi.1.log` to `kimchi.3.log` for the three before; a log is also rotated when it reaches
16 MB. Reports are in `logs/crashes/`; the latest 25 are kept. See
[Configuration](../CONFIGURATION.md#files-and-folders) for where the data folder is.

## Troubleshooting

**"ffmpeg wasn't found."** The released apps include ffmpeg. A build from source uses the `ffmpeg`
on your `PATH`, or the files named by `KIMCHI_FFMPEG` and `KIMCHI_FFPROBE`.
`kimchi-cli doctor` shows which ffmpeg is in use.

**A clip shows as missing.** Imported media is used where it is on disk, not copied. If a file was
moved or deleted, `project.overview` lists it under problems; put the file back where it was.

**A model isn't in the picker.** Check its provider's page in Settings: it must be **Enabled** and
**Ready** (a key saved, or the local server answering). **Test connection** says what is wrong.
Click the refresh button in the model picker to reload model lists, which are otherwise kept for
30 minutes.

**The agent says it isn't ready.** Settings › Agent shows the reason for each choice: for lsuite AI,
sign in (or choose a plan, or wait for next month's allowance: Manage plan); for Claude
Code and Codex, the command must be installed and signed in (`claude auth status`,
`codex login status`); for the APIs, a key; for Ollama, a running server with a model.

**The agent was refused a command.** It needs a permission that is off; see
[Permissions](agent.md#permissions).

**3D is slow.** The standard engine uses the graphics card when there is one. kimchi ignores
software graphics drivers (such as llvmpipe on Linux), which would be slower than its own CPU
renderer. Path-traced scenes are always slow live: render them ahead (see
[Rendering ahead](motion.md#rendering-ahead)).

**No sound.** Check Settings › Audio › Output, and that the tracks aren't muted and no other track
is soloed. The mixer's meters move only while playing.

**A plugin is missing.** Click **Rescan plugins**, and see [Effects](sound.md#effects) for the
folders kimchi looks in.

**Exports fail with the GPU encoder.** With **Encoder: Auto**, kimchi already retries on the
processor. Choose **CPU** to skip the hardware, or set `KIMCHI_HARDWARE=0`.

**Something else.** Copy the system details and the log lines from Settings › Diagnostics into an
[issue](https://github.com/ludovic111/kimchi/issues).
