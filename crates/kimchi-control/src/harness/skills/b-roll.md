# B-roll generation
When: the person wants generated shots (b-roll, cutaways, establishing shots, inserts) to cover parts of the cut, or a missing shot made.

## Steps

1. Only when asked: generation spends the person's credits. `generate.providers` shows which are ready;
   `generate.models {task: "text_to_video"}` (or `text_to_image` for stills) lists models. None ready:
   tell them which key to add in Settings; never ask for the key itself.
2. Find where cover helps: long talking shots, jumps between takes, gaps, the opening. `project.overview`
   and `harness.look` show them. Match the project's aspect ratio (the default) and look.
3. Write each prompt as a shot: subject, action, camera move, lens, light, mood ("slow dolly past rain on a
   neon-lit Tokyo street at night, 35 mm, shallow depth of field"). Vary them across shots.
4. `generate.submit {prompt, video: true, duration, start, trackId}`: a placeholder lands on the timeline at
   once and becomes the shot. B-roll goes on a video track above the main one, so the main sound keeps
   playing under it. `wait: true` to get the result before going on, or follow with `generate.jobs` /
   `generate.wait`.
5. From the cut itself: `generate.animateFrame {clipId, time, prompt}` moves a still frame,
   `generate.extendClip` continues a shot, `generate.bridge {fromClipId, toClipId, prompt}` makes a
   transition shot between two clips, `generate.restyleFrame` repaints a frame.
6. A shot close but not right: `generate.regenerate {clipId, variation: true}` rather than a new prompt.

## Checks

- `media.look {assetId}` on each result: it shows what was asked, no artefacts (warped hands, melting text).
- `harness.look` where it landed: it cuts well with its neighbours (colour, motion direction); grade it to
  match (skill `color-grade`).
- Report each shot's prompt, model and where it landed. Make no more shots than asked.
