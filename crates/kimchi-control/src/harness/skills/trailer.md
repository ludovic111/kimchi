# Trailer or teaser
When: the person wants a short, punchy piece that sells a longer film, product or event (a trailer, a teaser, a sizzle reel).

## Steps

1. `project.overview` and `media.look` on the material: pick the 8–20 strongest moments (faces, action,
   reveals, wide shots that set the world). A teaser shows less: 4–8 moments and a title.
2. Structure in three acts over 30–90 s (15–30 s for a teaser):
   - **Set-up** (first quarter): the world, slower shots of 2–3 s, music low.
   - **Build** (middle): the stakes, shots shortening from 2 s to under 1 s, cards with short lines.
   - **Climax and button**: the fastest cutting, then a beat of black or a hold, the title card, a final
     short moment (the "button"), the date or call to action.
3. Music drives it: put it on an audio track first (`clip.insertMedia`), then `audio.detectBeats` and cut
   the picture on the beats with `audio.beatCut {musicClipId, every: 2}` in the build (every 1 at the climax).
   No music in the project: offer to score it in ryolune (skill `scoring`).
4. Cards: `motion.addTemplate {template: "titleCard"}` or `kineticType` for 2–4 short lines ("THIS SUMMER"),
   each 1.5–2.5 s, between shots on the top track. The final title card holds 3 s or more.
5. Transitions: hard cuts; a dip to black (`transition.set {kind: "dipToBlack", duration: 0.4}`) between
   acts; a flash (`dipToWhite`, 0.2 s) on a big hit at most once or twice.
6. Sound design: fade the music out under the button, bring it to -14 LUFS (`audio.setMaster {loudness: -14}`).

## Checks

- Shot lengths shorten toward the end: list clip durations from `project.overview`.
- Every card is legible: `project.renderFrame` at each card's middle; text inside the safe area.
- `harness.look` over the whole piece: no blank frames except the intended beat of black.
- `audio.measure`: the music reaches the end or fades out; no abrupt stop.
