# Motion graphics and 3D in kimchi

kimchi draws animation itself, the same in the preview and the export. There are three ways in:

1. **Animate any clip** (video, image, title, solid, motion clip): `clip.setKeyframes` on its
   placement (x, y, scale, rotation, opacity, blur…), or a ready-made `clip.animate` preset.
2. **Motion clips** (`motion.add`): a scene of 2D layers (shapes, paths, text, images) or a
   3D scene (camera, lights, objects), written as JSON, every property animatable.
3. **Templates** (`motion.addTemplate`): a lower third, a title card, a 3D title… from a few
   values; the clip remembers them so `motion.setTemplate` can change them later.

To change part of a scene, prefer the small commands: `motion.updateLayer {clipId, id, props}` sets
some properties of one layer, object, light, the camera or the scene (an animated property gets a
keyframe at `time` instead), `motion.addKeyframe` / `motion.removeKeyframe` / `motion.setKeyframes`
animate one property, `motion.setLayer` adds or replaces a whole layer, `motion.removeLayer` deletes
one. People make the same changes in the inspector, so keep ids readable (`title`, `logo`, `floor`).

Work in this order: add, then **look** (`project.renderFrame` with a few `times`; the result is
a PNG path you can open), then fix what you see. A motion clip sits on a video track like any
clip: tracks above draw over it, its own placement and fades apply, and without a `background`
it is transparent over what's below.

## Keyframes

Keyframes are `[time, value]`, `[time, value, "easing"]` or `{"time", "value", "easing"}`:

```json
{"x": [[0, -800], [0.6, 0, "easeOutBack"]], "opacity": [[0, 0], [0.3, 1]]}
```

- Times are seconds from the start of whatever owns them: the clip (clip keyframes) or the
  scene (layer, object, camera keyframes). A scene's 0 is the clip's first frame.
- Before the first keyframe the value is the first one; after the last, the last one. One
  keyframe is a constant.
- The **easing belongs to the keyframe it arrives at**: `[0.6, 0, "easeOut"]` decelerates into 0.
  Default `linear`. Names: `linear`, `hold` (jump at the keyframe), `ease`, `easeIn`, `easeOut`,
  `easeInOut`, and `ease<In|Out|InOut><Sine|Quad|Cubic|Quart|Quint|Expo|Circ|Back|Elastic|Bounce>`
  (`easeOutBack` overshoots, `easeOutBounce` bounces), `cubicBezier(x1, y1, x2, y2)`,
  `spring(bounce)` (0 = no overshoot, 1 = very bouncy).
- Values are numbers, `[x, y]` / `[x, y, z]` vectors, colours (`"#ff5a36"`, `#rrggbbaa`), which
  blend, or strings: two SVG paths with the same commands morph; other strings switch at the
  keyframe (`"text"` keyframes change words).

Good motion: entrances `easeOut`/`easeOutCubic` (0.3–0.8 s), exits `easeIn`, gentle loops
`easeInOutSine`, playful pops `easeOutBack`. Stagger related elements by 0.05–0.15 s.

## Clip animation

`clip.setKeyframes {clipId, property, keyframes}`: properties `x`, `y` (offset of the centre from
the canvas centre, project pixels, y down), `position` ([x, y]), `scale` (1 = fitted to the
canvas), `scaleX`, `scaleY` (multiply scale), `rotation` (degrees clockwise), `opacity` (0–1),
`blur` (pixels), `volume` (0–4); text clips also `fontSize`, `color`, `letterSpacing`.
`clip.addKeyframe` sets one at a timeline time; `clip.removeKeyframe` removes one or all.

`clip.animate {clipIds, preset, length?}` writes the keyframes of a preset (see
`motion.presets`): entrances `fadeIn riseIn slideInLeft slideInRight slideInUp slideInDown popIn
zoomIn spinIn dropIn blurIn`, exits `fadeOut sinkOut slideOutLeft slideOutRight slideOutUp
slideOutDown popOut zoomOut spinOut blurOut`, whole-clip `kenBurns kenBurnsOut panLeft panRight
pulse float wiggle shake spin`. Presets combine (an entrance and an exit on one clip).

## 2D scenes

```json
{
  "type": "2d",
  "background": "#0e0e12",
  "layers": [
    {"id": "card", "type": "rect", "width": 760, "height": 180, "radius": 24, "fill": "#ff5a36",
     "keyframes": {"scaleX": [[0, 0], [0.5, 1, "easeOutCubic"]]}},
    {"id": "title", "type": "text", "text": "Hello", "fontSize": 110, "fill": "#ffffff",
     "reveal": {"by": "char", "style": "rise"},
     "keyframes": {"reveal": [[0.3, 0], [1.2, 1, "easeOut"]]}}
  ]
}
```

Coordinates are project pixels from the canvas centre, y down (a 1920×1080 canvas spans x
−960…960, y −540…540). Layers draw in order: later ones on top. `background` is optional
(none = transparent).

**Every layer**: `id` (unique), `type`, `x`, `y`, `anchorX`, `anchorY` (the point, in the layer's
own pixels from its centre, it turns and scales around), `scale`, `scaleX`, `scaleY`,
`rotation`, `skewX` (degrees), `opacity`, `fill` (colour or gradient), `stroke`
(`{color, width, cap: butt|round|square, join: miter|round|bevel, dash: [on, off…], dashOffset}`),
`trimStart`/`trimEnd`/`trimOffset` (0–1: draw only part of the outline; animate `trimEnd`
0 → 1 to draw a line on), `blur` (pixels), `shadow` (`{color, blur, x, y}`), `glow`
(`{color, radius, strength}`), `blend` (`normal multiply screen overlay add darken lighten
difference colorDodge colorBurn softLight hardLight`), `mask` (id of a sibling layer whose shape
this layer shows through; that layer isn't drawn), `maskInvert`, `start`/`end` (scene seconds
the layer is visible), `hidden`, `keyframes`.

**Types**:
- `rect` `{width, height, radius}`, `ellipse` `{width, height}`
- `polygon` `{sides, radius, roundness 0–1}`, `star` `{points, radius, innerRadius}`
- `path` `{d: "M0 0 C…" (SVG path data, any command), points: [[x, y]…], closed}`; animate `d`
  between paths with the same commands to morph.
- `text` `{text, fontFamily (Manrope, IBM Plex Mono, Instrument Sans, Instrument Serif, or
  installed ones: app.fonts), fontSize, fontWeight, italic, align: left|center|right,
  lineHeight, letterSpacing, value, decimals, reveal}`. `x`/`y` is the left edge for
  left-aligned text, the right edge for right-aligned, else the centre. `"text": "{value}%"`
  with `value` animated makes a counter (thousands get commas). Text uses `fill` (default white)
  and `stroke`.
- `image` `{asset: media id or name or a file path, width?, height?, radius}`: one size keeps
  the aspect ratio; a video shows its frame at the scene's time.
- `group` `{layers: […]}`: moves, fades, masks and blurs its children together.

**Gradient fill**: `{"type": "linear", "stops": [[0, "#ff5a36"], [1, "#7a3cff"]], "from": [-300, 0],
"to": [300, 0]}` or `{"type": "radial", "stops": […], "center": [0, 0], "radius": 400}`
(points in the layer's own pixels; defaults span the shape).

**Animatable** (keyframes): `x y position anchorX anchorY scale scaleX scaleY rotation skewX
opacity fill strokeColor strokeWidth dashOffset trimStart trimEnd trimOffset blur shadowColor
shadowBlur shadowX shadowY glowColor glowRadius glowStrength`, plus per type: rect `width height
size radius`, ellipse `width height size`, polygon `sides radius roundness`, star `points radius
innerRadius`, path `d`, text `text fontSize fontWeight letterSpacing lineHeight value reveal`,
image `width height radius`. The scene's own keyframes (id `scene`) animate `background`.

## Text reveals

`"reveal": {"by": "char"|"word"|"line", "style": "fade"|"rise"|"drop"|"slide"|"pop"|"type"|"blur",
"progress": 0–1, "overlap": 3, "distance": 40}`: letters, words or lines appear one after another
as `progress` goes from 0 to 1 (animate it with the `reveal` property). `overlap` is how many
move at once (1 = one by one, larger = smoother wave); `distance` the travel of rise/drop/slide
(default half the font size). `type` shows each letter at once, like typing.

## 3D scenes

```json
{
  "type": "3d",
  "background": "#0e0e12",
  "camera": {"position": [0, 1, 8], "target": [0, 0, 0], "fov": 35,
             "keyframes": {"position": [[0, [2, 2, 10]], [4, [0, 1, 7], "easeInOutCubic"]]}},
  "lights": [{"id": "sun", "type": "directional", "direction": [-0.5, -1, -0.7], "intensity": 1.5}],
  "objects": [
    {"id": "logo", "type": "text", "text": "KIMCHI", "size": 1.2, "depth": 0.3,
     "material": {"color": "#ff5a36", "metallic": 0.3, "roughness": 0.35},
     "keyframes": {"rotation.y": [[0, -40], [1.5, 0, "easeOutBack"]]}},
    {"id": "floor", "type": "plane", "width": 30, "height": 30, "position": [0, -0.8, 0],
     "rotation": [-90, 0, 0], "material": {"color": "#16161c", "roughness": 0.9}}
  ]
}
```

World units, y up. The camera looks from `position` at `target` (`fov` vertical degrees,
`roll` degrees). Without `lights`, a key light and a fill light are used. `ambient` (0–1, default
0.25) and `ambientColor` light everything softly from a sky above and a ground below; metals
reflect them. `shadows` (default true): the strongest directional light casts soft shadows.
`fog` (default true): with a background, distant things fade into it (a soft horizon).

**Lights**: `{id, type: "directional"|"point", color, intensity, direction (directional: where it
shines towards), position (point), range (point: distance where it fades out, 0 = never)}`.

**Every object**: `id`, `type`, `position` [x, y, z], `rotation` [x, y, z] degrees (applied x, then
y, then z), `scale` (number or [x, y, z]), `material`, `children` (objects that move with it,
positions relative to it), `start`/`end`, `hidden`, `keyframes`.

**Types**: `box` `{size: [w, h, d] or number, bevel (rounded edges)}`, `sphere` `{radius}`,
`cylinder` `{radius, height}`, `cone` `{radius, height}`, `torus` `{radius, tube}`, `plane`
`{width, height}` (faces +z: a card facing the camera; rotate [-90, 0, 0] for a floor), `text`
`{text, fontFamily, fontWeight, size (letter height), depth, align, letterSpacing}` (extruded,
centred), `model` `{src: a .glb/.gltf file path or media item}` (centred, scaled to 2 units, keeps
its own materials; `color` tints it), `image` `{asset, width}` (a picture card in its own colours),
`group` (only children).

**Material**: `{color, metallic 0–1, roughness 0–1 (0 mirror, 1 matte), emissive (a colour that
glows), emissiveIntensity, opacity, texture (picture wrapped on it), flat (faceted look), unlit
(the colour as is, no lighting)}`.

**Animatable**: objects `position position.x position.y position.z` (or `x y z`), `rotation
rotation.x rotation.y rotation.z`, `scale scale.x scale.y scale.z`, `color opacity metallic
roughness emissive emissiveIntensity`, plus `size bevel` (box), `radius` (sphere, cylinder, cone,
torus), `height`, `tube`, `width`, `text size depth letterSpacing` (text). Camera (id `camera`):
`position target fov roll` and their `.x/.y/.z`. Lights: `intensity color range position
direction`. Scene (id `scene`): `background ambient ambientColor`.

3D draws on the GPU (Metal on Macs including Apple Silicon, Vulkan or DirectX 12 elsewhere), or
on the CPU when there is none; both give the same picture.

## Templates

`motion.templates` lists them with their values; `motion.addTemplate {template, values, start,
duration}` adds one; `motion.setTemplate {clipId, values}` re-makes it. 2D: lowerThird,
titleCard, kineticType (`*word*` = accent), counter, barChart, logoReveal, callout, quote,
subscribe, aurora (moving background), wipe (a transition: put it on a track above a cut,
centred on it). 3D: title3d, logoSpin3d, turntable (a model on a pedestal), shapes3d. A template
clip is an ordinary motion clip: `motion.get` shows its scene, `motion.setLayer` and
`motion.setKeyframes` change it (until the next `motion.setTemplate`).

## Checking your work

- `project.renderFrame {"times": [0.2, 0.6, 1.2, 2.5]}` gives one labelled image of those moments:
  check positions, overlaps, legibility, that entrances end where they should.
- `motion.get {clipId}` shows the scene as kimchi stored it (defaults left out).
- Errors name the field and the fix ("Unknown field `colour` in rect layer \"card\". Did you
  mean `color`?").
- Keep text inside the frame's safe area (about 5% from each edge), sizes ≥ 3% of the height
  for body text, and contrast with what is below (a plate, a shadow or a glow helps).
