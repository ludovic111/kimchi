# Changelog

What changed in each kimchi release. The app shows the newest section in **What's new** after it
updates, and the release workflow puts the section in the update's notes.

## Unreleased

lsuite is now entirely free: no account, no plans.

### Better
- **Updates need no account.** kimchi gets its updates from lsuite without signing in to anything, and Settings › Updates no longer asks you to sign in. Updates are still signed and checked exactly as before.

### Removed
- **lsuite AI and the lsuite account.** The agent runs on the model you already have: Claude Code, Codex, Gemini CLI, an API key (Anthropic, OpenAI, Gemini and more) or a local Ollama model. If lsuite AI was your choice, the agent uses Claude Code until you pick another provider; your conversations stay. The `account.*` commands and the "Run Claude Code on lsuite AI" switch are gone, and kimchi no longer reads `~/.lsuite/account.json` (it leaves the file alone).
- **The zenith agent provider.** zenith is discontinued, so the Agent panel no longer offers it. If it was your choice, the agent uses Claude Code until you pick another provider; your conversations stay.

## 0.11.0 — 2026-10-08

kimchi is in beta on **Linux** while lsuite is in beta. **macOS and Windows are coming soon.**

### New
- **An agent that knows video.** Every agent working in kimchi (the Agent panel's, the lsuite app's, or Claude Code and Codex connected to kimchi) now starts from the same expert brief: how to cut and pace, J and L cuts, when to dissolve, how to match colour, how big titles must be and where they stay readable, how loud a video should be for YouTube, social or podcasts, how to animate with intent, how to light a 3D shot, and which export suits each platform.
- **Playbooks for the common jobs.** Rough cut from your footage, trailer or teaser, vertical edit for TikTok, Reels and Shorts, titles and captions, colour grade, sound mix, motion graphics, 3D product shot, generated b-roll, a score made in ryolune, export for a platform, writing a plugin, and a review of your cut. The agent follows the right one, step by step, and checks the result the way the playbook says.
- **It checks its work before it says it's done.** The agent looks at a sheet of frames across what it changed and measures the sound (loudness, peaks, blank frames, holes in the picture), compares it with what you asked, fixes what's off, then tells you in a few lines what changed.
- **It keeps up with you.** While it works, the agent sees what you change in the window between its steps, so it doesn't undo your edits or work from an old picture of the project.
- **Run the agent from a terminal.** `kimchi-cli --file cut.json ask "…"` runs kimchi's agent on a project file without opening the window.

### Better
- **Updates come through lsuite.** kimchi now gets its updates from lsuite with your free lsuite account (sign in once, in the lsuite app). Signed out, Settings › Updates says so instead of showing an error. Updates are still signed and checked exactly as before.

## 0.10.0 — 2026-10-07

### New
- **lsuite AI: the agent works with no setup.** Sign in once with your lsuite account and the Agent panel runs Claude models on a monthly plan: nothing to install, no key to paste. It is the first choice in the agent's providers and in the first-run setup, it signs in every lsuite app on your computer at once, and Settings › Agent shows your plan and how much of the month you have used, with Manage plan and Sign out. When the allowance runs out, the agent says so in one line. Claude Code can run on it too. A demo for now: choosing a plan charges nothing. Bringing your own (Claude Code, Codex, API keys, Ollama) stays free.
- **Plugins.** A Plugins window (⋯ › Plugins) shows what ships with kimchi, what is installed on your computer with a switch for each, and the formats kimchi loads: lsuite plugins, frei0r filters (the ones Kdenlive and Shotcut use), LUTs, and ryolune's, CLAP, VST3 and Audio Unit sound plugins. The inspector's Plugins section puts them on clips; transitions can come from a plugin too.
- **Build a plugin with your agent.** Describe the effect you want; your agent writes it in Rust with kimchi's new plugin SDK, builds it, installs it and tries it on your clip, and it appears in kimchi without a restart. Rebuild it and the new version is used at once.
- **Four plugins made with the SDK** come with kimchi: Halftone, Chromatic aberration, Gradient and Radial wipe.
- **Real logos.** The editors kimchi works with (Premiere Pro, Final Cut Pro, DaVinci Resolve, Media Composer, After Effects, Lightroom, CapCut, iMovie, Blender, Kdenlive, VEGAS, Nuke, Shotcut) and the plugin formats show their own logos in the setup, Home, the export menu and Settings › Keyboard.

### Better
- A plugin that crashes is switched off and kimchi carries on; switch it back on in Plugins once it is fixed.

## 0.9.1 — 2026-10-06

### New
- **Bring your projects from other editors.** kimchi opens and writes OpenTimelineIO (DaVinci Resolve, Nuke, Kdenlive), FCPXML (Final Cut Pro, Resolve), Premiere / Final Cut 7 XML (Premiere Pro, VEGAS) and EDL (Avid and every editor). Use Home › Open from another editor, and ⋯ › Export project for <app> to send the cut back. A summary shows what came through, and Find missing files points moved media at the right folder.
- **Your looks come with you.** LUTs in .cube, .3dl, .csp, .spi and Hald formats, plus Lightroom / Camera Raw and Premiere Lumetri presets, go into a look library you apply from the inspector. Save your own grade as a look or a .cube, and pick export presets for YouTube, Shorts/Reels, Instagram and more.
- **Keys you already know.** Settings › Keyboard switches to Premiere Pro, Final Cut Pro, DaVinci Resolve, Avid, CapCut, Kdenlive, Shotcut, VEGAS or iMovie shortcuts.
- **A first-run setup** asks which editor you come from, whether you want generative AI (and connects a provider if so), and which assistant to use. Help › Set up kimchi brings it back.
- **More models for the agent.** Gemini CLI, Google Gemini, OpenRouter, Groq, Mistral, DeepSeek, xAI, Together, Fireworks, Cerebras, Azure OpenAI, Amazon Bedrock, LM Studio and any OpenAI-compatible server. Their models show in the Agent panel's model picker.

### Fixed
- Regenerate and Variation repeat the original request exactly, including the input pictures, resolution and model settings.
- Plugin folders you add in Settings › Audio are searched when you rescan plugins.
- Copies installed from the .deb or the portable Windows zip are pointed at the right download when an update is out.
- A rendered motion clip that now shows a part of its scene the render doesn't have is marked "Out of date" instead of "Rendered".
- Help opens the kimchi user guide.
- Building kimchi from source asks for Rust 1.92 or later, which its Linux dependencies need.

## 0.9.0 — 2026-10-06

### New
- **A new look.** kimchi now wears lsuite's design system v2: black and white, square corners, hard shadows, film grain behind the chrome, Chakra Petch for the interface and a new mark. Red is kept for what deletes or records.
- **An organised editor.** Every area has a title, toolbars are grouped by what goes together, track headers show their switches at all times, and there is one sidebar on the left for everything (including the Studio's Scene, Properties and Model views) and one on the right for agents.
- **The agent can see.** When it makes a title, an animation or a 3D scene, it looks at the frame it made and fixes what looks wrong before saying it's done. Every picture it looks at appears in the Agent panel, so you see what it saw. It can also look at your media before using it, to pick the best take or describe footage you imported.
- **The agent knows what you mean by "this".** Your selection, the playhead and the clip open in the Studio go along with each request, so "make this shorter" or "put a title here" just works. The line above the message box shows what it will be told.
- **Conversations that stay with a project.** Keep several named conversations per project, go back to earlier ones, and save project memory that every request shares. History survives restarting kimchi.
- **Steer an agent while it works.** Send a new direction without stopping it or losing the edits it already made.
- **Pick the model** right in the Agent panel, including live lists from Zenith and Ollama and custom model names.
- **Zenith agents in kimchi.** Choose Zenith's providers and models from the Agent panel; each project gets its own Zenith workspace.
- **Generate sound.** ElevenLabs brings voices, sound effects and music, and Stability AI brings Stable Audio. Sound lands on audio tracks, keeps its prompt and settings, and can be regenerated like a shot.
- **A modelling workbench in the Studio.** Tools grouped by task with search, selection tools (linked, grow, shrink, invert, boundaries, edge loops and rings), pivots, align and distribute, and moving, rotating and scaling mesh parts with G, R and S.

### Better
- Animation in the Studio is quicker to work with: zoom and frame the timeline, box-select, duplicate and retime keys from the keyboard, jump between keys with Alt+Left/Right, and type exact values. Keyframe drags snap to frames (hold Alt for in-between).
- Path-traced views keep outlines, highlights, the grid and camera and light helpers visible while the picture refines.
- Duplicating, deleting and dragging groups of objects or keys is one undo step and keeps their links, order and animation.
- Claude Code, Codex and other MCP apps see the same pictures as the built-in agent, and so do Ollama models that support images.
- The docs now include a user guide, configuration, project format, architecture and development guides.

### Fixed
- Very large scene coordinates no longer crash the viewport controls, and mesh edits that would overflow are refused without touching the scene or its undo history.
- Switching clips, scenes or projects ends any tool in progress, so a late result can no longer land in the wrong scene.
- Conversation files are saved safely: unreadable history is kept with a visible error instead of being lost.

## 0.8.0 — 2026-10-05

### New
- **A proper audio mixer** shared by playback and exports: track and clip gain, pan, fade curves, buses, sends, automation, ducking, level meters and a master true-peak limiter.
- **Ryolune inside the sound engine.** Use its stock effects, discover external plugins, import `.ryolune` songs as mixes or stems, refresh changed songs, and send an editable multitrack session back to Ryolune.
- **Voice-over recording** with a count-in, selectable input and one-step undo for placing a take.
- **Audio exports** in WAV, AIFF, FLAC, MP3, AAC, Opus and Ogg, with sample-rate, bitrate, stems and loudness controls. Beat detection and timeline beat markers help cut to music.
- **Camera navigation you can find.** Visible orbit, pan, zoom and fly controls, a Camera menu, camera-to-view locking, and editable camera move presets in the 3D Studio.
- Authentic provider logos, bundled locally for OpenAI/ChatGPT/Codex, Claude and other supported services, with source attribution.

### Better
- The editor adapts its side panels, timeline controls, mixer and dialogs to the available window size. The Studio toolbar wraps and its side panels become drawers in compact windows.
- Renderer optimisations cover row compositing, colour calculations, large blurs, procedural noise, reusable GPU buffers and path tracing. Point lights now cast shadows in the standard engine.
- The Motion Studio adds direct canvas zoom and pan controls alongside its existing layer, keyframe and graph tools.

### Fixed
- Preview audio recovers after a buffer underrun and supports integer-format output devices as well as floating-point devices.
- Finishing a recording stops the input callback before draining and closing its WAV file; failed placement preserves the take.
- The agent panel's opening animation stays within the window, and compact toolbars respond to their actual available width.

## 0.7.0 — 2026-10-04

### New
- **3D like Blender, inside kimchi.** Model your own shapes: turn any object into a mesh and extrude, inset, bevel, loop cut, subdivide, bridge, spin or knife it. Stack modifiers (subdivision, mirror, array, bevel, solidify, boolean, displace, twist, bend, taper, wave, wireframe, explode, build…) that stay editable and animatable. New shapes: icosphere, capsule, grid, extruded logos from SVG paths (holes kept), lathed profiles, curve tubes that draw on, OBJ and STL models.
- **Light, cameras and materials**: spot and area lights with soft shadows, worlds (gradient, daylight sky, 360° panoramas) that light and reflect, glass, clear coat and procedural surfaces (marble, wood, checker, cells, bricks…), several cameras with cuts between them, depth of field, orthographic views, constraints (look at, follow a path), 3D particles, bloom, motion blur, ambient occlusion and filmic tone mapping.
- **A path tracer** for final frames: true reflections, light through glass, soft shadows and bounced light. Pick it per scene; the preview stays fast.
- **2D motion design like After Effects**: nested compositions with time remapping, parenting and nulls, masks you can feather and combine, track mattes, adjustment layers, 34 effects (glow, blurs, drop shadow, outline, echo, colour, fractal noise, turbulent displace, waves, glitch, chromatic aberration, halftone, kaleidoscope, corner pin…), shape operators (repeater, zig zag, wiggle, offset, round corners), text animators, text on a path, particles and motion blur.
- **Expressions** on any property, in 2D and 3D: `wiggle(2, 30)`, `loopOut("pingpong")`, `time * 90`, following another layer with `prop()`, staggering with `index`.
- **The Studio**: double-click a motion clip to edit it in a full workspace with an outliner, a 3D viewport (orbit, handles, edit mode) or a 2D canvas, every property of what you pick, a dope sheet and a graph editor.
- **Render now or at export**: a motion clip is drawn live (quickly while you edit, at full quality in the export), or **Render** it ahead into a file the timeline plays smoothly. Change the scene and it goes back to live until you render it again.
- Nine new templates: glitch title, particle burst, kinetic sweep, radial burst, liquid background, product shot, extruded logo, particle field and morphing blob.
- **Everything the window does is a command**: the Agent panel (`agent.*`), timeline toggles, panel sizes and any shortcut (`ui.action`), so scripts, MCP clients and agents can do all of it. New motion commands for stacks, expressions, materials, compositions, modelling, views from any angle and rendering ahead.

### Better
- The agent's guide (`motion.guide`) covers every new feature, with examples, and errors say what to write instead, and where (`at layers › "sparks" › emitterSize`).
- 3D text can have rounded front edges, bevelled logos shade smoothly, and textures wrap with their seam at the back.
- Shadows hold in small recesses and on thin ledges, and large floors no longer get a dark band towards the horizon or holes near the camera.
- Many layers with drop shadows or glows draw several times faster.
- Codex as the built-in agent can delete and replace things, and sends lists the way kimchi expects.

### Fixed
- A dashed outline keeps its dashes while it draws on.
- `"rotation": 45` on a 3D object explains that it takes `[x, y, z]` instead of turning it around every axis.
- "Revert this run" no longer shows as reverted after the revert itself was undone.
- Window commands sent right as kimchi starts wait for the window instead of failing.
- Snapping, ripple and loop toggles are reported to scripts and agents.
- Freeze-frame stills get readable names in the media panel.
- The agent's own commands are no longer listed twice in the Changes tab.

## 0.6.0 — 2026-10-03

### New
- **What's new**: after an update, kimchi shows what changed. Open it any time from Help › What's New, the command palette or Settings › Updates.
- **Logs and crash reports**: kimchi keeps a log of each run and writes a report when something goes wrong, including when it didn't quit properly. Settings › Diagnostics lists them and shows the log, and Help › Report a Problem opens an issue with your version and system filled in. Nothing is sent anywhere.
- **Updates on Windows**: the installer is downloaded and verified in the background, then runs when kimchi restarts or quits.
- **Updates while kimchi stays open**: it checks again every few hours, and can download and install updates by itself (Settings › Updates). **Restart now** right from Settings.
- **Window buttons** on Windows and on Linux desktops that don't draw their own, and the top bar moves the window on Windows.
- New commands: `app.whatsNew`, `app.diagnostics`, `app.logs`, `app.crashReports`, `app.clearCrashReports`, `app.restart`; `ui.showPanel` opens `shortcuts`, `whatsNew` and `diagnostics`.

### Better
- **Phone media**: portrait photos stay upright, HDR video from recent phones is tone-mapped, and transparent WebM keeps its transparency.
- **Long recordings**: hundreds of cuts from one file export on every system, and NTSC frame rates (29.97, 59.94, 23.976) are exact.
- **Captions** from other tools: SRT and WebVTT files in UTF-16 or Windows encodings, with odd spacing or no blank lines, import correctly, and text like "I <3 you" stays whole.
- kimchi finds ffmpeg, Claude Code and Codex installed with Homebrew, npm, nvm, Snap, Scoop, Chocolatey or WinGet, also when opened from the Dock or Finder.
- Generations ride out a short network drop instead of failing (and losing a render already paid for); a ComfyUI job removed from its queue stops waiting.
- The built-in Codex agent only reaches kimchi's own tools.
- Wording that matches your system: Ctrl instead of ⌘, Explorer or your file manager instead of Finder, and where your keys are kept.

### Fixed
- An edit that fails halfway (moving several clips, deleting across tracks) no longer leaves the project half-changed.
- Deleting a project no longer removes media its duplicate uses.
- An agent's batch of changes can't stay open after it's stopped, or roll back edits made by someone else meanwhile.
- Typing `?` in a text field types it; Ctrl+Enter sends on Linux and Windows; renaming a track and slider arrow keys keep the keyboard; Space, Delete and other editing keys no longer act on the timeline behind a dialog.
- Delete on a media file asks first, as its menu does; "Clear finished" clears; errors show above dialogs.
- Switching projects stops the agent's run and resets its conversation, the clipboard and the generation target.
- Ruler labels and times no longer repeat or read "1:60"; zoom to fit matches the Fit button.
- Playing without a sound device no longer fills memory; a file without a preview stops showing a loader.
- Reversed clips and clips past the end of their video show a picture instead of nothing.
- Sound cut from AAC recordings starts exactly where it should, whichever ffmpeg kimchi uses.
- Logging out or shutting down is a proper quit.
- Many smaller fixes: unbounded times, huge timeouts and tiny animation lengths can't hang or break a project, project files are written safely, and the CLI sends paths relative to where it runs.

## 0.5.0 — 2026-10-03

### New
- **Motion graphics and 3D**: keyframes with easings on every clip, presets, 2D and 3D motion clips, 15 templates and a scene editor.
- **Transitions** on cuts, with sound crossfades in the export.
- **Colour**: grading, chroma key and LUTs, graded on the media or the layer.
- **Speed**: reverse, freeze frames and speed presets.
- **Captions**: Whisper runs on this computer; import and export SRT and WebVTT; a Captions tab.
- **GPU export**: hardware encoders are tried once, with a CPU fallback.

## 0.4.0 — 2026-10-02

### New
- kimchi is now a native app (GPUI) instead of a web view: faster, smaller, and the same on every system.
- **One command registry** for the window, the built-in agent, `kimchi-cli` and MCP, with one undo history.
- **Built-in agent** (Claude Code, Codex, Anthropic, OpenAI, Ollama) with "Revert this run" and permissions.
- Hand-offs with ryolune, and lsuite discovery.

## 0.1.1 — 2026-10-01

### New
- Signed and notarized macOS builds.

## 0.1.0 — 2026-10-01

### New
- The first kimchi: a timeline editor with image and video generation from cloud and local models, bundled ffmpeg and automatic updates.
