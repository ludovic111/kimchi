# Motion graphics and 3D in kimchi

kimchi draws animation itself, the same in the preview and the export. There are three ways in:

1. **Animate any clip** (video, image, title, solid, motion clip): `clip.setKeyframes` on its
   placement (x, y, scale, rotation, opacity, blur…), or a ready-made `clip.animate` preset.
2. **Motion clips** (`motion.add`): a 2D scene, like an After Effects composition (layers of
   shapes, paths, text, images, particles; nested compositions, parenting, masks, track mattes,
   effects, shape operators, text animators, motion blur), or a 3D scene, like a Blender scene
   (cameras, lights, a world, objects: primitives, editable meshes, extruded text and logos,
   lathed profiles, curves, particles, models; materials, modifiers, constraints, two render
   engines), written as JSON, every property animatable and drivable by an expression.
3. **Templates** (`motion.addTemplate`): a lower third, a title card, a 3D title… from a few
   values; the clip remembers them so `motion.setTemplate` can change them later.

To change part of a scene, prefer the small commands: `motion.updateLayer {clipId, id, props}` sets
some properties of one layer, object, light, the camera or the scene (an animated property gets a
keyframe at `time` instead), `motion.addKeyframe` / `motion.removeKeyframe` / `motion.setKeyframes`
animate one property, `motion.setLayer` adds or replaces a whole layer, `motion.removeLayer` deletes
one; `motion.setStackItem` adds a modifier, constraint, effect, operator, mask or text animator
(`motion.stackTypes` lists them all with every parameter), `motion.setExpression` drives a
property with a formula, `motion.editMesh` models a mesh. People make the same changes in the
Studio (the window's motion editor), so keep ids readable (`title`, `logo`, `floor`).

Work in this order: add, then **look** (`project.renderFrame` with a few `times`; the result is
a PNG path you can open; `motion.view` shows a 3D scene from any side), then fix what you see. A motion clip sits on a video track like any
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

**More layer types**: `null` (draws nothing: a handle others follow), `adjustment` (its
`effects` apply to everything below it in its list, inside its masks), `comp` `{comp, speed,
offset, loop, time}` (shows a composition: comp time = (scene time − start) × speed + offset, or
`time` when set — animate `time` to remap it; past a composition's `duration` it shows nothing
unless it loops), `particles` (see Particles below).

**Compositions** (precomps): `"compositions": [{"id": "card", "width": 800, "height": 400,
"duration": 3, "background": null, "layers": [...]}]` at the scene's top level; a `comp` layer
shows one, placed and transformed like any layer, with its own time. Ids are unique across the
whole scene. `motion.precompose {clipId, ids, compositionId}` moves layers into a new one.

**Parenting**: `"parent": "<sibling id>"`: the layer follows the parent's position, rotation and
scale (its own values become relative), like After Effects. Parent to a `null` to move a group of
layers without grouping them.

**Masks** (on the layer itself, in its own pixels from its centre): `"masks": [{"type": "path",
"d": "M-200 -100 L200 -100 L200 100 Z", "mode": "add", "feather": 20}, {"type": "ellipse",
"center": [0, 0], "size": [300, 300], "mode": "subtract"}]`. Modes `add subtract intersect
difference none`; `feather`, `expansion`, `opacity`, `inverted`. Animate `masks.<id>.d` to morph a
path mask, `masks.<id>.feather`…

**Track mattes**: `"matte": {"layer": "<sibling id>", "mode": "alpha"|"alphaInverted"|"luma"|
"lumaInverted"}`: the other layer's picture decides where this one shows (it isn't drawn itself).
The older `mask`/`maskInvert` is an alpha matte of a sibling's shape.

**Effects** (applied to the layer's picture in order): `"effects": [{"type": "glow", "radius": 30},
{"type": "colorCorrect", "saturation": 0.3}]`. Types: blur, directionalBlur, radialBlur, glow,
dropShadow, stroke (outline), echo, colorCorrect, levels, tint, tritone, fill, gradientRamp,
invert, threshold, posterize, vignette, noise (grain), fractalNoise, halftone, scanlines,
turbulentDisplace, waveWarp, ripple, twirl, bulge, mosaic, chromaticAberration, glitch, mirror,
kaleidoscope, motionTile, cornerPin, sharpen. Each has an id (default its type) and parameters
(`motion.stackTypes {"family": "effects"}`); animate them as `effects.<id>.<param>`:
`{"effects.glow.radius": [[0, 0], [1, 40]]}`.

**Shape operators** (on rect, ellipse, polygon, star, path outlines, in order):
`"operators": [{"type": "repeater", "copies": 8, "rotation": 45, "position": [0, 0]}]`. Types:
repeater (copies with a transform step and an opacity ramp; works on groups too), offset,
zigzag, wiggle (boiling lines), roundCorners, twist, puckerBloat.

**Text animators** (text layers): `"animators": [{"type": "range", "by": "char", "start": 0,
"end": 30, "offset": -30, "y": 40, "opacity": 0, "shape": "rampUp"}]` moves, turns, scales, fades,
blurs or recolours the letters (words, lines) a range picks; animate `animators.<id>.offset`
from −100 to 100 to sweep it across. `"type": "wiggly"` jitters them. Properties: x, y, scale,
rotation, opacity, fill, blur, tracking, skew, amount. **Text on a path**: `"path": "M-400 0 C…"`
(the baseline follows it), `pathOffset` 0–1 slides the text along.

**Motion blur**: `"motionBlur": true` on a layer blurs it along its motion in exports and renders
(the scene's `shutter`, default 0.5 = a 180° shutter, and `motionBlurSamples`, default 8).

**Blend modes** also: `exclusion hue saturation color luminosity`.

## Particles

`{"id": "sparks", "type": "particles", "rate": 60, "lifetime": 1.5, "speed": 300, "spread": 40,
"direction": [0, -1, 0], "gravity": [0, 500, 0], "size": 8, "sizeEnd": 0, "color": "#ffd27a",
"colorEnd": "#ff5a36", "shape": "spark", "emitter": "point"}` — a 2D layer or a 3D object. Many
small things are born from the layer's (object's) position and fly, fall, swirl and fade.
`rate` per second and/or `burst` at `emitFrom`; `emitUntil`; `lifetime` (+ `lifetimeRandom`);
`emitter` point, line, rect/box, circle/disc, ring, sphere (`emitterSize`); `direction`,
`spread` (degrees), `speed` (+ `speedRandom`), `gravity`, `drag`, `turbulence`; `size`
(diameter), `sizeEnd` (a multiple of `size` at the end of life: 0 shrinks away, 2 doubles), `sizeRandom`, `spin`, `spinRandom`; `color`, `colorEnd`, `colors` (each picks one:
confetti); `fadeIn`/`fadeOut` (share of life); `shape` 2D circle, square, triangle, star, spark,
image / 3D sphere, cube, tetra, spark, image (`asset`); `trail` (default true: particles stay
where they were born when the emitter moves); `prewarm`; `seed`. Units: pixels in 2D (y down),
world units in 3D (defaults 200 px/s or 2 units/s). The same frame always shows the same
particles. Birth settings (`rate`, `lifetime`) can't be animated; the others can.

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
reflect them. `shadows` (default true): the strongest directional light, spot lights and area
lights cast soft shadows. `fog` (default true): with a background, distant things fade into it (a soft horizon).

**Lights**: `{id, type: "directional"|"point"|"spot"|"area", color, intensity, direction (where
it shines towards), position (point, spot, area), range (point and spot: distance where it fades
out, 0 = never), angle (spot: the cone's full angle, default 45), blend (spot: soft edge 0–1),
size (area: [width, height]; point/spot: bulb radius; sun: softness), castShadows, hidden}`.
Shadows come from the main directional light and spot and area lights (four at most; softer as
`size` grows); point lights cast none in the standard engine (the path tracer shadows every
light).

**Cameras**: the main `camera` (id `camera`) and more in `"cameras": [{"id": "close", "position":
…, "target": …}]`; `"activeCamera": "close"` picks the one filming, and scene keyframes
`{"activeCamera": [[0, "camera"], [2, "close"]]}` cut between them. Each: `position`, `target`,
`fov`, `roll`, `projection` (`perspective` or `orthographic` with `orthoSize`, the height it
covers), depth of field `fStop` (like a lens: 1.4 very blurry, 8 sharp; 0 = off) and
`focusDistance` (0 = the target's distance), `constraints` (a `lookAt` keeps it on an object,
`followPath` flies it along a curve).

**Camera moves**: `motion.cameraMove {"move": "orbit", "degrees": 90}` writes a classic shot as
plain keyframes, constraints and expressions you can edit afterwards (default over the whole clip;
`from`/`to` scene seconds, `easing`, `camera`): `orbit`/`turntable` (around `around`: an object
id it keeps facing, a point, or what it looks at; an invisible circle curve `cameraOrbit`, a
`followPath` constraint `orbit` and keyframes of `constraints.orbit.progress`), `dolly`
(`distance` + in), `truck` (+ right), `crane` (+ up, still looking at the same point) as two
`position` (and `target`) keyframes, `zoom` (`amount` degrees of `fov`, − closer), `flyThrough`
(along `path`, a curve object, or new `points`; `lookAt` an object, else it faces along the
path: a `followPath` constraint `flyThrough`), `handheld` (`amount` 1 gentle: `wiggle`
expressions on position, target and roll) and `clear`.

**World**: `"environment": {"type": "gradient", "top": "#5b7fb8", "horizon": "#d8e2ee",
"bottom": "#3a3632", "strength": 1, "visible": true}` lights the scene from all around and is
reflected by shiny things. Types `color` (`color`), `gradient`, `sky` (a daylight sky lit by the
main directional light), `image` (`image`: a 360° panorama picture; `rotation` degrees).
`visible` shows it behind the objects (instead of `background`). Without one, `ambient` and
`ambientColor` do the same job more simply.

**Every object**: `id`, `type`, `position` [x, y, z], `rotation` [x, y, z] degrees (applied x, then
y, then z), `scale` (number or [x, y, z]), `material`, `modifiers`, `constraints`, `children`
(objects that move with it, positions relative to it), `castShadow`, `start`/`end`, `hidden`,
`expressions`, `keyframes`. An object's front is its +z side.

**Types**: `box` `{size: [w, h, d] or number, bevel (rounded edges)}`, `sphere` `{radius,
segments}`, `icosphere` `{radius, detail}`, `cylinder` `{radius, height, segments}` (3 = a
prism, 6 = a hexagon), `cone` `{radius, height, segments}` (4 = a pyramid), `capsule` `{radius,
height}`, `torus` `{radius, tube}`, `grid` `{width, height, rows, cols}` (a flat subdivided
sheet, y up: for waves and terrain), `extrude` `{d: SVG path data (a logo, an icon), size (its
larger side in world units, default 2), depth, bevel}` (holes stay holes), `lathe` `{profile:
[[radius, height]…], segments, angle}` (a profile turned around y: vases, glasses), `curve`
`{points: [[x, y, z]…], closed, smooth, radius (0 = an invisible path for followPath), sides,
trimStart, trimEnd}` (a tube along the points; animate `trimEnd` 0 → 1 to draw it on), `mesh`
`{vertices: [[x, y, z]…], faces: [[0, 1, 2, 3]…] (counter-clockwise seen from outside), uvs,
autoSmooth (degrees)}` (see Modelling), `particles` (see Particles), `plane`
`{width, height}` (faces +z: a card facing the camera; rotate [-90, 0, 0] for a floor), `text`
`{text, fontFamily, fontWeight, size (letter height), depth, align, letterSpacing, bevel}`
(extruded, centred), `model` `{src: a .glb/.gltf/.obj/.stl file path or media item}` (centred,
scaled to 2 units, keeps
its own materials; a `material` with a `color` tints it), `image` `{asset, width}` (a picture card in its own colours),
`group` (only children).

**Material**: `{color, metallic 0–1, roughness 0–1 (0 mirror, 1 matte), emissive (a colour that
glows), emissiveIntensity, opacity, transmission (glass: 0 opaque … 1 clear) with ior (1.33
water, 1.5 glass, 2.4 diamond), clearcoat (a varnish layer, car paint), texture (picture wrapped
on it), textureScale ([u, v] repeats), pattern (a procedural surface: `{"type": "marble",
"color": "#f2efe9", "color2": "#8a8178", "scale": 2, "bump": 0.3}`; types checker, stripes,
dots, noise, marble, wood, voronoi, bricks, gradient), flat (faceted look), unlit (the colour as
is, no lighting)}`. **Shared materials**: `"materials": [{"id": "gold", "color": "#e8b04a",
"metallic": 1, "roughness": 0.25}]` at the scene's top level, used as `"material": "gold"`
(`motion.setMaterial` adds or changes one).

**Modifiers** (evaluated in order on the shape, like Blender's stack; any object):
`"modifiers": [{"type": "array", "count": 8, "rotation": [0, 45, 0], "relative": [0, 0, 0],
"offset": [1.5, 0, 0]}, {"type": "bevel", "width": 0.04}]`. Types: subdivision (smooth),
mirror, array (lines, circles, spirals), bevel, solidify, displace (noise bumps; animate
`evolution`), twist, bend, taper, wave (moves on its own), smooth, wireframe, boolean (cut with,
join with or intersect another object: `{"type": "boolean", "object": "cutter", "operation":
"difference"}`; hide the cutter), decimate, triangulate, explode (animate `progress`), build
(faces appear: animate `progress`), weld, spherify, noise (jitter / boil). Parameters:
`motion.stackTypes {"family": "modifiers"}`; animate them as `modifiers.<id>.<param>`.

**Constraints** (after keyframes and expressions): `lookAt {target}` (face an object; cameras
and lights aim at it), `followPath {path: a curve's id, progress 0–1, align}` (animate
`constraints.followPath.progress`), `copyPosition`, `copyRotation`, `copyScale {target}`,
`limitPosition {min, max}`, `floor {height}`; each with `influence` 0–1.

**Render settings** (`"render"`): the preview while editing always uses the fast standard engine;
exports and clips rendered ahead use `engine`: `"standard"` (fast, like Eevee) or `"path"` (a
path tracer, like Cycles: true reflections, refraction through glass, soft shadows, bounced
light; slow, best rendered ahead) with `samples` (64), `bounces` (4), `denoise` (true). Both:
`exposure` (stops), `toneMapping` (`standard` or `filmic`), `bloom` (glow around bright things,
0 = off) with `bloomThreshold` and `bloomRadius`, `motionBlur` (share of a frame the shutter is
open, 0.5 = 180°; 0 = off) with `motionBlurSamples`, `ambientOcclusion` (darker creases, 0–1).

**Animatable**: objects `position position.x position.y position.z` (or `x y z`), `rotation
rotation.x rotation.y rotation.z`, `scale scale.x scale.y scale.z`, `color opacity metallic
roughness emissive emissiveIntensity transmission ior clearcoat textureScale pattern.<param>`,
`modifiers.<id>.<param>`, `constraints.<id>.<param>`, plus `size bevel` (box), `radius`
(sphere, icosphere, cylinder, cone, capsule, torus, curve), `height`, `tube`, `width`, `text size
depth letterSpacing bevel` (text), `d size depth` (extrude), `angle` (lathe), `trimStart trimEnd
points` (curve), particle settings. Cameras (id `camera` or theirs): `position target fov roll
orthoSize fStop focusDistance` and the `.x/.y/.z`s. Lights: `intensity color range angle blend
size position direction`. Scene (id `scene`): `background ambient ambientColor activeCamera
environment.strength environment.rotation environment.color/top/horizon/bottom render.exposure
render.bloom render.bloomThreshold render.bloomRadius render.motionBlur
render.ambientOcclusion`.

3D draws on the GPU (Metal on Macs including Apple Silicon, Vulkan or DirectX 12 elsewhere), or
on the CPU when there is none; both give the same picture. The path tracer runs on the CPU.

## Modelling

Any object can become an editable mesh: `motion.convertToMesh {clipId, id}` (keeps its
modifiers, or bakes them with `applyModifiers`); `motion.applyModifier` bakes one modifier or
the stack. Then `motion.editMesh {clipId, id, op, faces | vertices | select, params}` works like
Blender's edit mode, and answers with the new selection to chain the next step:

```json
{"op": "extrude", "select": {"facing": [0, 1, 0]}, "params": {"distance": 0.6}}
{"op": "inset", "faces": [6], "params": {"thickness": 0.15}}
{"op": "scale", "faces": [6], "params": {"factor": [0.5, 1, 0.5]}}
{"op": "bevel", "select": {"all": true}, "params": {"width": 0.04, "segments": 3}}
```

Ops: extrude, extrudeIndividual, inset, bevel, subdivide, loopCut, delete, dissolve, merge,
fill, bridge, flip, recalcNormals, translate (move), rotate, scale, mirror, duplicate, triangulate, poke,
smooth, spin, knife, unwrap. Selections: vertex and face indices (`motion.get` shows the mesh),
or `{"all": true}`, `{"facing": [x, y, z], "angle": 30}`, `{"inside": [[x0, y0, z0], [x1, y1,
z1]]}`, `{"edgeLoop": [v0, v1]}`, `{"edgeRing": [v0, v1]}` (add `"linked": true` to grow to
everything connected). `motion.stackTypes {"family": "editOps"}` lists each operation's values. Add a `subdivision` modifier for smooth,
organic shapes; `bevel` for crisp product shots.

EXPRESSIONS_GUIDE

## Rendering ahead

A motion clip is drawn live: quickly in the preview, at full quality (the scene's engine,
samples, motion blur) in the export. `motion.render {clipIds}` renders it ahead at full quality
into a file the timeline then plays: smooth playback, fast exports, and the path tracer's
quality while editing. `motion.renderStatus` follows it and shows each clip's state (live,
rendered, outdated: the scene changed since, so it is drawn live again until rendered again);
`motion.unrender` goes back to live. Render heavy 3D (path tracer, many particles, big
subdivisions) once the scene is settled.

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
- `motion.view {clipId, axis: "front"|"top"|…}` or `{view: {position, target}}` shows a 3D scene
  from another side (with a grid), `shading: "rendered"` with the final engine.
- Errors name the field and the fix ("Unknown field `colour` in rect layer \"card\". Did you
  mean `color`?").
- Keep text inside the frame's safe area (about 5% from each edge), sizes ≥ 3% of the height
  for body text, and contrast with what is below (a plate, a shadow or a glow helps).
