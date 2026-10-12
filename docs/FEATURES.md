# What kimchi does

The full list of kimchi's features, by area. The [README](../README.md) has the short version;
the [user guide](guide/README.md) explains how to use each one.

## Features

### Editing

- Multi-track timeline: video, image, text, solid and audio clips
- Split, trim, ripple delete, duplicate, copy / cut / paste, cross-track moves, snapping, markers
- Rubber-band selection, ⌥-drag to copy, files dropped straight onto a track
- The shortcuts editors expect: J / K / L shuttle, ↑ / ↓ to the previous / next cut, Q / W trim to the playhead; press ? for all of them
- Per-clip transform (position, scale, rotation, opacity, blur, fit), speed (pitch kept), reverse, freeze frames, fades, volume
- **Transitions** on cuts: dissolve, dip to black / white, wipes, slides, pushes, zoom, iris, blur; centred on the cut with the clips' media past it, the sound crossfading. Click the + on a cut, drag the badge's edges for the length
- **Colour**: looks (punchy, warm, cool, mono, faded, vintage, noir, teal & orange, dreamy), brightness, contrast, saturation, warmth, tint, vignette, sharpen (all keyframable), green-screen chroma key
- **A look library**: LUTs (.cube, .3dl, .csp, .spi1d / .spi3d, Hald images), Lightroom / Camera Raw and Premiere Lumetri presets, applied from the inspector; save your own grade as a look or a `.cube`
- **Captions**: transcribe the cut on your computer (Whisper, nothing sent anywhere; the model downloads once), import and export SRT / WebVTT, edit them as titles, style them all at once; burned in or as an `.srt` beside the export
- Live preview compositor, on-canvas move/scale handles
- Snapshot undo/redo for every edit, autosave
- Export to MP4 (H.264), HEVC, ProRes, WebM, GIF or audio-only, with presets for YouTube, Shorts / Reels, Instagram and more

### Coming from another editor

- Open and write OpenTimelineIO (DaVinci Resolve, Nuke, Kdenlive), FCPXML (Final Cut Pro, Resolve), Premiere / Final Cut 7 XML (Premiere Pro, VEGAS) and EDL; a report says what came across, and **Find missing files** relinks moved media. See [SWITCHING.md](SWITCHING.md) and [COMPATIBILITY.md](COMPATIBILITY.md)
- Keyboard layouts of Premiere Pro, Final Cut Pro, DaVinci Resolve, Avid, CapCut, Kdenlive, Shotcut, VEGAS and iMovie (Settings › Keyboard)
- A first-run setup asks where you come from, whether you want generative AI, and which agent to use (Help › Set up kimchi brings it back)

### Sound and ryolune

- A Rust mixer shared by playback, exports and speech transcription: clip and track gain/pan, editable fade shapes, buses, sends, automation, ducking, meters and a master true-peak limiter
- ryolune's stock effects and plugin host, with searchable effect lists and parameter panels; CLAP, VST3, native plugins and Audio Units on macOS, subject to the plugin and platform
- Loudness measurement and normalization, beat detection, beat snapping and cuts on music beats
- Voice-over recording with a count-in; finish a take to place it on an audio track, with undo
- Import `.ryolune` songs as mixes or stems, refresh after changes, and send an editable multitrack audio session back to ryolune
- WAV (16/24-bit or float), AIFF, FLAC, MP3, AAC, Opus and Ogg exports, with sample rate, bitrate, stems and loudness options

### A window that fits

- Side panels become drawers on small windows; dialogs and popovers fit the available space, and inspector sections fold
- Real provider and app logos in menus, settings and generation results

### Animation, motion graphics and 3D

Drawn by kimchi, the same in the preview and the export.

- **Keyframes on any clip**: position, scale, rotation, opacity, blur, volume, text size and colour, with easings (ease, back, elastic, bounce, cubic-bezier, spring). Toggle a keyframe per property at the playhead in the inspector, or drag on the canvas
- **Presets**: fade, rise, slide, pop, zoom, focus in and out; Ken Burns, pan, pulse, float, shake, spin
- **2D motion design, like After Effects**: shapes, paths (with draw-on), text, images, particles, nulls, adjustment layers and nested compositions with time remap; parenting, masks (add, subtract, intersect, feathered), track mattes (alpha and luma), blend modes; 34 effects (blurs, glow, drop shadow, outline, echo, colour correction, levels, tint, gradient ramp, fractal noise, grain, halftone, turbulent displace, wave warp, ripple, twirl, bulge, mosaic, chromatic aberration, glitch, kaleidoscope, motion tile, corner pin…), shape operators (repeater, zig zag, wiggle, offset, round corners, twist, pucker & bloat), text animators (range and wiggly selectors), text on a path, motion blur
- **3D, like Blender**: primitives (box, sphere, icosphere, cylinder, cone, capsule, torus, plane, grid), extruded text and logos (SVG paths, holes kept), lathed profiles, curve tubes that draw on, glTF / OBJ / STL models, and editable meshes (extrude, inset, bevel, loop cut, subdivide, bridge, spin, knife…); a modifier stack (subdivision, mirror, array, bevel, solidify, boolean, displace, twist, bend, taper, wave, wireframe, explode, build…); constraints (look at, follow a path, copy, limit); cameras with depth of field, orthographic views and cuts between cameras; directional, point, spot and area lights with soft shadows; environments (gradient, sky, 360° panorama); materials with glass, metal, clear coat, textures and procedural patterns (marble, wood, checker, cells…); particles; bloom, motion blur, ambient occlusion and filmic tone mapping
- **Two render engines**: a fast GPU engine (Metal on Macs, Apple Silicon included; Vulkan or DirectX 12 elsewhere; on the CPU otherwise) and a path tracer for real reflections, refraction through glass, soft light and bounced light
- **Expressions** on any property: `wiggle(2, 30)`, `loopOut("pingpong")`, `time * 90`, `prop("ball", "x") + 100`, staggering by `index`
- **The Studio**: a workspace for a motion clip with an outliner, a 3D viewport (orbit, move / rotate / scale handles, edit mode) or a 2D canvas (handles, pen tool), the properties of what's selected (modifiers, effects, materials, expressions, world and render settings), a dope sheet and a graph editor
- **Two sidebars**: navigation, Studio, scene properties and the searchable modelling workbench share the left sidebar; agents have the right sidebar. The workbench exposes mesh operations with editable parameters and undo.
- **Camera navigation**: drag the visible orbit, pan and zoom controls, use the axis ball, or fly with WASD/QE. The Camera menu can lock the scene camera to your view, align it, add cameras and apply editable camera moves. The 2D canvas has visible pan, zoom and fit controls.
- **Render now or at export**: a motion clip is drawn live (quick in the preview, full quality in the export), or rendered ahead into a file the timeline plays smoothly; edit the scene and it goes back to live until you render it again
- **Templates**: lower third, title card, kinetic type, counter, bar chart, logo reveal, callout, quote, subscribe button, aurora background, wipe transition, glitch title, particle burst, kinetic sweep, radial burst, liquid background, 3D title, 3D logo spin, turntable, floating shapes, product shot, extruded logo, particle field, morphing blob; change their words and colours in the inspector
- **Ask the agent**: it writes the scene (`motion.guide` explains every feature), models meshes, then looks at the frames it made from any angle and fixes what looks wrong
- Hardware encoding where the computer has it (Apple VideoToolbox, NVIDIA NVENC, AMD AMF, Intel Quick Sync, VA-API), checked with a test encode and redone on the CPU if it fails; 4K, HEVC and ProRes sources decode in hardware

### Generation, woven into the edit

- **Generate at the playhead.** A placeholder clip appears where the shot will go and turns into the result when it's done
- **Generate in a gap.** Right-click empty space on a track; the shot is sized to fill it
- **Animate this frame.** Turn the frame under the playhead into a moving shot (image-to-video)
- **Extend shot.** Continue a clip from its last frame, landing right after it
- **Bridge two shots.** First frame and last frame, from the clips on either side
- **Restyle frame.** Edit a still with an image model
- **Provenance.** Every generated asset remembers its prompt, model, seed and inputs: regenerate or make variations in one click
- Start a project from a prompt on the home screen, or type one into the command palette (⌘K)

## Models

Keys live in your OS keychain (or come from the usual environment variables) and
are only sent to the provider they belong to.

| Provider | Kind | Images | Video |
|---|---|:-:|:-:|
| [OpenRouter](https://openrouter.ai) | cloud | ✓ | ✓ |
| [fal](https://fal.ai) | cloud | ✓ | ✓ |
| [Replicate](https://replicate.com) | cloud | ✓ | ✓ |
| [OpenAI](https://platform.openai.com) | cloud | ✓ | |
| [Google Gemini](https://ai.google.dev) | cloud | ✓ | ✓ |
| [xAI](https://x.ai/api) | cloud | ✓ | ✓ |
| [Runway](https://dev.runwayml.com) | cloud | ✓ | ✓ |
| [Luma AI](https://lumalabs.ai/api) | cloud | ✓ | ✓ |
| [Black Forest Labs](https://bfl.ai) | cloud | ✓ | |
| [Stability AI](https://platform.stability.ai) | cloud | ✓ | |
| [Together AI](https://together.ai) | cloud | ✓ | ✓ |
| [ComfyUI](https://www.comfy.org) | local | ✓ | ✓ |
| Stable Diffusion WebUI (A1111, Forge, SD.Next, Draw Things) | local | ✓ | |
| [Ollama](https://ollama.com) | local | ✓ | |
| Any OpenAI-compatible images server (LocalAI, vLLM-Omni, sd-server…) | local | ✓ | |

Model lists are fetched live where the provider exposes them, and every model
declares what it can do (aspect ratios, durations, resolutions, first/last frame,
reference images, sound), so the generate panel only shows controls that apply.

**Audio and speech** have their own Generate tabs. ElevenLabs supplies speech with account voices,
sound effects and music; Stability AI supplies Stable Audio 2.5. Results enter the media library
and audio timeline with their original prompt and settings, and can be reused or regenerated.
Set the provider key in Settings, or use `ELEVENLABS_API_KEY` / `STABILITY_API_KEY`.

### Your own ComfyUI workflows

Export a workflow in API format into `~/Documents/kimchi/comfyui-workflows` (or
set another folder in Settings → Models & keys → ComfyUI). Each file becomes a
model. Put placeholders in any string input and kimchi fills them in:

`{{prompt}}` `{{negative_prompt}}` `{{seed}}` `{{width}}` `{{height}}`
`{{steps}}` `{{cfg}}` `{{denoise}}` `{{duration}}` `{{fps}}` `{{frames}}`
`{{image}}` / `{{start_image}}` `{{end_image}}` (uploaded for you)

A value that is exactly one numeric placeholder becomes a number. Workflows with
a video save node are treated as video models; an optional
`<name>.kimchi.json` sidecar can set the name, tasks and defaults.
