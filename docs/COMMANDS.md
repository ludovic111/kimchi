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
| `commands` | array | required | Array of {"command": "clip.update", "params": {…}}. |
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
| `paths` | array | required | Absolute paths of the files to import. |
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

Rename, mute, hide or lock a track. One undo step. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `trackId` | string | required | Track id or unique name ("Video 1"), as listed by track.list. |
| `name` | string |  | New name. |
| `muted` | boolean |  | Silence the track. |
| `hidden` | boolean |  | Hide the track's pictures. |
| `locked` | boolean |  | Protect the track from edits. |

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
| `moves` | array | required | Array of {clipId, start, trackId?}. |
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
| `clipIds` | array |  | Clips to split (ids or names). |

### `clip.delete`

Delete clips. With ripple, later clips on the same track move left to close the gap. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array | required | Clips to delete (ids or names). |
| `ripple` | boolean |  | Close the gap (default false). |

### `clip.duplicate`

Copy clips to the end of their track. _(changes things)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array | required | Clips to duplicate (ids or names). |

### `clip.update`

Change a clip: name, position, scale, rotation, opacity, fit, volume, fades, speed, text style or solid colour. Only the given fields change. One undo step. _(changes things)_

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
| `speed` | number |  | 0.1-16; the clip gets shorter or longer on the timeline. |
| `style` | object |  | Text clips: style fields to change (see clip.addText). |
| `color` | string |  | Solid clips: colour #rrggbb. |
| `coalesce` | string |  | Edits with the same key within ~1 s fold into one undo step (drags, sliders). |

## timeline

### `timeline.seek`

Move the playhead. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `time` | number | required | Timeline time in seconds. |

### `timeline.play`

Start playback from the playhead. _(changes things · needs the window)_

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

Undo the last step, whoever made it. _(changes things)_

### `history.redo`

Redo the last undone step. _(changes things)_

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
| `images` | array |  | Input images: [{role: reference\|start_frame\|end_frame, path?, assetId?, clipId?, time?}]. A clip gives the frame it shows at time. |
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

Render the open project to a file through one ffmpeg graph (text is drawn the same as in the preview), encoded on the GPU or media engine when there is one. Returns an export id; follow it with export.status, or pass wait. _(changes things · permission: files)_

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

Every setting with its value (agent permissions, updates, appearance, default models). _(read only)_

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

### `app.quit`

Quit kimchi. _(changes things · permission: app control · needs the window)_

### `app.notify`

Show a short message in the window. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `text` | string | required | Message. |
| `kind` | string |  | info (default), success or error. |

## ui

### `ui.state`

What the window shows: home or editor, playhead, playing, selection, zoom, open panel and dialogs, theme. _(read only)_

### `ui.select`

Select clips (or one media item) in the window. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `clipIds` | array |  | Clips to select (ids or names); empty clears. |
| `assetId` | string |  | A media item to select instead. |

### `ui.showPanel`

Open a panel or dialog: media, generate, text (left panel), agent, jobs, settings, export, palette; or home. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `panel` | string | required | Panel name. |
| `section` | string |  | For settings: models, agent, appearance, updates or about. |

### `ui.closeDialogs`

Close open dialogs and popovers. _(changes things · needs the window)_

### `ui.zoom`

Zoom the timeline. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `pixelsPerSecond` | number |  | 4-600. |
| `fit` | boolean |  | Fit the whole project in view. |

### `ui.screenshot`

Save a PNG of the window and return its path. _(changes things · needs the window)_

| Parameter | Type | | Description |
| --- | --- | --- | --- |
| `path` | string |  | Destination .png (default: a temporary file). |
