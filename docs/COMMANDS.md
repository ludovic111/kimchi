# kimchi commands

Generated from the command registry (`crates/kimchi-control`) by `kimchi-cli docs`. Do not edit by hand.

Every command works the same from the window, the built-in agent, `kimchi-cli` and `kimchi-mcp` (where `family.verb` becomes the tool `family_verb`). Times are seconds on the timeline; ids and unique names are accepted wherever an id is expected. Commands that edit the project also accept `coalesce` (edits with the same key within about a second fold into one undo step). See [AI_CONTROL.md](AI_CONTROL.md).

## project

### `project.list`

List the projects in the library, most recent first, with their length, size and whether one is open. _(read only)_

### `project.overview`

The whole open project in one bounded answer: settings, every track with its clips (times, media, text, transforms that differ from the defaults), media with generation provenance, markers, running jobs, undo history, what the window shows, and problems (missing files, placeholders still generating, hidden or muted tracks). Read it first. _(read only)_

### `project.get`

The complete open project as JSON (the project file format). _(read only)_

### `project.renderFrame`

Render what the timeline shows at a time (or a labelled contact sheet of several times) to a PNG and return its path, to look at a result: animations, motion graphics, 3D, the whole cut. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `time` | number |  | Timeline seconds (default: the playhead). |
| `times` | array of numbers |  | Several times in seconds: one image with a frame per time, labelled (up to 16). |
| `width` | integer |  | Width of each frame in pixels (default 960, or 480 in a sheet). |

### `project.create`

Create a project in the library and open it, replacing the open one. _(changes things · permission: projects)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `name` | string |  | Project name (default "Untitled"). |
| `width` | integer |  | Canvas width in pixels (default 1920). |
| `height` | integer |  | Canvas height in pixels (default 1080). |
| `fps` | number |  | Frames per second (default 30). |
| `background` | string |  | Canvas colour #rrggbb (default #000000). |

### `project.open`

Open a library project (projectId) or a project file (path), replacing the open one. _(changes things · permission: projects)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `projectId` | string |  | Project id or unique name, as listed by project.list. |
| `path` | string |  | A project .json file to open in place; every change is saved back to it. |

### `project.close`

Close the open project and go back to the home screen. _(changes things · permission: projects)_

### `project.delete`

Delete a library project and its generated media and caches. Cannot be undone. _(changes things · permission: projects)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `projectId` | string | required | Project id or unique name, as listed by project.list. |

### `project.duplicate`

Copy a library project (the open one by default) as "<name> copy". _(changes things · permission: projects)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `projectId` | string |  | Project id or unique name; defaults to the open project. |

### `project.rename`

Rename the open project (one undo step), or another library project. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `name` | string | required | New name. |
| `projectId` | string |  | A library project (id or unique name); defaults to the open one. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `project.setSettings`

Change the canvas: size, frame rate, background colour, sample rate. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `width` | integer |  | Width in pixels (16 or more). |
| `height` | integer |  | Height in pixels (16 or more). |
| `fps` | number |  | Frames per second. |
| `background` | string |  | Canvas colour #rrggbb. |
| `sampleRate` | integer |  | Audio sample rate for exports, e.g. 48000. |

### `project.saveAs`

Write a copy of the open project to a .json file (the project file format). _(changes things · permission: files)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `path` | string | required | Destination .json file. |

### `project.batch`

Run several commands as one undo step. With atomic (the default) a failing command rolls back the ones before it. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `commands` | array of objects | required | Array of {"command": "clip.update", "params": {…}}. |
| `atomic` | boolean |  | Roll everything back if one command fails (default true). |
| `label` | string |  | Name of the undo step (default "batch"). |

## media

### `media.list`

List the open project's media (imported and generated) with kind, length, size, previews and generation details. _(read only)_

### `media.get`

One media item in full, including how it was generated (prompt, model, seed, inputs). _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `assetId` | string | required | Media id or unique name, as listed by media.list. |

### `media.import`

Import media files (video, image, audio) into the open project. Thumbnails, filmstrips, waveforms and proxies are made in the background. With place, each file is also put on the timeline, one after the other. _(changes things · permission: files)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `paths` | array of strings | required | Absolute paths of the files to import. |
| `place` | boolean |  | Also put each file on the timeline (default false). |
| `trackId` | string |  | Track id or name. Defaults to the first free compatible track (a new one if none is free). |
| `start` | number |  | Timeline position in seconds. Defaults to the playhead. |

### `media.remove`

Remove a media item and every clip that uses it. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `assetId` | string | required | Media id or unique name, as listed by media.list. |

### `media.frame`

Save the frame a clip shows at a timeline time as a PNG and return its path (for image-to-video and references). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `time` | number |  | Timeline time in seconds inside the clip. Defaults to the playhead, or the clip's first frame. |

## track

### `track.list`

List tracks from top to bottom with their kind, flags and clip count. Track 0 is drawn on top. _(read only)_

### `track.add`

Add a track. New video tracks go on top, new audio tracks at the bottom. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `kind` | string | required | "video" (pictures: video, images, text, solids) or "audio". |
| `index` | integer |  | Position from the top (0 = top). |

### `track.remove`

Delete a track and every clip on it. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `trackId` | string | required | Track id or unique name ("Video 1"), as listed by track.list. |

### `track.update`

Rename, mute, hide or lock a track, or make it the captions track. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `trackId` | string | required | Track id or unique name ("Video 1"), as listed by track.list. |
| `name` | string |  | New name. |
| `muted` | boolean |  | Silence the track. |
| `hidden` | boolean |  | Hide the track's pictures. |
| `locked` | boolean |  | Protect the track from edits. |
| `captions` | boolean |  | Make it the captions track (video tracks): its titles are the captions. |

### `track.move`

Move a track to another position (0 = top). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `trackId` | string | required | Track id or unique name ("Video 1"), as listed by track.list. |
| `index` | integer | required | Zero-based target position from the top. |

## clip

### `clip.list`

List clips with their track, start, end, kind and media. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `trackId` | string |  | Only clips on this track. |

### `clip.get`

One clip in full: timing, source in-point, speed, transform, fades, volume, text style. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |

### `clip.insertMedia`

Put a media item on the timeline at its full length. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `assetId` | string | required | Media id or unique name, as listed by media.list. |
| `trackId` | string |  | Track id or name. Defaults to the first free compatible track (a new one if none is free). |
| `start` | number |  | Timeline position in seconds. Defaults to the playhead. |

### `clip.addText`

Add a title. Text is drawn the same in the preview and the export. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `text` | string | required | The words; \n starts a new line. |
| `start` | number |  | Timeline position in seconds. Defaults to the playhead. |
| `duration` | number |  | Seconds on screen (default 4). |
| `trackId` | string |  | Track id or name. Defaults to the first free compatible track (a new one if none is free). |
| `style` | object |  | Text style fields to change: fontFamily, fontSize (project pixels, default 120), fontWeight (100-900), italic, color (#rrggbb), background (#rrggbb or null), align (left\|center\|right), lineHeight, letterSpacing, shadow. |
| `x` | number |  | Horizontal offset of the centre from the canvas centre, in project pixels. |
| `y` | number |  | Vertical offset of the centre from the canvas centre, in project pixels (positive is down). |

### `clip.addSolid`

Add a solid colour clip (a background, a flash, a fade card). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `color` | string | required | Colour #rrggbb. |
| `start` | number |  | Timeline position in seconds. Defaults to the playhead. |
| `duration` | number |  | Seconds (default 5). |
| `trackId` | string |  | Track id or name. Defaults to the first free compatible track (a new one if none is free). |

### `clip.move`

Move a clip to another time and/or track of the same kind. Whatever it lands on is overwritten. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `start` | number |  | New start in seconds. |
| `trackId` | string |  | Destination track. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `clip.moveMany`

Move several clips at once, as one undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `moves` | array of objects | required | Array of {clipId, start, trackId?}. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `clip.trim`

Move one edge of a clip to a timeline time, like dragging it. Bounded by the neighbours and the source length. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `edge` | string | required | "start" or "end". |
| `time` | number | required | Timeline time in seconds for that edge. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `clip.split`

Split clips at a time. Without clipIds, every clip under that time on unlocked tracks (or the selection in the window). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `time` | number |  | Timeline time in seconds. Defaults to the playhead. |
| `clipIds` | array of strings |  | Clips to split (ids or names). |

### `clip.delete`

Delete clips. With ripple, later clips on the same track move left to close the gap. Returns the clips as they were (removed), which clip.paste takes back: a cut. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array of strings | required | Clips to delete (ids or names). |
| `ripple` | boolean |  | Close the gap (default false). |

### `clip.duplicate`

Copy clips to the end of their track. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array of strings | required | Clips to duplicate (ids or names). |

### `clip.paste`

Paste copies of clips: the earliest copy starts at time and the others keep their spacing and tracks. Whatever they land on is overwritten. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array of strings |  | Clips in the project to copy (ids or names). |
| `clips` | array of objects |  | Clip objects as returned by clip.get (with trackId), e.g. clips deleted since (a cut). |
| `time` | number |  | Where the earliest copy starts, in seconds. Defaults to the playhead. |
| `trackId` | string |  | Put every copy on this track instead of each clip's own. |

### `clip.update`

Change a clip: name, position, scale, rotation, opacity, fit, volume, fades, speed, reverse, text style or solid colour. Only the given fields change. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `name` | string |  | Clip name. |
| `x` | number |  | Offset of the centre from the canvas centre, project pixels. |
| `y` | number |  | Offset of the centre from the canvas centre, project pixels (positive is down). |
| `scale` | number |  | 1 = fitted to the canvas. |
| `rotation` | number |  | Degrees, clockwise. |
| `opacity` | number |  | 0-1. |
| `fit` | string |  | "contain", "cover" or "stretch". |
| `volume` | number |  | 0-4 (1 = unchanged). |
| `fadeIn` | number |  | Fade-in length in seconds. |
| `fadeOut` | number |  | Fade-out length in seconds. |
| `speed` | number |  | 0.1-16; the clip gets shorter or longer on the timeline. The sound keeps its pitch. |
| `reverse` | boolean |  | Video and sound clips: play the same part of the media backwards. |
| `style` | object |  | Text clips: style fields to change (see clip.addText). |
| `color` | string |  | Solid clips: colour #rrggbb. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `clip.setKeyframes`

Animate one property of a clip: replace its keyframes (times in seconds from the clip's start). Properties: x, y, position ([x, y]), scale, scaleX, scaleY, rotation, opacity, blur (pixels), volume, the effects brightness, contrast, saturation, temperature, tint, vignette, sharpen (see clip.setEffects); text clips also fontSize, color, letterSpacing. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `property` | string | required | The property to animate. |
| `keyframes` | array | required | [{"time": 0, "value": 0}, {"time": 0.6, "value": 1, "easing": "easeOut"}] or [[0, 0], [0.6, 1, "easeOut"]]. A keyframe's easing shapes the move into it: linear (default), hold, ease, easeIn, easeOut, easeInOut, ease<In\|Out\|InOut><Sine\|Quad\|Cubic\|Quart\|Quint\|Expo\|Circ\|Back\|Elastic\|Bounce>, cubicBezier(x1,y1,x2,y2), spring(bounce 0-1). Empty removes the animation. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `clip.addKeyframe`

Set one keyframe of a clip property at a timeline time, replacing one already there (what the window's keyframe buttons do). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `property` | string | required | x, y, scale, scaleX, scaleY, rotation, opacity, blur, volume, brightness, contrast, saturation, temperature, tint, vignette, sharpen, fontSize, color or letterSpacing. |
| `time` | number |  | Timeline seconds (default: the playhead). |
| `value` | any |  | The value (default: what the property is at that time). |
| `easing` | string |  | How the value arrives here from the previous keyframe (default linear). |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `clip.removeKeyframe`

Remove a clip's keyframe at a timeline time, or every keyframe of a property (it then keeps its value at that time, or its own). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `property` | string | required | The animated property. |
| `time` | number |  | Timeline seconds; omit to remove the property's whole animation. |

### `clip.animate`

Give clips a ready-made animation written as ordinary keyframes: entrances (fadeIn, riseIn, slideInLeft, popIn, zoomIn, spinIn, dropIn, blurIn…), exits (fadeOut, slideOutRight, popOut…) or over the whole clip (kenBurns, panLeft, pulse, float, wiggle, shake, spin). motion.presets lists them all. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array of strings | required | Clips to animate (ids or names). |
| `preset` | string | required | Preset name. |
| `length` | number |  | Seconds the move takes (default 0.6; one cycle for repeating ones). |

### `clip.setEffects`

Colour and picture effects on clips: a ready-made look, corrections (brightness, contrast, saturation, temperature, tint), vignette, sharpen, a chroma key (green or blue screen) and a .cube LUT. Drawn in the preview and the export. Only the given fields change; animate the numeric ones with clip.setKeyframes. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array of strings | required | Clips to change (ids or names). |
| `look` | string |  | Start from a look (clip.looks): none, punchy, warm, cool, mono, faded, vintage, noir, teal, dreamy. The other fields given go on top. |
| `brightness` | number |  | -1 to 1 (0 = unchanged). |
| `contrast` | number |  | -1 (flat grey) to 1 (twice the contrast). |
| `saturation` | number |  | -1 (black and white) to 1 (twice as colourful). |
| `temperature` | number |  | -1 (cooler, blue) to 1 (warmer, orange). |
| `tint` | number |  | -1 (greener) to 1 (more magenta). |
| `vignette` | number |  | 0-1: darker corners. |
| `sharpen` | number |  | 0-1. |
| `chromaKey` | any |  | Key a colour out: true (a green screen), a colour #rrggbb (the screen's colour, best picked from the footage), {color, similarity, softness, spill} (0-1 each; similarity 0.5, softness 0.1, spill 0.5 by default), or false to remove it. |
| `lut` | any |  | Absolute path of a 3D .cube LUT, {path, strength}, or null to remove it. |
| `lutStrength` | number |  | 0-1: how much of the LUT shows (default 1). |
| `reset` | boolean |  | Remove every effect first. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `clip.looks`

The ready-made looks clip.setEffects applies, with their values. _(read only)_

### `clip.freezeFrame`

Hold the frame a clip shows at a time: the clip is split there and a still of that frame plays for the duration, pushing the rest of its track later. The still keeps the clip's position, size and effects. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `time` | number |  | Timeline time inside the clip (default: the playhead). |
| `duration` | number |  | Seconds to hold the frame (default 2). |

## transition

### `transition.kinds`

The transitions kimchi draws, with what each looks like. _(read only)_

### `transition.list`

Every transition in the project: the clip it leads into, the clip it leaves (on a cut), kind, length and where it plays. _(read only)_

### `transition.set`

Put a transition at the start of clips. On a cut (the clip before ends where this one starts) it is centred on the cut and both clips play on past it with their media beyond the cut (or hold their edge frame), so nothing moves on the timeline; with no clip right before, the clip transitions in over what is below it. The sound crossfades over the same span. Changes the kind or length of transitions already there. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array of strings |  | The incoming clips (ids or names): each gets a transition at its start. |
| `trackId` | string |  | Instead of clipIds: every cut on this track. |
| `kind` | string |  | dissolve (default), dipToBlack, dipToWhite, wipeLeft, wipeRight, wipeUp, wipeDown, slideLeft, slideRight, slideUp, slideDown, pushLeft, pushRight, pushUp, pushDown, zoom, iris or blur. |
| `duration` | number |  | Seconds (default 0.8). On a cut it can't be longer than the shorter clip; otherwise than half the clip. |
| `easing` | string |  | How the progress moves (default easeInOutSine; any keyframe easing). |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `transition.remove`

Remove the transitions at the start of clips (or every one on a track). One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array of strings |  | Clips whose transition goes (ids or names). |
| `trackId` | string |  | Instead of clipIds: every transition on this track. |

## captions

### `captions.list`

The captions, by time: clip, start, end and words. Captions are titles on the captions track; edit one like any title (clip.update style.content, clip.trim, clip.delete). _(read only)_

### `captions.models`

The speech models captions.transcribe can use, their download size and whether they are on this computer. _(read only)_

### `captions.status`

The transcription running now, if any: stage (downloading the model, mixing, listening) and progress. _(read only)_

### `captions.transcribe`

Caption the cut by listening to it: the mixed sound (or one clip's) is transcribed by Whisper on this computer, split into readable captions (two lines at most) and put on the captions track as titles (the track is made if needed), replacing the captions in that span. One undo step. The model is downloaded the first time (150 MB to 1 GB); then the base model takes about a tenth of the sound's length. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string |  | Only this clip's sound (default: the whole mix). |
| `from` | number |  | Start of the span in seconds (default 0). |
| `to` | number |  | End of the span in seconds (default: the end of the cut). |
| `language` | string |  | Language spoken: en, fr, es, de, ja… (default: detected). |
| `model` | string |  | tiny, base (default) or small (captions.models). |
| `maxChars` | integer |  | Longest caption line in characters (default 42). |
| `replace` | boolean |  | Remove the captions already in the span (default true). |

### `captions.cancel`

Stop the running transcription. _(changes things)_

### `captions.import`

Read an SRT or WebVTT file onto the captions track. One undo step. _(changes things · permission: files)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `path` | string | required | The .srt or .vtt file. |
| `offset` | number |  | Seconds added to every time (default 0). |
| `replace` | boolean |  | Remove the captions in the file's span first (default false). |

### `captions.export`

Write the captions to an SRT or WebVTT file. _(changes things · permission: files)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `path` | string | required | Destination .srt or .vtt. |
| `format` | string |  | srt or vtt (default: from the extension). |

### `captions.add`

Add one caption on the captions track, styled like the others. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `text` | string | required | The words; \n starts a second line. |
| `start` | number |  | Timeline position in seconds. Defaults to the playhead. |
| `duration` | number |  | Seconds (default 2.5). |

### `captions.setStyle`

Restyle every caption at once: text style fields (see clip.addText) and/or their height. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `style` | object |  | Text style fields to change, e.g. {"fontSize": 60, "background": null}. |
| `y` | number |  | Vertical offset of the captions' centre from the canvas centre, project pixels (positive is down). |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `captions.clear`

Remove every caption. One undo step. _(changes things)_

## motion

### `motion.guide`

How to make motion graphics and 3D with kimchi: the scene formats (2D layers, 3D objects, camera, lights), every property, keyframes and easings, text reveals, masks, effects, templates and presets, with examples. Read it before writing a scene. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `topic` | string |  | 2d, 3d, keyframes, templates or all (default). |

### `motion.templates`

Motion templates (lower third, title card, kinetic type, counter, bar chart, logo reveal, callout, quote, subscribe, aurora, wipe, 3D title, 3D logo spin, turntable, floating shapes) with the values each takes. _(read only)_

### `motion.presets`

The ready-made clip animations clip.animate applies. _(read only)_

### `motion.add`

Add a motion clip: 2D motion graphics (layers of shapes, paths, text, images) or a 3D scene (camera, lights, objects, extruded text, glTF models), drawn by kimchi in the preview and the export, every property animatable. Look at the result with project.renderFrame. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `scene` | object | required | The scene, as described by motion.guide. |
| `start` | number |  | Timeline position in seconds. Defaults to the playhead. |
| `duration` | number |  | Seconds on the timeline (default: the last keyframe + 1 s, at least 3). |
| `trackId` | string |  | Track id or name. Defaults to the first free compatible track (a new one if none is free). |
| `name` | string |  | Clip name (default: from the scene). |

### `motion.addTemplate`

Add a motion clip made from a template with your values (see motion.templates). The clip remembers them: motion.setTemplate changes them later. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `template` | string | required | Template id, e.g. lowerThird. |
| `values` | object |  | Template values to change, e.g. {"title": "Grace Hopper"}. |
| `start` | number |  | Timeline position in seconds. Defaults to the playhead. |
| `duration` | number |  | Seconds (default: the template's). |
| `trackId` | string |  | Track id or name. Defaults to the first free compatible track (a new one if none is free). |

### `motion.get`

A motion clip's scene as JSON, or one layer, object or light of it. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `id` | string |  | A layer, object or light id, or "camera". |

### `motion.update`

Replace a motion clip's whole scene. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `scene` | object | required | The new scene. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `motion.setLayer`

Add a layer (2D) or an object or light (3D) to a motion clip, or replace the one with the same id. New 2D layers go on top. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `layer` | object | required | The layer, object or light (with its id). |
| `parent` | string |  | Put it inside this group (2D) or object (3D). |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `motion.removeLayer`

Remove a layer, object or light from a motion clip. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `id` | string | required | Its id. |

### `motion.setKeyframes`

Animate one property of a layer, object, light or the camera inside a motion clip: replace its keyframes (times in scene seconds). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `id` | string | required | A layer, object or light id, "camera", or "scene" (background, ambient). |
| `property` | string | required | The property, e.g. x, opacity, trimEnd, reveal, rotation.y, position, fov. |
| `keyframes` | array | required | [{"time": 0, "value": 0}, {"time": 0.6, "value": 1, "easing": "easeOut"}] or [[0, 0], [0.6, 1, "easeOut"]]. A keyframe's easing shapes the move into it: linear (default), hold, ease, easeIn, easeOut, easeInOut, ease<In\|Out\|InOut><Sine\|Quad\|Cubic\|Quart\|Quint\|Expo\|Circ\|Back\|Elastic\|Bounce>, cubicBezier(x1,y1,x2,y2), spring(bounce 0-1). Empty removes the animation. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `motion.updateLayer`

Change some properties of one layer, object, light, the camera or the scene ("scene": background, ambient) of a motion clip. A property that is animated gets a keyframe at that time instead; others change for the whole clip. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `id` | string | required | A layer, object or light id, "camera" or "scene". |
| `props` | object | required | Properties and values, e.g. {"x": 120, "fill": "#ff5a36", "text": "Hi"} (any field of motion.guide; nested ones like stroke or material merge). |
| `time` | number |  | Timeline seconds, for animated properties (default: the playhead). |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `motion.addKeyframe`

Set one keyframe of a layer, object, light or camera property at a timeline time (replacing one already there). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `id` | string | required | A layer, object or light id, "camera" or "scene". |
| `property` | string | required | The property, e.g. x, opacity, rotation.y, fov. |
| `time` | number |  | Timeline seconds (default: the playhead). |
| `value` | any |  | The value (default: what the property is at that time). |
| `easing` | string |  | How the value arrives here from the previous keyframe (default linear). |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

### `motion.removeKeyframe`

Remove a keyframe of a layer, object, light or camera property at a timeline time, or its whole animation (it then keeps its value at the playhead). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `id` | string | required | A layer, object or light id, "camera" or "scene". |
| `property` | string | required | The animated property. |
| `time` | number |  | Timeline seconds; omit to remove the property's whole animation. |

### `motion.setTemplate`

Re-make a template clip with new values (the others keep theirs). Edits made to its scene by hand are replaced. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `values` | object | required | Values to change. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

## timeline

### `timeline.seek`

Move the playhead. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `time` | number | required | Timeline time in seconds. |

### `timeline.play`

Start playback from the playhead. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `speed` | number |  | 1 (default) plays with sound; 2 to 8 faster, -1 to -8 backwards, without sound (like L and J). |

### `timeline.pause`

Stop playback. _(changes things · needs the window)_

### `timeline.closeGap`

Close the empty space at a time on a track by pulling the later clips left. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `trackId` | string | required | Track id or unique name ("Video 1"), as listed by track.list. |
| `time` | number | required | A time inside the gap, in seconds. |

### `timeline.markers`

List markers by time. _(read only)_

### `timeline.addMarker`

Add a marker. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `time` | number |  | Seconds. Defaults to the playhead. |
| `label` | string |  | Label. |

### `timeline.removeMarker`

Remove a marker. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `markerId` | string | required | Marker id or unique label, as listed by timeline.markers. |

## history

### `history.list`

The undo and redo steps: which command made each one and who (window, agent, cli, mcp). _(read only)_

### `history.undo`

Undo the last step, whoever made it. Returns the command that made the step. _(changes things)_

### `history.redo`

Redo the last undone step. Returns the command that made the step. _(changes things)_

### `history.checkpoint`

Remember the project as it is now; history.revertTo puts it back. _(changes things)_

### `history.revertTo`

Put the project back as it was at a checkpoint, as one new undo step (so the revert can be undone too). _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `checkpoint` | integer | required | Id returned by history.checkpoint. |

## generate

### `generate.providers`

Image and video providers with whether each is ready (enabled, and has a key when it needs one). _(read only)_

### `generate.models`

Models of one provider, or of every ready provider, with what each can do (tasks, aspect ratios, durations, frames, sound). _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `provider` | string |  | Provider id; omit for every ready provider. |
| `task` | string |  | Only models that can do text_to_image, image_to_image, text_to_video or image_to_video. |
| `refresh` | boolean |  | Fetch the list again instead of using the cache. |

### `generate.check`

Check that a provider answers with the saved key or address. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `provider` | string | required | Provider id. |

### `generate.setKey`

Save (or with no key, remove) a provider's API key in the OS keychain. _(changes things · person only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `provider` | string | required | Provider id. |
| `key` | string |  | The key; omit to remove it. |

### `generate.setProvider`

Turn a provider on or off, or point it at another address. _(changes things · permission: settings)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `provider` | string | required | Provider id. |
| `enabled` | boolean |  | Show its models. |
| `baseUrl` | string |  | Server address (local providers and gateways). |
| `options` | object |  | Provider options, e.g. the ComfyUI workflows folder. |

### `generate.submit`

Generate an image or a video. By default a placeholder clip appears on the timeline and becomes the result when it is done; place "library" only adds it to the media. _(changes things · permission: generate)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `prompt` | string | required | What to make, in words. |
| `provider` | string |  | Provider id (generate.providers). With model, picks the model; defaults to settings.generate. |
| `model` | string |  | Model id from generate.models, or "provider::model". Defaults to the model set in settings.generate, else the first featured ready model. |
| `task` | string |  | text_to_image, image_to_image, text_to_video or image_to_video. Defaults from video and the images given. |
| `video` | boolean |  | Make a video rather than an image (when task is omitted). |
| `negativePrompt` | string |  | What to avoid, for models that take it. |
| `images` | array of objects |  | Input images: [{role: reference\|start_frame\|end_frame, path?, assetId?, clipId?, time?}]. A clip gives the frame it shows at time. |
| `aspectRatio` | string |  | "16:9", "9:16", "1:1"… Defaults to the project's. |
| `duration` | number |  | Video length in seconds (the model picks the closest it supports). |
| `resolution` | string |  | "720p", "1080p"… when the model offers several. |
| `seed` | integer |  | Seed, when the model takes one. |
| `count` | integer |  | How many results (images). |
| `audio` | boolean |  | Generate sound with the video when the model can. |
| `params` | object |  | Model-specific values (see the model's params in generate.models). |
| `place` | string |  | "timeline" (default) or "library". |
| `trackId` | string |  | Track id or name. Defaults to the first free compatible track (a new one if none is free). |
| `start` | number |  | Timeline position in seconds. Defaults to the playhead. |
| `length` | number |  | Placeholder length on the timeline in seconds (default: duration, or 5). |
| `wait` | boolean |  | Wait until the job finishes and return it (always true with --file). |

### `generate.animateFrame`

Turn the frame a clip shows at a time into a moving shot (image-to-video), placed right after the clip. _(changes things · permission: generate)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `time` | number |  | Timeline time of the frame. Defaults to the playhead, or the clip's first frame. |
| `prompt` | string |  | What to make, in words. |
| `provider` | string |  | Provider id (generate.providers). With model, picks the model; defaults to settings.generate. |
| `model` | string |  | Model id from generate.models, or "provider::model". Defaults to the model set in settings.generate, else the first featured ready model. |
| `duration` | number |  | Video length in seconds (the model picks the closest it supports). |
| `seed` | integer |  | Seed, when the model takes one. |
| `wait` | boolean |  | Wait until the job finishes and return it (always true with --file). |

### `generate.extendClip`

Continue a clip from its last frame; the new shot lands right after it on the same track. _(changes things · permission: generate)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `prompt` | string |  | What to make, in words. |
| `provider` | string |  | Provider id (generate.providers). With model, picks the model; defaults to settings.generate. |
| `model` | string |  | Model id from generate.models, or "provider::model". Defaults to the model set in settings.generate, else the first featured ready model. |
| `duration` | number |  | Video length in seconds (the model picks the closest it supports). |
| `seed` | integer |  | Seed, when the model takes one. |
| `wait` | boolean |  | Wait until the job finishes and return it (always true with --file). |

### `generate.bridge`

Generate a transition from the last frame of one clip to the first frame of another, filling the gap between them. _(changes things · permission: generate)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `fromClipId` | string | required | The clip before (id or name). |
| `toClipId` | string | required | The clip after (id or name). |
| `prompt` | string |  | What to make, in words. |
| `provider` | string |  | Provider id (generate.providers). With model, picks the model; defaults to settings.generate. |
| `model` | string |  | Model id from generate.models, or "provider::model". Defaults to the model set in settings.generate, else the first featured ready model. |
| `duration` | number |  | Video length in seconds (the model picks the closest it supports). |
| `seed` | integer |  | Seed, when the model takes one. |
| `wait` | boolean |  | Wait until the job finishes and return it (always true with --file). |

### `generate.restyleFrame`

Edit the frame a clip shows at a time with an image model; the still lands at that time. _(changes things · permission: generate)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `time` | number |  | Timeline time of the frame. Defaults to the playhead. |
| `prompt` | string | required | What to make, in words. |
| `provider` | string |  | Provider id (generate.providers). With model, picks the model; defaults to settings.generate. |
| `model` | string |  | Model id from generate.models, or "provider::model". Defaults to the model set in settings.generate, else the first featured ready model. |
| `seed` | integer |  | Seed, when the model takes one. |
| `wait` | boolean |  | Wait until the job finishes and return it (always true with --file). |

### `generate.regenerate`

Run a generated clip's request again (same prompt, model and settings); with variation, a new seed. The result lands after the clip. _(changes things · permission: generate)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipId` | string | required | Clip id or unique name, as listed by clip.list. |
| `variation` | boolean |  | Use a new seed (default false: same seed). |
| `prompt` | string |  | Change the prompt. |
| `wait` | boolean |  | Wait until the job finishes and return it (always true with --file). |

### `generate.jobs`

Generation jobs, newest first, with status, progress, outputs and errors. _(read only)_

### `generate.wait`

Wait for a job to finish and return it. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `jobId` | string | required | Job id from generate.jobs. |
| `timeout` | number |  | Give up after this many seconds (default 600). |

### `generate.cancel`

Cancel a running or queued job; its placeholder goes away. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `jobId` | string | required | Job id from generate.jobs. |

### `generate.clearFinished`

Remove finished, failed and cancelled jobs from the list. _(changes things)_

## export

### `export.formats`

Export formats, qualities and encoder choices. _(read only)_

### `export.encoders`

The video encoders this computer uses per format: hardware ones (Apple VideoToolbox, NVIDIA NVENC, AMD AMF, Intel Quick Sync, VA-API, Media Foundation) that passed a test encode, and the CPU ones. _(read only)_

### `export.start`

Render the open project to a file: every frame drawn as in the preview (titles, animation, motion graphics, 3D), encoded on the GPU or CPU with the mixed sound. Returns an export id; follow it with export.status, or pass wait. _(changes things · permission: files)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `path` | string | required | Destination file. The extension should match the format. |
| `format` | string |  | mp4 (default), hevc, prores, webm, gif, audio (AAC) or wav. |
| `quality` | string |  | draft, standard (default) or high. |
| `width` | integer |  | Output width (default: the project's). |
| `height` | integer |  | Output height (default: the project's). |
| `fps` | number |  | Output frame rate (default: the project's). |
| `from` | number |  | Start of the range in seconds (default 0). |
| `to` | number |  | End of the range in seconds (default: the end). |
| `encoder` | string |  | auto (default: the GPU or media engine when there is one, redone on the CPU if it fails), hardware (GPU only; WebM may be AV1) or software (CPU only: slower, smallest files). |
| `captions` | string |  | burn (default: in the picture), file (an .srt next to the video instead), both, or none. |
| `wait` | boolean |  | Wait until the job finishes and return it (always true with --file). |

### `export.status`

Exports with their progress, or one export. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `exportId` | string |  | One export. |

### `export.cancel`

Stop an export; nothing is left at the destination. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `exportId` | string | required | Export id. |

## handoff

### `handoff.apps`

Other lsuite apps installed on this computer (from ~/.lsuite/apps) and whether they are running. _(read only)_

### `handoff.toRyolune`

Send the cut to ryolune to score it: renders the audio (WAV) and writes its length and markers next to it; when ryolune is running, imports the audio there and adds the markers. _(changes things · permission: files)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `from` | number |  | Start of the range in seconds (default 0). |
| `to` | number |  | End of the range in seconds (default: the end). |
| `name` | string |  | Name for the hand-off files (default: the project name). |

### `handoff.fromRyolune`

Put audio from ryolune on an audio track: a file ryolune exported (path), or, when ryolune is running, a fresh bounce of its open song. _(changes things · permission: files)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `path` | string |  | An audio file from ryolune. Omit to ask the running ryolune for a bounce. |
| `trackId` | string |  | Track id or name. Defaults to the first free compatible track (a new one if none is free). |
| `start` | number |  | Seconds (default 0). |

## app

### `app.info`

Version, ffmpeg, library and data folders, whether the window and the bridge are running. _(read only)_

### `app.commands`

Describe every command with its parameters, or one command. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `command` | string |  | One command name. |

### `app.fonts`

Font families text clips can use: the bundled ones (Manrope, IBM Plex Mono, Instrument Sans, Instrument Serif) first, then this computer's. _(read only)_

### `app.settings`

Every setting with its value (agent permissions, updates, appearance, default models, diagnostics). _(read only)_

### `app.setSetting`

Change one setting by dotted key, e.g. updates.checkOnStart or appearance.mode. Agent permissions stay with the person. _(changes things · permission: settings)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `key` | string | required | Dotted key from app.settings. |
| `value` | any | required | New value, of the same type. |

### `app.setAgentKey`

Save (or with no key, remove) the API key the built-in agent uses, in the OS keychain. _(changes things · person only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `provider` | string | required | "anthropic" or "openai". |
| `key` | string |  | The key; omit to remove it. |

### `app.checkUpdates`

Check GitHub Releases for a newer kimchi and report it. _(read only)_

### `app.installUpdate`

Download, verify (signature) and install the update found by app.checkUpdates; kimchi restarts into it. _(changes things · permission: app control)_

### `app.restart`

Quit and start kimchi again (into an installed update, when there is one). _(changes things · permission: app control · needs the window)_

### `app.whatsNew`

Release notes: what changed in this version, in another (version), or in every version since one (since). Markdown, newest first. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `version` | string |  | One version, e.g. 0.5.0. Defaults to this one. |
| `since` | string |  | Every release newer than this version, up to this one. |
| `all` | boolean |  | Every release. |

### `app.diagnostics`

What a bug report needs: version, system, ffmpeg, 3D renderer, folders, log level, the log file and recent crash reports. Contains no keys or project content. _(read only)_

### `app.logs`

kimchi's log files and the last lines of this run's log (or of another file it lists). _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `lines` | integer |  | How many lines (default 100, up to 2000). |
| `file` | string |  | A log file name from the list, e.g. kimchi.1.log (the previous run). |

### `app.crashReports`

Crash reports, newest first: panics kimchi caught and runs that ended without quitting. With id, one report's text. _(read only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `id` | string |  | A report's id from the list. |

### `app.clearCrashReports`

Delete every crash report. _(changes things · permission: files)_

### `app.quit`

Quit kimchi. _(changes things · permission: app control · needs the window)_

### `app.notify`

Show a short message in the window. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `text` | string | required | Message. |
| `kind` | string |  | info (default), success or error. |

## agent

### `agent.providers`

The models that can run the built-in agent (Claude Code, Codex, the Anthropic and OpenAI APIs, Ollama), whether each is ready on this computer and why not, and which one is chosen. _(read only · needs the window)_

### `agent.setProvider`

Choose what runs the built-in agent (Settings › Agent). _(changes things · person only)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `provider` | string | required | claude-code, codex, anthropic, openai or ollama. |
| `model` | string |  | Model id for the API providers and Ollama; empty for the provider's default. |
| `baseUrl` | string |  | Server address for Ollama or an OpenAI-compatible server; empty for the default. |

### `agent.send`

Ask the built-in agent (the Agent panel) to do something, in words. It continues the panel's conversation, runs commands like any client (permissions apply) and shows them as cards. Returns the run at once, or once it ends with wait. One run at a time. Uses the person's model account, so agents need the generate permission. _(changes things · permission: generate · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `prompt` | string | required | The request, e.g. "Add a title saying Hello at 0 s and fade it in". |
| `wait` | boolean |  | Wait until the run ends and return it with its reply and commands (default false). |
| `timeout` | number |  | With wait: stop waiting after this many seconds (default 900); the run goes on. |

### `agent.status`

One agent run (the running or latest one by default): the request, whether it is still working and on what, the reply, every command it ran with its outcome, changes, time and tokens. _(read only · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `run` | integer |  | Run id from agent.runs. |
| `wait` | boolean |  | Wait until the run ends. |
| `timeout` | number |  | With wait: stop waiting after this many seconds (default 900). |

### `agent.runs`

The agent's runs on the open project, oldest first: request, provider, outcome, changes and whether agent.revert can undo them. _(read only · needs the window)_

### `agent.conversation`

The Agent panel's conversation as it shows it: requests, replies, one card per command (the agent's, and those of MCP clients and the CLI) and how each run ended. _(read only · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `since` | integer |  | Only entries from this index on (each answer gives the next index). |

### `agent.stop`

Stop the agent's run. Edits it finished stay (agent.revert removes them). _(changes things · needs the window)_

### `agent.revert`

Revert an agent run: the project goes back to how it was before the run's first change, as one undo step (history.undo brings the run back). _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `run` | integer |  | Run id (default: the latest run that changed something and isn't reverted). |

### `agent.newConversation`

Start a new conversation in the Agent panel: the agent forgets the thread. Earlier runs can still be reverted. _(changes things · needs the window)_

## ui

### `ui.state`

What the window shows: home or editor, playhead, playing, selection, zoom, open panel and dialogs, theme. _(read only)_

### `ui.select`

Select clips (or one media item) in the window. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array of strings |  | Clips to select (ids or names); empty clears. |
| `assetId` | string |  | A media item to select instead. |

### `ui.showPanel`

Open a panel or dialog: media, generate, text, motion, captions (left panel), agent, jobs, settings, export, palette, shortcuts, whatsNew, diagnostics; or home. With open false, close it. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `panel` | string | required | Panel name. |
| `section` | string |  | For settings: models, agent, appearance, updates, diagnostics or about. |
| `open` | boolean |  | false closes the panel or dialog instead (agent, jobs or a dialog; default true). |
| `all` | boolean |  | For whatsNew: the notes of every release, not only this one's. |

### `ui.closeDialogs`

Close open dialogs and popovers. _(changes things · needs the window)_

### `ui.zoom`

Zoom the timeline. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `pixelsPerSecond` | number |  | 4-600. |
| `fit` | boolean |  | Fit the whole project in view. |

### `ui.setTimeline`

Timeline and playback options in the window: snapping, ripple delete and loop. Only the given ones change; returns all three. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `snapping` | boolean |  | Dragged clips, edges and the playhead stick to cuts, markers and the playhead (N). |
| `ripple` | boolean |  | Deleting in the window closes the gap, as clip.delete ripple does. |
| `loop` | boolean |  | Playback starts over at the end. |

### `ui.setLayout`

Resize the editor's panels, in pixels (each within its limits), or put them back as they start. Returns the sizes. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `left` | number |  | Width of the left panel (280-520). |
| `inspector` | number |  | Width of the inspector, on the right (260-440). |
| `timeline` | number |  | Height of the timeline (180-620). |
| `agent` | number |  | Width of the Agent panel (300-560). |
| `reset` | boolean |  | Back to the starting sizes first. |

### `ui.action`

Do what a keyboard shortcut or menu item of the window does, by its action name. It acts on the window's selection, playhead and clipboard as the key would, a moment after the answer. Agents need the permission of what it does (NewProject: projects, ToggleTheme: settings, Quit: app control…). _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `action` | string | required | PlayPause, ShuttleBack, ShuttleStop, ShuttleForward, ToggleLoop, StepBack, StepForward, StepBackSecond, StepForwardSecond, PrevEdit, NextEdit, GoToStart, GoToEnd, Undo, Redo, CopyClips, CutClips, PasteClips, Duplicate, Split, TrimStart, TrimEnd, NudgeLeft, NudgeRight, NudgeLeftMore, NudgeRightMore, Delete, RippleDelete, SelectAll, Deselect, AddText, AddMarker, ToggleSnap, ZoomIn, ZoomOut, ZoomFit, Palette, FocusGenerate, ShowMedia, ShowGenerate, ShowText, ShowMotion, ShowCaptions, ToggleAgent, ToggleJobs, ShowShortcuts, OpenSettings, WhatsNew, ShowDiagnostics, About, Save, CheckUpdates, OpenHelp, OpenSupport, ReportProblem, Import, Export, NewProject, CloseProject, ToggleTheme, RestartApp or Quit. |

### `ui.reveal`

Show a file in the file manager (Finder, Explorer…): a path, or a media item's file. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `path` | string |  | A file or folder (an export, a log folder…). |
| `assetId` | string |  | A media item (id or unique name) instead. |

### `ui.screenshot`

Save a PNG of the window and return its path. _(changes things · permission: files · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `path` | string |  | Destination .png (default: a temporary file). |
