# Social vertical edit
When: the cut is for TikTok, Instagram Reels or YouTube Shorts (9:16), or the person wants a vertical version of a horizontal cut.

## Steps

1. Canvas: `project.setSettings {width: 1080, height: 1920}` (or create the project that size). For a
   vertical version of an existing cut, `project.duplicate` first if the person wants to keep the original
   (needs the projects permission), then change the canvas.
2. Reframe each horizontal shot: `clip.update {clipId, fit: "cover"}` fills the frame; then move the subject
   into view with `x` (positive moves the picture right). Look at each shot with `project.renderFrame` at its
   middle: faces and action must stay in frame.
3. Hook: the first 1–2 s carry the most striking image or line. Cut hard and fast: 0.8–2 s a shot.
   15–60 s in all unless asked.
4. Text: big and high contrast (fontSize 70–110 on 1080 wide, a plate or shadow), inside the safe zone:
   not in the top 10 % (y above -768) nor the bottom 20 % (y below 576), where the apps draw buttons and
   captions. Burned-in captions help: most people watch without sound (skill `titles-and-captions`).
5. Sound: music on the beat (`audio.beatCut`), voice clear, the mix at -14 LUFS
   (`audio.setMaster {loudness: -14}`).
6. Delivery: the `shorts` export preset (1080×1920, 30 fps, -14 LUFS), when the person asks for the file.

## Checks

- `project.overview`: canvas 1080×1920, length within the platform's limit.
- `harness.look` across the cut: every frame filled (no black bars unless intended), subjects in frame,
  text inside the safe zone and readable.
- `audio.measure`: integrated loudness near -14 LUFS (or the master's target set to it), true peak ≤ -1 dBTP.
