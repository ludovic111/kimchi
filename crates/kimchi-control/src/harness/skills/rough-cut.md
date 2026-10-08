# Rough cut from footage
When: the person has footage (files, or media already in the project) and wants a first assembly of it.

## Steps

1. `project.overview`. No project open and the person asked for one: `project.create {name, width, height}`
   in the footage's shape (16:9 is 1920×1080). Footage given as files: `media.import {paths}` (without
   `place`, so you choose the order).
2. Look before you cut: `media.look {assetId, frames: 6}` on each video shows its content across its length;
   `media.list` gives lengths. Note the strongest moments (in seconds of the media) and the story order:
   establishing shot, action, details, reaction, resolution.
3. Lay the shots end to end on one picture track with `clip.insertMedia {assetId, start}`: each `start` is
   where the previous clip ends. Put them in one `project.batch`.
4. Shape each shot: `clip.trim {clipId, edge: "start", time}` and `edge: "end"` keep the part that matters
   (cut on movement, drop the slate and the camera settling). Then close the holes this leaves:
   `timeline.closeGap {trackId, time}` inside each gap, or `clip.delete {clipIds, ripple: true}` for whole
   shots you drop.
5. Hit the target length: if no length was given, aim for what the material holds (often 30–90 s). Too
   long: shorten the longest shots first, then drop the weakest. Too short: give the best shots room.
6. Sound: music or ambience on an audio track under the picture (`clip.insertMedia` on an audio track),
   faded in and out (`clip.update {fadeIn: 1, fadeOut: 2}`). Interviews stay in sync with their picture.
7. A marker (`timeline.addMarker {time, label}`) at each section change, so the person can find their way.

## Checks

- `project.overview`: one picture track without gaps, the length within 10 % of the target, `problems` empty.
- `harness.look` over the whole cut: every frame shows something (no blank frames), the order reads as a
  story, no two neighbouring shots look the same (a jump cut).
- `audio.measure`: the mix isn't silent when there is sound, and no clip is far louder than the rest.
- Report the cut shot by shot in a few lines (what, from where to where).
