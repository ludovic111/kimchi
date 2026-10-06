# Generation

kimchi sends prompts to image and video models, through your own accounts or models running on
your computer, and puts the results on the timeline like any other footage. Every generated item
remembers how it was made, so it can be made again or varied.

## Connecting a model

Open **Settings › Models & keys** (or click the provider pill on home, or **Open Models & keys** in
the Generate panel). Each provider has its own page.

| Provider | Where it runs | Images | Video | Key from the environment |
| --- | --- | :-: | :-: | --- |
| OpenRouter | cloud | ✓ | ✓ | `OPENROUTER_API_KEY` |
| fal | cloud | ✓ | ✓ | `FAL_KEY` or `FAL_API_KEY` |
| Replicate | cloud | ✓ | ✓ | `REPLICATE_API_TOKEN` |
| OpenAI | cloud | ✓ | | `OPENAI_API_KEY` |
| Google Gemini | cloud | ✓ | ✓ | `GEMINI_API_KEY` or `GOOGLE_API_KEY` |
| xAI | cloud | ✓ | ✓ | `XAI_API_KEY` |
| Runway | cloud | ✓ | ✓ | `RUNWAYML_API_SECRET` or `RUNWAY_API_KEY` |
| Luma AI | cloud | ✓ | ✓ | `LUMAAI_API_KEY` or `LUMA_API_KEY` |
| Black Forest Labs | cloud | ✓ | | `BFL_API_KEY` |
| Stability AI | cloud | ✓ | | `STABILITY_API_KEY` |
| Together AI | cloud | ✓ | ✓ | `TOGETHER_API_KEY` |
| ComfyUI | this computer (`http://127.0.0.1:8188`) | ✓ | ✓ | none |
| Stable Diffusion WebUI: AUTOMATIC1111, Forge, SD.Next, Draw Things (`http://127.0.0.1:7860`) | this computer | ✓ | | none |
| Ollama (`http://127.0.0.1:11434`) | this computer | ✓ | | none |
| Any OpenAI-compatible images server (`http://127.0.0.1:8080/v1`) | this computer | ✓ | | none (an optional key on its page) |

**Cloud providers** need an API key. Paste it on the provider's page and **Save**; **Get a key ↗**
opens the provider's site. kimchi stores keys in the system's keychain (the macOS Keychain, the
Windows Credential Manager, or the Secret Service on Linux) and sends each one only to its own
provider. A key in the environment variable listed above works too, when nothing is saved in the
keychain. **Test connection** checks the key.

**Local providers** need their server running. Change **Server address** if it runs elsewhere.

Each page also has an **Enabled** switch: a disabled provider is hidden from the model pickers.
On the fal and Replicate pages, **Extra models** adds any of their model ids to the picker; the
OpenAI-compatible page takes the server's **Model id**.

The OpenAI key is shared with the agent's OpenAI API choice: saving or removing it in either place
changes both.

## The Generate panel

Open the **Generate** tab (⌘2), or press ⌘G (Ctrl+G) to jump to its prompt.

1. Choose **Image** or **Video** at the top.
2. Write the prompt.
3. Optionally add pictures: reference images (image mode, as many as the model takes), or a start
   frame and, for models that support it, an end frame (video mode). Add them from a file, with
   **Frame at the playhead**, or by dropping an image from the Media tab.
4. Choose the model. The picker lists the connected models that can do what you're asking (image or
   video, with or without pictures), searchable and grouped by provider; it shows each one's price
   and what it can do (lengths, resolution, first and last frame, reference images, sound), and puts
   featured models first. Its refresh button reloads the model lists, which are otherwise kept for
   30 minutes.
5. Set the options the model offers: **Aspect** (the project's by default), **Length**,
   **Quality**, **Variations** (up to 4), **Sound**. **More options** adds **Avoid** (a negative
   prompt), **Seed**, and the model's own settings.
6. Press **Generate** (⌘Enter, or Ctrl+Enter). Enter alone adds a line to the prompt.

The panel only shows controls the chosen model supports, and remembers the model you last used for
images and for video. That choice is also the default for scripts and agents that don't name a model.

**Where results go.** The last row under the options says where: **On the timeline at** the playhead
(the default), or, with its switch off, **Library only**, which keeps results in the media library
without placing them. When one of the actions below chose a spot, a **Lands …** chip above the prompt
names it; its × goes back to the playhead.

## Generation in the edit

These prefill the Generate panel with the right pictures and destination; you still choose the
model and press Generate. The placeholder marks the spot, but the shot is as long as the **Length**
you choose; when it lands, a longer shot pushes the later clips on its track to the right.

| Action | Where | What it makes |
| --- | --- | --- |
| **Generate at playhead** | Timeline toolbar | A video at the playhead |
| **Generate video / image here…** | Right-click empty track space | A video or image placed in the gap (or after the track's last clip) |
| **Animate this frame** | Clip menu, or the inspector's With AI section | A video starting from the frame under the playhead, after the clip |
| **Extend shot** | Clip menu (**Extend shot with AI**), or With AI | A video continuing from the clip's last frame, right after it |
| **Restyle frame** | Clip menu, or With AI | An image edited from the frame under the playhead |
| **Bridge two shots** | Select two clips on video tracks, then **Bridge with AI** in the clip menu, or **Bridge these two shots** in the inspector | A video from the first clip's last frame to the second's first, between them |
| **Animate / Edit with AI** | A library image's menu or inspector, or the Recent list's menu | A video from the image, or an edited image |

The command palette also turns what you type into a generation: **Generate video: “…”** and
**Generate image: “…”** start at once with the default model. If that fails (no model is set up, for
example), kimchi says why and opens the Generate panel with the prompt.

## While it runs

The result's place on the timeline is held by a **placeholder** clip that shows the prompt and the
progress. If the placeholder can't be placed, the job isn't started, so nothing is billed without
somewhere to land. When the job finishes, the placeholder becomes the result (extra variations go to
the library). If it fails, the placeholder disappears and a notice gives the reason; if you cancel
it, the placeholder just goes away.

The Generate panel's **Recent** list shows running jobs with their progress and a **Cancel** button,
then the project's latest generated media (double-click to insert one at the playhead; right-click to
insert, reuse its prompt, animate or edit it). The top bar's **Generations** list shows every job,
finished ones with their cost, and **Clear finished**. Each cloud provider runs up to six jobs at
once, and each local one a single job; the rest wait as "Queued".

Generated media is stored in the project's folder in kimchi's library (`projects/<id>/generated/`),
and is deleted with the project. It stays there for projects opened from a file elsewhere too, and
**Save a copy** (`project.saveAs`) writes only the project file, not the media.

## Provenance

Select a generated clip or item: the inspector shows its prompt, provider, model, seed, the time
it took, its cost, what it avoided, and the pictures it was made from.

- **Regenerate** fills the Generate panel with the same prompt, model and seed, landing after the
  clip; press Generate.
- **Variation** does the same with a random seed.

Both are also in the clip's right-click menu. In 0.8.0 they don't bring back the input pictures, the
resolution or the model's settings; `generate.regenerate` repeats the request exactly. **Reuse
prompt**, in the Recent list's menu, puts an earlier prompt back in the composer.

## Your own ComfyUI workflows

Each ComfyUI workflow you export in API format (Workflow › Export (API)) into
`~/Documents/kimchi/comfyui-workflows` becomes a model; choose another folder on the ComfyUI page
of Settings. Every checkpoint installed in ComfyUI is offered as a model too, through kimchi's
built-in graphs.

In the workflow, write placeholders in any text value and kimchi fills them in:

| Placeholder | Value |
| --- | --- |
| `{{prompt}}`, `{{negative_prompt}}` | The prompt and the Avoid text |
| `{{seed}}` | The seed (random if none is set) |
| `{{width}}`, `{{height}}` | The size for the chosen aspect, in multiples of 16 |
| `{{count}}` | The number of variations |
| `{{steps}}`, `{{cfg}}`, `{{denoise}}` | Defaults 20, 7 and 0.75 |
| `{{duration}}`, `{{fps}}`, `{{frames}}` | Defaults 5 s and 24 fps |
| `{{image}}` (or `{{start_image}}`), `{{end_image}}` | The input pictures, uploaded for you |
| `{{sampler}}`, `{{scheduler}}`, `{{checkpoint}}` | |
| `{{anything}}` | Any other parameter of the request |

A value that is exactly one numeric placeholder (`"{{steps}}"`) becomes a number. A workflow that
saves a video (SaveVideo, SaveWEBM, VHS_VideoCombine…) is a video model; one that uses an image
placeholder takes a picture.

An optional sidecar, `<workflow name>.kimchi.json`, names and describes the model and sets its
tasks, defaults and extra settings:

```json
{
  "name": "Wan 2.2 image to video",
  "description": "My local image-to-video graph",
  "tasks": ["image_to_video"],
  "defaults": { "steps": 30, "fps": 16 }
}
```

## Generating from scripts and agents

Everything here is a command: `generate.submit`, `generate.animateFrame`, `generate.extendClip`,
`generate.bridge`, `generate.jobs`, `generate.cancel`… See
[AI_CONTROL.md](../AI_CONTROL.md#generation). Agents can generate only while Settings › Agent ›
Permissions › **Generate** is on, and never see your keys (only their last four characters).
