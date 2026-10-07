# Colour grade
When: the person wants the cut to look better, consistent, or in a given style (warm, cinematic, moody, vintage…).

## Steps

1. Look first: `harness.look` or `project.renderFrame {times}` with one time inside each shot gives a
   sheet to compare shots side by side. Note which are too dark, too bright, too blue or orange, flat.
2. **Balance** (correction) shot by shot with `clip.setEffects {clipIds, brightness, contrast, temperature,
   tint, saturation}`: bring exposure and white balance in line, small steps (±0.05–0.3). Shots of the same
   scene should look like the same moment.
3. **Look** (grade) on top, the same on every shot of a scene: a built-in look (`clip.setEffects {look:
   "warm" | "cool" | "punchy" | "teal" | "faded" | "vintage" | "noir" | "dreamy" | "mono"}`), one from the
   library (`looks.list`, `looks.apply {clipIds, look, strength}`), or a LUT the person gives
   (`clip.setEffects {lut: "/path/look.cube", lutStrength: 0.6}`). Fields given with `look` go on top of it.
4. Keep skin natural (watch temperature and tint on faces), keep detail in shadows and highlights, a
   `vignette` of 0.1–0.3 draws the eye; `sharpen` sparingly.
5. A change of grade over time (a flashback fading to warm): `clip.setKeyframes {property: "saturation"}`.
6. Save a look the person likes for later: `looks.save`.

## Checks

- The same sheet again after grading: shots match each other, nothing clipped to pure white or black over
  large areas, faces look like skin.
- Titles and graphics still read over the graded picture.
