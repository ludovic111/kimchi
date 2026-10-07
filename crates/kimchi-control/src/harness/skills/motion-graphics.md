# Motion graphics
When: the person wants animated titles, lower thirds, callouts, counters, charts, kinetic type, logo animations or any 2D animation on the cut.

## The model

kimchi draws motion itself, the same in the preview and the export, and everything stays editable:

- **Animate any clip**: `clip.animate {clipIds, preset}` (entrances fadeIn, riseIn, slideInLeft, popIn,
  zoomIn, blurIn…; exits fadeOut, popOut…; whole-clip kenBurns, panLeft, pulse, float; `motion.presets`
  lists them) or keyframes `clip.setKeyframes {clipId, property, keyframes}` on x, y, scale, rotation,
  opacity, blur (titles also fontSize, color, letterSpacing). Keyframes: `[[0, 0], [0.6, 1, "easeOut"]]`,
  times in seconds from the clip's start.
- **Templates**: `motion.templates` lists them with their values (lowerThird, titleCard, kineticType,
  counter, barChart, logoReveal, callout, quote, subscribe, aurora, wipe…). `motion.addTemplate {template,
  values, start, duration}`; change the values later with `motion.setTemplate`.
- **Your own scene**: `motion.add {scene: {type: "2d", layers: [...]}, start}`: layers of shapes, text,
  paths, images, with keyframes, masks, effects and text animators. **Read `motion.guide {topic: "2d"}`
  before writing one** (every property, easings, reveals, examples); `motion.guide {topic: "keyframes"}`
  for timing. Edit pieces with `motion.updateLayer {clipId, id, props}`, `motion.setKeyframes`,
  `motion.setLayer`; give layers readable ids (`title`, `bar`).

## Steps

1. `project.overview`: canvas, length, what is on which track. Motion clips go on a video track above the
   picture they cover (`track.add {kind: "video"}` adds one on top); without a background they are
   transparent over what is below.
2. Start from a template when one fits; write a scene when none does.
3. Animate with intent: entrances easeOut 0.3–0.8 s, exits easeIn and a little quicker, related elements
   staggered 0.05–0.15 s, a hold long enough to read (2 s or more for text), nothing moving without reason.
   One style across the piece: the same fonts, colours and easing.
4. Keep text inside the safe area (5 % margins), large enough, readable over the picture (a plate, a shadow).

## Checks

- `project.renderFrame {times: [entrance + 0.1, middle, exit - 0.1]}` for each motion clip: nothing cut off,
  no overlaps you didn't mean, the end state clean. Fix with `motion.updateLayer` / `motion.setTemplate`
  and look again.
- `harness.look` over the span: the animation reads as one piece with the cut.
