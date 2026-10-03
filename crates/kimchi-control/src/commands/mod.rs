//! Every command's spec and handler. Specs are listed here in one table so the
//! docs, the CLI help and the MCP tools are generated in a stable order.

pub mod app;
pub mod clip;
pub mod export;
pub mod generate;
pub mod handoff;
pub mod history;
pub mod media;
pub mod motion;
pub mod project;
pub mod timeline;
pub mod track;
pub mod transition;
pub mod ui;

use std::sync::Arc;

use crate::registry::Kind::*;
use crate::registry::{Args, Ctx, Perm, Spec, edit, opt, query, req};
use crate::session::{CmdResult, Session};

const CLIP_ID: crate::registry::Param = req("clipId", String, "Clip id or unique name, as listed by clip.list.");
const TRACK_ID: crate::registry::Param = req("trackId", String, "Track id or unique name (\"Video 1\"), as listed by track.list.");
const ASSET_ID: crate::registry::Param = req("assetId", String, "Media id or unique name, as listed by media.list.");
const PROJECT_ID: crate::registry::Param = req("projectId", String, "Project id or unique name, as listed by project.list.");
const OPT_TRACK: crate::registry::Param = opt("trackId", String, "Track id or name. Defaults to the first free compatible track (a new one if none is free).");
const START: crate::registry::Param = opt("start", Number, "Timeline position in seconds. Defaults to the playhead.");
const KEYFRAMES: crate::registry::Param = req(
    "keyframes",
    Array,
    "[{\"time\": 0, \"value\": 0}, {\"time\": 0.6, \"value\": 1, \"easing\": \"easeOut\"}] or [[0, 0], [0.6, 1, \"easeOut\"]]. A keyframe's easing shapes the move into it: linear (default), hold, ease, easeIn, easeOut, easeInOut, ease<In|Out|InOut><Sine|Quad|Cubic|Quart|Quint|Expo|Circ|Back|Elastic|Bounce>, cubicBezier(x1,y1,x2,y2), spring(bounce 0-1). Empty removes the animation.",
);

// Generation parameters shared by the AI editing commands.
const PROVIDER: crate::registry::Param = opt("provider", String, "Provider id (generate.providers). With model, picks the model; defaults to settings.generate.");
const MODEL: crate::registry::Param = opt("model", String, "Model id from generate.models, or \"provider::model\". Defaults to the model set in settings.generate, else the first featured ready model.");
const PROMPT: crate::registry::Param = req("prompt", String, "What to make, in words.");
const OPT_PROMPT: crate::registry::Param = opt("prompt", String, "What to make, in words.");
const DURATION: crate::registry::Param = opt("duration", Number, "Video length in seconds (the model picks the closest it supports).");
const SEED: crate::registry::Param = opt("seed", Integer, "Seed, when the model takes one.");
const WAIT: crate::registry::Param = opt("wait", Boolean, "Wait until the job finishes and return it (always true with --file).");

pub static SPECS: &[Spec] = &[
    // ---- project ----------------------------------------------------------
    query("project.list", "List the projects in the library, most recent first, with their length, size and whether one is open.", &[]),
    query("project.overview", "The whole open project in one bounded answer: settings, every track with its clips (times, media, text, transforms that differ from the defaults), media with generation provenance, markers, running jobs, undo history, what the window shows, and problems (missing files, placeholders still generating, hidden or muted tracks). Read it first.", &[]),
    query("project.get", "The complete open project as JSON (the project file format).", &[]),
    query("project.renderFrame", "Render what the timeline shows at a time (or a labelled contact sheet of several times) to a PNG and return its path, to look at a result: animations, motion graphics, 3D, the whole cut.", &[
        opt("time", Number, "Timeline seconds (default: the playhead)."),
        opt("times", Array, "Several times in seconds: one image with a frame per time, labelled (up to 16)."),
        opt("width", Integer, "Width of each frame in pixels (default 960, or 480 in a sheet)."),
    ]),
    edit("project.create", "Create a project in the library and open it, replacing the open one.", &[
        opt("name", String, "Project name (default \"Untitled\")."),
        opt("width", Integer, "Canvas width in pixels (default 1920)."),
        opt("height", Integer, "Canvas height in pixels (default 1080)."),
        opt("fps", Number, "Frames per second (default 30)."),
        opt("background", String, "Canvas colour #rrggbb (default #000000)."),
    ]).perm(Perm::Projects),
    edit("project.open", "Open a library project (projectId) or a project file (path), replacing the open one.", &[
        opt("projectId", String, "Project id or unique name, as listed by project.list."),
        opt("path", String, "A project .json file to open in place; every change is saved back to it."),
    ]).perm(Perm::Projects),
    edit("project.close", "Close the open project and go back to the home screen.", &[]).perm(Perm::Projects),
    edit("project.delete", "Delete a library project and its generated media and caches. Cannot be undone.", &[PROJECT_ID]).perm(Perm::Projects),
    edit("project.duplicate", "Copy a library project (the open one by default) as \"<name> copy\".", &[opt("projectId", String, "Project id or unique name; defaults to the open project.")]).perm(Perm::Projects),
    edit("project.rename", "Rename the open project (one undo step), or another library project.", &[req("name", String, "New name."), opt("projectId", String, "A library project (id or unique name); defaults to the open one."), crate::registry::COALESCE]),
    edit("project.setSettings", "Change the canvas: size, frame rate, background colour, sample rate. One undo step.", &[
        opt("width", Integer, "Width in pixels (16 or more)."),
        opt("height", Integer, "Height in pixels (16 or more)."),
        opt("fps", Number, "Frames per second."),
        opt("background", String, "Canvas colour #rrggbb."),
        opt("sampleRate", Integer, "Audio sample rate for exports, e.g. 48000."),
    ]),
    edit("project.saveAs", "Write a copy of the open project to a .json file (the project file format).", &[req("path", String, "Destination .json file.")]).perm(Perm::Files),
    edit("project.batch", "Run several commands as one undo step. With atomic (the default) a failing command rolls back the ones before it.", &[
        req("commands", Array, "Array of {\"command\": \"clip.update\", \"params\": {…}}."),
        opt("atomic", Boolean, "Roll everything back if one command fails (default true)."),
        opt("label", String, "Name of the undo step (default \"batch\")."),
    ]),
    // ---- media ------------------------------------------------------------
    query("media.list", "List the open project's media (imported and generated) with kind, length, size, previews and generation details.", &[]),
    query("media.get", "One media item in full, including how it was generated (prompt, model, seed, inputs).", &[ASSET_ID]),
    edit("media.import", "Import media files (video, image, audio) into the open project. Thumbnails, filmstrips, waveforms and proxies are made in the background. With place, each file is also put on the timeline, one after the other.", &[
        req("paths", Array, "Absolute paths of the files to import."),
        opt("place", Boolean, "Also put each file on the timeline (default false)."),
        OPT_TRACK,
        START,
    ]).perm(Perm::Files),
    edit("media.remove", "Remove a media item and every clip that uses it. One undo step.", &[ASSET_ID]),
    edit("media.frame", "Save the frame a clip shows at a timeline time as a PNG and return its path (for image-to-video and references).", &[CLIP_ID, opt("time", Number, "Timeline time in seconds inside the clip. Defaults to the playhead, or the clip's first frame.")]),
    // ---- track ------------------------------------------------------------
    query("track.list", "List tracks from top to bottom with their kind, flags and clip count. Track 0 is drawn on top.", &[]),
    edit("track.add", "Add a track. New video tracks go on top, new audio tracks at the bottom.", &[
        req("kind", String, "\"video\" (pictures: video, images, text, solids) or \"audio\"."),
        opt("index", Integer, "Position from the top (0 = top)."),
    ]),
    edit("track.remove", "Delete a track and every clip on it. One undo step.", &[TRACK_ID]),
    edit("track.update", "Rename, mute, hide or lock a track. One undo step.", &[
        TRACK_ID,
        opt("name", String, "New name."),
        opt("muted", Boolean, "Silence the track."),
        opt("hidden", Boolean, "Hide the track's pictures."),
        opt("locked", Boolean, "Protect the track from edits."),
    ]),
    edit("track.move", "Move a track to another position (0 = top).", &[TRACK_ID, req("index", Integer, "Zero-based target position from the top.")]),
    // ---- clip -------------------------------------------------------------
    query("clip.list", "List clips with their track, start, end, kind and media.", &[opt("trackId", String, "Only clips on this track.")]),
    query("clip.get", "One clip in full: timing, source in-point, speed, transform, fades, volume, text style.", &[CLIP_ID]),
    edit("clip.insertMedia", "Put a media item on the timeline at its full length.", &[ASSET_ID, OPT_TRACK, START]),
    edit("clip.addText", "Add a title. Text is drawn the same in the preview and the export.", &[
        req("text", String, "The words; \\n starts a new line."),
        START,
        opt("duration", Number, "Seconds on screen (default 4)."),
        OPT_TRACK,
        opt("style", Object, "Text style fields to change: fontFamily, fontSize (project pixels, default 120), fontWeight (100-900), italic, color (#rrggbb), background (#rrggbb or null), align (left|center|right), lineHeight, letterSpacing, shadow."),
        opt("x", Number, "Horizontal offset of the centre from the canvas centre, in project pixels."),
        opt("y", Number, "Vertical offset of the centre from the canvas centre, in project pixels (positive is down)."),
    ]),
    edit("clip.addSolid", "Add a solid colour clip (a background, a flash, a fade card).", &[
        req("color", String, "Colour #rrggbb."),
        START,
        opt("duration", Number, "Seconds (default 5)."),
        OPT_TRACK,
    ]),
    edit("clip.move", "Move a clip to another time and/or track of the same kind. Whatever it lands on is overwritten.", &[
        CLIP_ID,
        opt("start", Number, "New start in seconds."),
        opt("trackId", String, "Destination track."),
        crate::registry::COALESCE,
    ]),
    edit("clip.moveMany", "Move several clips at once, as one undo step.", &[req("moves", Array, "Array of {clipId, start, trackId?}."), crate::registry::COALESCE]),
    edit("clip.trim", "Move one edge of a clip to a timeline time, like dragging it. Bounded by the neighbours and the source length.", &[
        CLIP_ID,
        req("edge", String, "\"start\" or \"end\"."),
        req("time", Number, "Timeline time in seconds for that edge."),
        crate::registry::COALESCE,
    ]),
    edit("clip.split", "Split clips at a time. Without clipIds, every clip under that time on unlocked tracks (or the selection in the window).", &[
        opt("time", Number, "Timeline time in seconds. Defaults to the playhead."),
        opt("clipIds", Array, "Clips to split (ids or names)."),
    ]),
    edit("clip.delete", "Delete clips. With ripple, later clips on the same track move left to close the gap.", &[
        req("clipIds", Array, "Clips to delete (ids or names)."),
        opt("ripple", Boolean, "Close the gap (default false)."),
    ]),
    edit("clip.duplicate", "Copy clips to the end of their track.", &[req("clipIds", Array, "Clips to duplicate (ids or names).")]),
    edit("clip.paste", "Paste copies of clips: the earliest copy starts at time and the others keep their spacing and tracks. Whatever they land on is overwritten. One undo step.", &[
        opt("clipIds", Array, "Clips in the project to copy (ids or names)."),
        opt("clips", Array, "Clip objects as returned by clip.get (with trackId), e.g. clips deleted since (a cut)."),
        opt("time", Number, "Where the earliest copy starts, in seconds. Defaults to the playhead."),
        opt("trackId", String, "Put every copy on this track instead of each clip's own."),
    ]),
    edit("clip.update", "Change a clip: name, position, scale, rotation, opacity, fit, volume, fades, speed, reverse, text style or solid colour. Only the given fields change. One undo step.", &[
        CLIP_ID,
        opt("name", String, "Clip name."),
        opt("x", Number, "Offset of the centre from the canvas centre, project pixels."),
        opt("y", Number, "Offset of the centre from the canvas centre, project pixels (positive is down)."),
        opt("scale", Number, "1 = fitted to the canvas."),
        opt("rotation", Number, "Degrees, clockwise."),
        opt("opacity", Number, "0-1."),
        opt("fit", String, "\"contain\", \"cover\" or \"stretch\"."),
        opt("volume", Number, "0-4 (1 = unchanged)."),
        opt("fadeIn", Number, "Fade-in length in seconds."),
        opt("fadeOut", Number, "Fade-out length in seconds."),
        opt("speed", Number, "0.1-16; the clip gets shorter or longer on the timeline. The sound keeps its pitch."),
        opt("reverse", Boolean, "Video and sound clips: play the same part of the media backwards."),
        opt("style", Object, "Text clips: style fields to change (see clip.addText)."),
        opt("color", String, "Solid clips: colour #rrggbb."),
        crate::registry::COALESCE,
    ]),
    edit("clip.setKeyframes", "Animate one property of a clip: replace its keyframes (times in seconds from the clip's start). Properties: x, y, position ([x, y]), scale, scaleX, scaleY, rotation, opacity, blur (pixels), volume, the effects brightness, contrast, saturation, temperature, tint, vignette, sharpen (see clip.setEffects); text clips also fontSize, color, letterSpacing. One undo step.", &[
        CLIP_ID,
        req("property", String, "The property to animate."),
        KEYFRAMES,
        crate::registry::COALESCE,
    ]),
    edit("clip.addKeyframe", "Set one keyframe of a clip property at a timeline time, replacing one already there (what the window's keyframe buttons do).", &[
        CLIP_ID,
        req("property", String, "x, y, scale, scaleX, scaleY, rotation, opacity, blur, volume, brightness, contrast, saturation, temperature, tint, vignette, sharpen, fontSize, color or letterSpacing."),
        opt("time", Number, "Timeline seconds (default: the playhead)."),
        opt("value", Any, "The value (default: what the property is at that time)."),
        opt("easing", String, "How the value arrives here from the previous keyframe (default linear)."),
        crate::registry::COALESCE,
    ]),
    edit("clip.removeKeyframe", "Remove a clip's keyframe at a timeline time, or every keyframe of a property (it then keeps its value at that time, or its own).", &[
        CLIP_ID,
        req("property", String, "The animated property."),
        opt("time", Number, "Timeline seconds; omit to remove the property's whole animation."),
    ]),
    edit("clip.animate", "Give clips a ready-made animation written as ordinary keyframes: entrances (fadeIn, riseIn, slideInLeft, popIn, zoomIn, spinIn, dropIn, blurIn…), exits (fadeOut, slideOutRight, popOut…) or over the whole clip (kenBurns, panLeft, pulse, float, wiggle, shake, spin). motion.presets lists them all. One undo step.", &[
        req("clipIds", Array, "Clips to animate (ids or names)."),
        req("preset", String, "Preset name."),
        opt("length", Number, "Seconds the move takes (default 0.6; one cycle for repeating ones)."),
    ]),
    edit("clip.setEffects", "Colour and picture effects on clips: a ready-made look, corrections (brightness, contrast, saturation, temperature, tint), vignette, sharpen, a chroma key (green or blue screen) and a .cube LUT. Drawn in the preview and the export. Only the given fields change; animate the numeric ones with clip.setKeyframes. One undo step.", &[
        req("clipIds", Array, "Clips to change (ids or names)."),
        opt("look", String, "Start from a look (clip.looks): none, punchy, warm, cool, mono, faded, vintage, noir, teal, dreamy. The other fields given go on top."),
        opt("brightness", Number, "-1 to 1 (0 = unchanged)."),
        opt("contrast", Number, "-1 (flat grey) to 1 (twice the contrast)."),
        opt("saturation", Number, "-1 (black and white) to 1 (twice as colourful)."),
        opt("temperature", Number, "-1 (cooler, blue) to 1 (warmer, orange)."),
        opt("tint", Number, "-1 (greener) to 1 (more magenta)."),
        opt("vignette", Number, "0-1: darker corners."),
        opt("sharpen", Number, "0-1."),
        opt("chromaKey", Any, "Key a colour out: true (a green screen), a colour #rrggbb (the screen's colour, best picked from the footage), {color, similarity, softness, spill} (0-1 each; similarity 0.5, softness 0.1, spill 0.5 by default), or false to remove it."),
        opt("lut", Any, "Absolute path of a 3D .cube LUT, {path, strength}, or null to remove it."),
        opt("lutStrength", Number, "0-1: how much of the LUT shows (default 1)."),
        opt("reset", Boolean, "Remove every effect first."),
        crate::registry::COALESCE,
    ]),
    query("clip.looks", "The ready-made looks clip.setEffects applies, with their values.", &[]),
    edit("clip.freezeFrame", "Hold the frame a clip shows at a time: the clip is split there and a still of that frame plays for the duration, pushing the rest of its track later. The still keeps the clip's position, size and effects. One undo step.", &[
        CLIP_ID,
        opt("time", Number, "Timeline time inside the clip (default: the playhead)."),
        opt("duration", Number, "Seconds to hold the frame (default 2)."),
    ]),
    // ---- transition -------------------------------------------------------
    query("transition.kinds", "The transitions kimchi draws, with what each looks like.", &[]),
    query("transition.list", "Every transition in the project: the clip it leads into, the clip it leaves (on a cut), kind, length and where it plays.", &[]),
    edit("transition.set", "Put a transition at the start of clips. On a cut (the clip before ends where this one starts) it is centred on the cut and both clips play on past it with their media beyond the cut (or hold their edge frame), so nothing moves on the timeline; with no clip right before, the clip transitions in over what is below it. The sound crossfades over the same span. Changes the kind or length of transitions already there. One undo step.", &[
        opt("clipIds", Array, "The incoming clips (ids or names): each gets a transition at its start."),
        opt("trackId", String, "Instead of clipIds: every cut on this track."),
        opt("kind", String, "dissolve (default), dipToBlack, dipToWhite, wipeLeft, wipeRight, wipeUp, wipeDown, slideLeft, slideRight, slideUp, slideDown, pushLeft, pushRight, pushUp, pushDown, zoom, iris or blur."),
        opt("duration", Number, "Seconds (default 0.8). On a cut it can't be longer than the shorter clip; otherwise than half the clip."),
        opt("easing", String, "How the progress moves (default easeInOutSine; any keyframe easing)."),
        crate::registry::COALESCE,
    ]),
    edit("transition.remove", "Remove the transitions at the start of clips (or every one on a track). One undo step.", &[
        opt("clipIds", Array, "Clips whose transition goes (ids or names)."),
        opt("trackId", String, "Instead of clipIds: every transition on this track."),
    ]),
    // ---- motion -----------------------------------------------------------
    query("motion.guide", "How to make motion graphics and 3D with kimchi: the scene formats (2D layers, 3D objects, camera, lights), every property, keyframes and easings, text reveals, masks, effects, templates and presets, with examples. Read it before writing a scene.", &[
        opt("topic", String, "2d, 3d, keyframes, templates or all (default)."),
    ]),
    query("motion.templates", "Motion templates (lower third, title card, kinetic type, counter, bar chart, logo reveal, callout, quote, subscribe, aurora, wipe, 3D title, 3D logo spin, turntable, floating shapes) with the values each takes.", &[]),
    query("motion.presets", "The ready-made clip animations clip.animate applies.", &[]),
    edit("motion.add", "Add a motion clip: 2D motion graphics (layers of shapes, paths, text, images) or a 3D scene (camera, lights, objects, extruded text, glTF models), drawn by kimchi in the preview and the export, every property animatable. Look at the result with project.renderFrame.", &[
        req("scene", Object, "The scene, as described by motion.guide."),
        START,
        opt("duration", Number, "Seconds on the timeline (default: the last keyframe + 1 s, at least 3)."),
        OPT_TRACK,
        opt("name", String, "Clip name (default: from the scene)."),
    ]),
    edit("motion.addTemplate", "Add a motion clip made from a template with your values (see motion.templates). The clip remembers them: motion.setTemplate changes them later.", &[
        req("template", String, "Template id, e.g. lowerThird."),
        opt("values", Object, "Template values to change, e.g. {\"title\": \"Grace Hopper\"}."),
        START,
        opt("duration", Number, "Seconds (default: the template's)."),
        OPT_TRACK,
    ]),
    query("motion.get", "A motion clip's scene as JSON, or one layer, object or light of it.", &[CLIP_ID, opt("id", String, "A layer, object or light id, or \"camera\".")]),
    edit("motion.update", "Replace a motion clip's whole scene. One undo step.", &[CLIP_ID, req("scene", Object, "The new scene."), crate::registry::COALESCE]),
    edit("motion.setLayer", "Add a layer (2D) or an object or light (3D) to a motion clip, or replace the one with the same id. New 2D layers go on top.", &[
        CLIP_ID,
        req("layer", Object, "The layer, object or light (with its id)."),
        opt("parent", String, "Put it inside this group (2D) or object (3D)."),
        crate::registry::COALESCE,
    ]),
    edit("motion.removeLayer", "Remove a layer, object or light from a motion clip.", &[CLIP_ID, req("id", String, "Its id.")]),
    edit("motion.setKeyframes", "Animate one property of a layer, object, light or the camera inside a motion clip: replace its keyframes (times in scene seconds).", &[
        CLIP_ID,
        req("id", String, "A layer, object or light id, \"camera\", or \"scene\" (background, ambient)."),
        req("property", String, "The property, e.g. x, opacity, trimEnd, reveal, rotation.y, position, fov."),
        KEYFRAMES,
        crate::registry::COALESCE,
    ]),
    edit("motion.updateLayer", "Change some properties of one layer, object, light, the camera or the scene (\"scene\": background, ambient) of a motion clip. A property that is animated gets a keyframe at that time instead; others change for the whole clip.", &[
        CLIP_ID,
        req("id", String, "A layer, object or light id, \"camera\" or \"scene\"."),
        req("props", Object, "Properties and values, e.g. {\"x\": 120, \"fill\": \"#ff5a36\", \"text\": \"Hi\"} (any field of motion.guide; nested ones like stroke or material merge)."),
        opt("time", Number, "Timeline seconds, for animated properties (default: the playhead)."),
        crate::registry::COALESCE,
    ]),
    edit("motion.addKeyframe", "Set one keyframe of a layer, object, light or camera property at a timeline time (replacing one already there).", &[
        CLIP_ID,
        req("id", String, "A layer, object or light id, \"camera\" or \"scene\"."),
        req("property", String, "The property, e.g. x, opacity, rotation.y, fov."),
        opt("time", Number, "Timeline seconds (default: the playhead)."),
        opt("value", Any, "The value (default: what the property is at that time)."),
        opt("easing", String, "How the value arrives here from the previous keyframe (default linear)."),
        crate::registry::COALESCE,
    ]),
    edit("motion.removeKeyframe", "Remove a keyframe of a layer, object, light or camera property at a timeline time, or its whole animation (it then keeps its value at the playhead).", &[
        CLIP_ID,
        req("id", String, "A layer, object or light id, \"camera\" or \"scene\"."),
        req("property", String, "The animated property."),
        opt("time", Number, "Timeline seconds; omit to remove the property's whole animation."),
    ]),
    edit("motion.setTemplate", "Re-make a template clip with new values (the others keep theirs). Edits made to its scene by hand are replaced.", &[
        CLIP_ID,
        req("values", Object, "Values to change."),
        crate::registry::COALESCE,
    ]),
    // ---- timeline ---------------------------------------------------------
    edit("timeline.seek", "Move the playhead.", &[req("time", Number, "Timeline time in seconds.")]).window(),
    edit("timeline.play", "Start playback from the playhead.", &[]).window(),
    edit("timeline.pause", "Stop playback.", &[]).window(),
    edit("timeline.closeGap", "Close the empty space at a time on a track by pulling the later clips left.", &[TRACK_ID, req("time", Number, "A time inside the gap, in seconds.")]),
    query("timeline.markers", "List markers by time.", &[]),
    edit("timeline.addMarker", "Add a marker.", &[opt("time", Number, "Seconds. Defaults to the playhead."), opt("label", String, "Label.")]),
    edit("timeline.removeMarker", "Remove a marker.", &[req("markerId", String, "Marker id or unique label, as listed by timeline.markers.")]),
    // ---- history ----------------------------------------------------------
    query("history.list", "The undo and redo steps: which command made each one and who (window, agent, cli, mcp).", &[]),
    edit("history.undo", "Undo the last step, whoever made it. Returns the command that made the step.", &[]),
    edit("history.redo", "Redo the last undone step. Returns the command that made the step.", &[]),
    edit("history.checkpoint", "Remember the project as it is now; history.revertTo puts it back.", &[]),
    edit("history.revertTo", "Put the project back as it was at a checkpoint, as one new undo step (so the revert can be undone too).", &[req("checkpoint", Integer, "Id returned by history.checkpoint.")]),
    // ---- generate ---------------------------------------------------------
    query("generate.providers", "Image and video providers with whether each is ready (enabled, and has a key when it needs one).", &[]),
    query("generate.models", "Models of one provider, or of every ready provider, with what each can do (tasks, aspect ratios, durations, frames, sound).", &[
        opt("provider", String, "Provider id; omit for every ready provider."),
        opt("task", String, "Only models that can do text_to_image, image_to_image, text_to_video or image_to_video."),
        opt("refresh", Boolean, "Fetch the list again instead of using the cache."),
    ]),
    query("generate.check", "Check that a provider answers with the saved key or address.", &[req("provider", String, "Provider id.")]),
    edit("generate.setKey", "Save (or with no key, remove) a provider's API key in the OS keychain.", &[req("provider", String, "Provider id."), opt("key", String, "The key; omit to remove it.")]).perm(Perm::PersonOnly),
    edit("generate.setProvider", "Turn a provider on or off, or point it at another address.", &[
        req("provider", String, "Provider id."),
        opt("enabled", Boolean, "Show its models."),
        opt("baseUrl", String, "Server address (local providers and gateways)."),
        opt("options", Object, "Provider options, e.g. the ComfyUI workflows folder."),
    ]).perm(Perm::Settings),
    edit("generate.submit", "Generate an image or a video. By default a placeholder clip appears on the timeline and becomes the result when it is done; place \"library\" only adds it to the media.", &[
        PROMPT,
        PROVIDER,
        MODEL,
        opt("task", String, "text_to_image, image_to_image, text_to_video or image_to_video. Defaults from video and the images given."),
        opt("video", Boolean, "Make a video rather than an image (when task is omitted)."),
        opt("negativePrompt", String, "What to avoid, for models that take it."),
        opt("images", Array, "Input images: [{role: reference|start_frame|end_frame, path?, assetId?, clipId?, time?}]. A clip gives the frame it shows at time."),
        opt("aspectRatio", String, "\"16:9\", \"9:16\", \"1:1\"… Defaults to the project's."),
        DURATION,
        opt("resolution", String, "\"720p\", \"1080p\"… when the model offers several."),
        SEED,
        opt("count", Integer, "How many results (images)."),
        opt("audio", Boolean, "Generate sound with the video when the model can."),
        opt("params", Object, "Model-specific values (see the model's params in generate.models)."),
        opt("place", String, "\"timeline\" (default) or \"library\"."),
        OPT_TRACK,
        START,
        opt("length", Number, "Placeholder length on the timeline in seconds (default: duration, or 5)."),
        WAIT,
    ]).perm(Perm::Generate),
    edit("generate.animateFrame", "Turn the frame a clip shows at a time into a moving shot (image-to-video), placed right after the clip.", &[
        CLIP_ID,
        opt("time", Number, "Timeline time of the frame. Defaults to the playhead, or the clip's first frame."),
        OPT_PROMPT,
        PROVIDER,
        MODEL,
        DURATION,
        SEED,
        WAIT,
    ]).perm(Perm::Generate),
    edit("generate.extendClip", "Continue a clip from its last frame; the new shot lands right after it on the same track.", &[CLIP_ID, OPT_PROMPT, PROVIDER, MODEL, DURATION, SEED, WAIT]).perm(Perm::Generate),
    edit("generate.bridge", "Generate a transition from the last frame of one clip to the first frame of another, filling the gap between them.", &[
        req("fromClipId", String, "The clip before (id or name)."),
        req("toClipId", String, "The clip after (id or name)."),
        OPT_PROMPT,
        PROVIDER,
        MODEL,
        DURATION,
        SEED,
        WAIT,
    ]).perm(Perm::Generate),
    edit("generate.restyleFrame", "Edit the frame a clip shows at a time with an image model; the still lands at that time.", &[
        CLIP_ID,
        opt("time", Number, "Timeline time of the frame. Defaults to the playhead."),
        PROMPT,
        PROVIDER,
        MODEL,
        SEED,
        WAIT,
    ]).perm(Perm::Generate),
    edit("generate.regenerate", "Run a generated clip's request again (same prompt, model and settings); with variation, a new seed. The result lands after the clip.", &[
        CLIP_ID,
        opt("variation", Boolean, "Use a new seed (default false: same seed)."),
        opt("prompt", String, "Change the prompt."),
        WAIT,
    ]).perm(Perm::Generate),
    query("generate.jobs", "Generation jobs, newest first, with status, progress, outputs and errors.", &[]),
    query("generate.wait", "Wait for a job to finish and return it.", &[req("jobId", String, "Job id from generate.jobs."), opt("timeout", Number, "Give up after this many seconds (default 600).")]),
    edit("generate.cancel", "Cancel a running or queued job; its placeholder goes away.", &[req("jobId", String, "Job id from generate.jobs.")]),
    edit("generate.clearFinished", "Remove finished, failed and cancelled jobs from the list.", &[]),
    // ---- export -----------------------------------------------------------
    query("export.formats", "Export formats, qualities and encoder choices.", &[]),
    query("export.encoders", "The video encoders this computer uses per format: hardware ones (Apple VideoToolbox, NVIDIA NVENC, AMD AMF, Intel Quick Sync, VA-API, Media Foundation) that passed a test encode, and the CPU ones.", &[]),
    edit("export.start", "Render the open project to a file: every frame drawn as in the preview (titles, animation, motion graphics, 3D), encoded on the GPU or CPU with the mixed sound. Returns an export id; follow it with export.status, or pass wait.", &[
        req("path", String, "Destination file. The extension should match the format."),
        opt("format", String, "mp4 (default), hevc, prores, webm, gif, audio (AAC) or wav."),
        opt("quality", String, "draft, standard (default) or high."),
        opt("width", Integer, "Output width (default: the project's)."),
        opt("height", Integer, "Output height (default: the project's)."),
        opt("fps", Number, "Output frame rate (default: the project's)."),
        opt("from", Number, "Start of the range in seconds (default 0)."),
        opt("to", Number, "End of the range in seconds (default: the end)."),
        opt("encoder", String, "auto (default: the GPU or media engine when there is one, redone on the CPU if it fails), hardware (GPU only; WebM may be AV1) or software (CPU only: slower, smallest files)."),
        WAIT,
    ]).perm(Perm::Files),
    query("export.status", "Exports with their progress, or one export.", &[opt("exportId", String, "One export.")]),
    edit("export.cancel", "Stop an export; nothing is left at the destination.", &[req("exportId", String, "Export id.")]),
    // ---- handoff (lsuite) -------------------------------------------------
    query("handoff.apps", "Other lsuite apps installed on this computer (from ~/.lsuite/apps) and whether they are running.", &[]),
    edit("handoff.toRyolune", "Send the cut to ryolune to score it: renders the audio (WAV) and writes its length and markers next to it; when ryolune is running, imports the audio there and adds the markers.", &[
        opt("from", Number, "Start of the range in seconds (default 0)."),
        opt("to", Number, "End of the range in seconds (default: the end)."),
        opt("name", String, "Name for the hand-off files (default: the project name)."),
    ]).perm(Perm::Files),
    edit("handoff.fromRyolune", "Put audio from ryolune on an audio track: a file ryolune exported (path), or, when ryolune is running, a fresh bounce of its open song.", &[
        opt("path", String, "An audio file from ryolune. Omit to ask the running ryolune for a bounce."),
        OPT_TRACK,
        opt("start", Number, "Seconds (default 0)."),
    ]).perm(Perm::Files),
    // ---- app --------------------------------------------------------------
    query("app.info", "Version, ffmpeg, library and data folders, whether the window and the bridge are running.", &[]),
    query("app.commands", "Describe every command with its parameters, or one command.", &[opt("command", String, "One command name.")]),
    query("app.fonts", "Font families text clips can use: the bundled ones (Manrope, IBM Plex Mono, Instrument Sans, Instrument Serif) first, then this computer's.", &[]),
    query("app.settings", "Every setting with its value (agent permissions, updates, appearance, default models).", &[]),
    edit("app.setSetting", "Change one setting by dotted key, e.g. updates.checkOnStart or appearance.mode. Agent permissions stay with the person.", &[
        req("key", String, "Dotted key from app.settings."),
        req("value", Any, "New value, of the same type."),
    ]).perm(Perm::Settings),
    edit("app.setAgentKey", "Save (or with no key, remove) the API key the built-in agent uses, in the OS keychain.", &[
        req("provider", String, "\"anthropic\" or \"openai\"."),
        opt("key", String, "The key; omit to remove it."),
    ]).perm(Perm::PersonOnly),
    query("app.checkUpdates", "Check GitHub Releases for a newer kimchi and report it.", &[]),
    edit("app.installUpdate", "Download, verify (signature) and install the update found by app.checkUpdates; kimchi restarts into it.", &[]).perm(Perm::AppControl),
    edit("app.quit", "Quit kimchi.", &[]).perm(Perm::AppControl).window(),
    edit("app.notify", "Show a short message in the window.", &[req("text", String, "Message."), opt("kind", String, "info (default), success or error.")]).window(),
    // ---- ui ---------------------------------------------------------------
    query("ui.state", "What the window shows: home or editor, playhead, playing, selection, zoom, open panel and dialogs, theme.", &[]),
    edit("ui.select", "Select clips (or one media item) in the window.", &[opt("clipIds", Array, "Clips to select (ids or names); empty clears."), opt("assetId", String, "A media item to select instead.")]).window(),
    edit("ui.showPanel", "Open a panel or dialog: media, generate, text, motion (left panel), agent, jobs, settings, export, palette; or home.", &[
        req("panel", String, "Panel name."),
        opt("section", String, "For settings: models, agent, appearance, updates or about."),
    ]).window(),
    edit("ui.closeDialogs", "Close open dialogs and popovers.", &[]).window(),
    edit("ui.zoom", "Zoom the timeline.", &[opt("pixelsPerSecond", Number, "4-600."), opt("fit", Boolean, "Fit the whole project in view.")]).window(),
    edit("ui.screenshot", "Save a PNG of the window and return its path.", &[opt("path", String, "Destination .png (default: a temporary file).")]).window(),
];

/// Runs the handler for a validated command.
pub async fn dispatch(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.family() {
        "project" => project::run(s, cx, a).await,
        "media" => media::run(s, cx, a).await,
        "track" => track::run(s, cx, a).await,
        "motion" => motion::run(s, cx, a).await,
        "clip" => clip::run(s, cx, a).await,
        "transition" => transition::run(s, cx, a).await,
        "timeline" => timeline::run(s, cx, a).await,
        "history" => history::run(s, cx, a).await,
        "generate" => generate::run(s, cx, a).await,
        "export" => export::run(s, cx, a).await,
        "handoff" => handoff::run(s, cx, a).await,
        "app" => app::run(s, cx, a).await,
        "ui" => ui::run(s, cx, a).await,
        _ => Err(unhandled(cx)),
    }
}

pub(crate) fn unhandled(cx: &Ctx) -> std::string::String {
    format!("`{}` is not implemented", cx.spec.name)
}
