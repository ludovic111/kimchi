# The project file

A kimchi project is one JSON document: `project.json` in the project's folder in the library, or
any `.json` file opened with `project.open --path` or `kimchi-cli --file`. This page describes its
structure. The Rust definitions are the reference: `crates/kimchi-core/src/model.rs` and the
modules it uses (`anim.rs`, `effects.rs`, `transition.rs`, `audio.rs`, `motion/`).

`project.get` returns the open project in this format; `project.overview` returns a shorter
summary meant for reading.

> **Prefer commands to editing the file.** Commands validate every change, keep the timeline's
> invariants (clips sorted and never overlapping on a track) and make undo steps. If you do edit
> a file by hand, do it while kimchi doesn't have it open, and use `kimchi-cli --file` to check it
> loads.

## Conventions

- **Times** are seconds (floating point). Clip times (`start`, `duration`) are on the timeline;
  `in_point` is in the source; keyframe times count from the clip's start; mixer automation times
  are on the timeline.
- **Positions** are project pixels measured from the canvas centre, with positive `y` downwards.
  **Rotation** is in degrees, clockwise. **Opacity** is 0 to 1.
- **Colours** are hex strings: `#rgb`, `#rrggbb` or `#rrggbbaa`.
- **Sound levels** are decibels (0 is unchanged, −96 or below is silent, +12 at most); **pan** runs
  from −1 (left) to 1 (right).
- **Ids** are UUIDs (v4) for projects, assets, tracks, clips, markers and buses. Motion scene items
  use short ids of your choice.
- **Field names** are `snake_case` in the core document (project, assets, tracks, clips, text,
  effects, transitions) and `camelCase` in the audio mix (`audio`, `mix`, `mixer`), an asset's
  `beats` and song origin, render records (`rendered`) and motion scenes. Tagged unions use a
  `"type"` field.
- **Optional fields.** Many fields are optional when reading. Later additions (keyframes, effects,
  transitions, renders, the audio mix…) are left out when they hold their default, so a project that
  doesn't use them has no trace of them; the original fields are always written.

There is no format version number. Older files open because new fields always have defaults.
Unknown fields are ignored (and dropped the next time kimchi saves the file), including in motion
scenes, which are read without the strict checks `motion.*` commands apply. A value kimchi doesn't
know, such as a new transition kind or layer type, stops the file from opening.

## A minimal project

A small valid file. (kimchi writes a few more fields when it saves it, such as the title's `italic`
and `background`.)


```json
{
  "id": "caf47557-2e1f-4553-94c2-31ed11b872a7",
  "name": "Integration check",
  "created_at": "2026-10-03T07:21:16.628847557Z",
  "updated_at": "2026-10-03T07:27:50.952180375Z",
  "settings": { "width": 1920, "height": 1080, "fps": 30.0, "background": "#000000", "sample_rate": 48000 },
  "assets": [],
  "tracks": [
    {
      "id": "aaa85dfd-d71e-4396-ad71-9dfe8391ad79",
      "kind": "video",
      "name": "Video 1",
      "muted": false,
      "hidden": false,
      "locked": false,
      "clips": [
        {
          "id": "62781132-e48c-4230-9ad8-f5e036e5b223",
          "name": "Opening title",
          "start": 0.0,
          "duration": 4.0,
          "in_point": 0.0,
          "speed": 1.0,
          "content": {
            "type": "text",
            "style": {
              "content": "Opening title",
              "font_family": "Manrope",
              "font_size": 120.0,
              "font_weight": 700,
              "color": "#ffffff",
              "align": "center",
              "line_height": 1.1,
              "letter_spacing": -1.0,
              "shadow": true
            }
          },
          "transform": { "x": 0.0, "y": 0.0, "scale": 1.0, "rotation": 0.0, "opacity": 1.0, "fit": "contain" },
          "volume": 1.0,
          "fade_in": 0.2,
          "fade_out": 0.2
        }
      ]
    },
    { "id": "5e0c9a4e-0b7e-4f4e-9d0e-0f5b7f3f6a11", "kind": "audio", "name": "Audio 1",
      "muted": false, "hidden": false, "locked": false, "clips": [] }
  ],
  "markers": []
}
```

## Project

| Field | Type | Notes |
| --- | --- | --- |
| `id` | UUID | |
| `name` | string | |
| `created_at`, `updated_at` | RFC 3339 time (UTC) | `updated_at` changes with every edit. |
| `settings` | [Settings](#settings) | |
| `assets` | [Asset](#asset)[] | The media library of this project. |
| `tracks` | [Track](#track)[] | Index 0 is the top track, drawn in front of the others. |
| `markers` | [Marker](#marker)[] | Optional. |
| `mixer` | [Mixer](#mixer) | Optional: buses and the master. |

## Settings

All fields are required.

| Field | Type | Default for new projects |
| --- | --- | --- |
| `width`, `height` | integer pixels, at least 16 | 1920 × 1080 |
| `fps` | number, above 0 | 30 |
| `background` | colour | `#000000` |
| `sample_rate` | integer Hz | 48000 |

## Asset

A media item: a file the project uses.

| Field | Type | Notes |
| --- | --- | --- |
| `id`, `name` | | |
| `kind` | `video` \| `image` \| `audio` | |
| `path` | string | Absolute path. Imported files are referenced where they are, not copied; a ryolune song's asset is its render in `generated/songs/`. |
| `meta` | object | What ffprobe found: `duration`, `width`, `height`, `fps` (each optional), `has_video`, `has_audio`, `video_codec`, `audio_codec` (optional), `size_bytes`. |
| `origin` | object | Where it came from; see below. |
| `created_at` | RFC 3339 time | |
| `thumbnail` | string | Optional path of a poster image (in the project's `cache/`). |
| `filmstrip` | object | Optional: `path`, `frames`, `frame_width`, `frame_height`, `interval` (seconds between frames). |
| `waveform` | object | Optional: `path` of a little-endian f32 peaks file, `peaks_per_second`. |
| `proxy` | string | Optional path of a transcode the preview plays instead of the source. |
| `beats` | object | Optional: `tempo` (BPM), `beatsPerBar` (default 4), `times` (source seconds), `firstDownbeat` (an index into `times`), `source` (`detected` or `ryolune`). |

`thumbnail`, `filmstrip`, `waveform` and `proxy` are made when the media is imported or generated; a
file without them is valid, and a missing proxy falls back to the source.

**`origin`** is one of:

- `{"type": "imported"}`
- `{"type": "generated", …}`, the generation's provenance:

  | Field | Notes |
  | --- | --- |
  | `job_id` | The generation job |
  | `provider`, `model`, `model_name` | Provider id, model id and its display name |
  | `task` | What was asked: text to image, image to video… |
  | `prompt`, `negative_prompt` | `negative_prompt` is optional |
  | `seed` | Optional integer |
  | `params` | The complete request, so it can be run again or varied |
  | `inputs` | Asset ids fed to the model (reference images, first and last frames) |
  | `elapsed_ms`, `cost_usd` | `cost_usd` is optional |

- `{"type": "song", …}`, a render of a ryolune song: `song` (path of the `.ryolune` file),
  `songModified` (when it was last rendered from), `track` (a stem's track id, when the asset is
  one stem), `title`, `tempo`, `beatsPerBar`.

## Track

| Field | Type | Notes |
| --- | --- | --- |
| `id`, `name` | | |
| `kind` | `video` \| `audio` | Video tracks hold pictures: video, images, titles, solids, motion. |
| `muted`, `hidden`, `locked` | boolean | Optional, default `false`. |
| `captions` | boolean | Optional. `true` makes this video track the captions track. |
| `clips` | [Clip](#clip)[] | Sorted by `start`; clips on a track never overlap. |
| `mix` | [Track mix](#track-mix) | Optional. |

## Clip

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `id`, `name` | | | |
| `start` | seconds | | Where the clip begins on the timeline. |
| `duration` | seconds | | Its length on the timeline, after speed. At least 1/60 s. |
| `in_point` | seconds | 0 | Where it starts in the source. |
| `speed` | number | 1 | 0.1 to 16. |
| `reverse` | boolean | `false` | Plays the source range backwards. |
| `content` | object | | What the clip shows; see below. |
| `transform` | object | | See below. |
| `volume` | number | 1 | A gain factor, 0 to 4. |
| `fade_in`, `fade_out` | seconds | 0 | Fades of picture and sound. |
| `keyframes` | object | | Optional. See [Keyframes](#keyframes). |
| `effects` | object | | Optional. See [Effects](#effects). |
| `transition` | object | | Optional. How the clip comes in; see [Transition](#transition). |
| `rendered` | object | | Optional. A motion clip rendered ahead; see below. |
| `audio` | object | | Optional. See [Clip audio](#clip-audio). |

**`content`** is one of:

| `type` | Fields | |
| --- | --- | --- |
| `media` | `asset_id` | A video, image or audio asset. |
| `text` | `style` | A title; see below. |
| `solid` | `color` | A plain colour filling the frame. |
| `motion` | `scene`, `template` | A 2D or 3D scene drawn by kimchi. `template` (optional) is `{id, params}` when the clip came from a template. The scene format is what `motion.guide` documents. |
| `pending` | `job_id`, `kind`, `prompt`, `model_name` | A placeholder while a generation runs; it becomes `media` when the result lands, and disappears if it fails. |

**`transform`** (optional; without it the clip is centred and fitted, but if present, every field
except `fit` is required): `x`, `y` (pixels from the centre), `scale` (1 fits the canvas), `rotation`
(degrees), `opacity` (0–1) and `fit`: `contain` (the whole picture, letterboxed; the default),
`cover` (fills the frame, cropped) or `stretch`.

**`style`** (titles; `content`, `font_family`, `font_size`, `font_weight` and `color` are required):
`content` (the words; `\n` breaks lines), `font_family`, `font_size`
(project pixels), `font_weight` (100–900), `italic`, `color`, `background` (optional box colour),
`align` (`left`, `center`, `right`), `line_height` (default 1.15), `letter_spacing`, `shadow`.

**`rendered`** records the file a motion clip was rendered into: `file` (lossless Matroska with
transparency, in the project's `cache/rendered/`), `key` (a hash of what it was made from), `from`
(scene time of the first frame), `fps`, `frames`, `width`, `height`, and `engine` (`standard` or
`path`). When the scene, the project's size or frame rate, or a picture or model the scene uses
changes, the key no longer matches and kimchi draws the clip live again.

## Keyframes

`keyframes` maps a property name to a list of keyframes, sorted by time:

```json
"keyframes": {
  "opacity": [ { "time": 0.0, "value": 0.0 }, { "time": 0.6, "value": 1.0, "easing": "easeOutCubic" } ],
  "position": [ { "time": 0.0, "value": [-400.0, 0.0] }, { "time": 2.0, "value": [0.0, 0.0], "easing": "ease" } ]
}
```

- `value` is a number, an array of numbers, or a string. Colours blend; SVG path strings with the
  same structure morph point by point; other strings hold until the next keyframe.
- `easing` shapes the move **into** its keyframe from the one before, and is omitted when linear.
- When reading, kimchi also accepts `t`/`at` for `time`, `v` for `value`, `ease` for `easing`, and
  the short form `[time, value]` or `[time, value, "easing"]`.

**Clip properties that take keyframes:** `x`, `y`, `position` (`[x, y]`), `scale`, `scaleX`,
`scaleY`, `rotation`, `opacity`, `blur`, `volume`, `pan`; on titles, `fontSize`, `color` and
`letterSpacing`; and the colour corrections `brightness`, `contrast`, `saturation`,
`temperature`, `tint`, `vignette`, `sharpen`.

**Easings:** `linear`, `hold`, `ease`, `easeIn`, `easeOut`, `easeInOut`; `easeIn`, `easeOut` or
`easeInOut` followed by `Sine`, `Quad`, `Cubic`, `Quart`, `Quint`, `Expo`, `Circ`, `Back`,
`Elastic` or `Bounce` (for example `easeOutBack`); `cubicBezier(x1, y1, x2, y2)`; `spring` or
`spring(b)` with `b` from 0 to 1. `ease` is the CSS curve `cubicBezier(0.25, 0.1, 0.25, 1)`; plain
`easeIn`, `easeOut` and `easeInOut` are cubic.

## Effects

| Field | Range | Notes |
| --- | --- | --- |
| `brightness`, `contrast`, `saturation`, `temperature`, `tint` | −1 to 1 | 0 is unchanged. |
| `vignette`, `sharpen` | 0 to 1 | |
| `chroma_key` | object | Optional: `color` (default `#00b140`), `similarity` (0.5), `softness` (0.1), `spill` (0.5), each 0 to 1. |
| `lut` | object | Optional: `path` (absolute path of a 3D `.cube` file) and `strength` (0 to 1, default 1). |

## Transition

| Field | Notes |
| --- | --- |
| `kind` | `dissolve`, `dipToBlack`, `dipToWhite`, `wipeLeft`, `wipeRight`, `wipeUp`, `wipeDown`, `slideLeft`, `slideRight`, `slideUp`, `slideDown`, `pushLeft`, `pushRight`, `pushUp`, `pushDown`, `zoom`, `iris`, `blur` |
| `duration` | Seconds, up to 30. The played length is limited by the clips; see [Transitions](guide/transitions-and-colour.md#length). |
| `easing` | Default `easeInOutSine`. |

## Clip audio

`audio` on a clip, all fields optional:

| Field | Default | Notes |
| --- | --- | --- |
| `pan` | 0 | −1 to 1 |
| `fadeCurve` | `linear` | `linear`, `equalPower`, `exponential`, `sCurve` |
| `channels` | `stereo` | `stereo`, `mono`, `left`, `right`, `swap` |
| `pitch` | 0 | Semitones, −24 to 24 |
| `preservePitch` | `true` | Keep the pitch when the speed changes |
| `muted` | `false` | |
| `effects` | `[]` | Up to 8 [effect inserts](#effect-inserts) |

## Track mix

`mix` on a track (and on a bus), all fields optional:

| Field | Notes |
| --- | --- |
| `gainDb`, `pan` | The fader and pan. |
| `solo` | |
| `effects` | Up to 8 [effect inserts](#effect-inserts). |
| `output` | A bus id; absent sends the track to the master. |
| `sends` | Up to 4: `{bus, levelDb, preFader}`. Sends are after the fader unless `preFader` is `true`. |
| `duck` | Lower this track while others sound: `under` (track ids; empty means every track that isn't itself ducked), `amountDb` (−12), `thresholdDb` (−40), `attack` (0.15 s), `release` (0.6 s). |
| `keyframes` | Automation, with timeline times: `gainDb`, `pan`, and effect parameters as `effects.<slot>.<param>`. |
| `armed` | Armed for a voice-over take. |

## Mixer

`mixer` on the project:

- `buses`: up to 16 `{id, name, muted, mix}`. Buses always feed the master; `output` and `sends` in a
  bus's `mix` are ignored.
- `master`: `gainDb`, `effects`, `keyframes` (automation of `gainDb` and effect parameters),
  `limiter` (default `true`), `ceilingDb` (default −1, from −24 to 0) and `loudness` (an optional
  export target in LUFS, −40 to −5).

## Effect inserts

Audio effects use ryolune's insert format, so chains move between the two apps unchanged:

| Field | Notes |
| --- | --- |
| `name` | Display name; with no `plugin`, the stock effect of that name |
| `state` | `active`, `bypassed` or `empty` |
| `id` | The slot's id; automation refers to it. Optional |
| `plugin` | `stock:…`, `native:…`, `clap:…`, `vst3:…` or `au:…` |
| `params` | The plugin's parameter ids (as string keys, `"4"`) → values |
| `blob` | Base64 plugin state, for plugins that keep more than parameters |
| `meta` | Free text. Optional |

## Marker

`{id, time, label, color}`, with `time` in seconds on the timeline.

## Files beside the document

A library project's folder holds:

```
projects/<project id>/
  project.json
  generated/          generated media and rendered ryolune songs (generated/songs/)
  cache/              thumbnails, filmstrips, waveforms, proxies, grabbed frames, motion renders
```

Most of `cache/` can be deleted: a missing proxy falls back to the source, and a missing motion
render makes the clip live again. Thumbnails, filmstrips and waveforms are only made at import, so
deleting them leaves the media panel and timeline without them. Imported media stays where it was on
disk, so moving or deleting the original makes it missing in the project; `project.overview` lists
missing files under `problems`.

`project.json` is written to a temporary file and renamed into place, so a crash never leaves a
half-written document.
