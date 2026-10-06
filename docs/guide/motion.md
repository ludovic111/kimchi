# Animation, motion graphics and 3D

kimchi animates in three ways, from simplest to fullest:

1. **Keyframes on any clip**: move, scale, fade or grade a video, picture or title over time.
2. **Templates**: ready-made motion graphics (titles, lower thirds, charts, 3D logos) whose words
   and colours you change in the inspector.
3. **Motion clips edited in the Studio**: your own 2D motion design or 3D scene, with layers or
   objects, effects, cameras, lights and a timeline of their own.

Everything is drawn by kimchi's compositor, the same in the preview and the export.

## Keyframes on any clip

The inspector's **Animation** section animates a selected picture clip.

**Animating a property.** Move the playhead into the clip and click the diamond beside a property
(X, Y, Scale, Rotate, Opacity, Blur) to set a keyframe there with the current value. From then on,
changing that property, in the inspector or by dragging the clip on the preview, adds or updates a
keyframe at the playhead instead of changing the value everywhere. Click a filled diamond to remove
the keyframe under the playhead. The arrows in the section's header jump to the previous and next
keyframe of any property.

Other properties animate the same way from their own sections: the colour corrections in
[Colour](transitions-and-colour.md#corrections), and gain and pan in
[the clip's sound](sound.md#a-clips-sound). On the timeline, keyframes show as diamonds along the
bottom of the clip.

Keyframes are tied to the clip: moving the clip moves them, and trimming or splitting it leaves
them where they were on the timeline.

**Easing.** With the playhead on a keyframe, **Into this keyframe** chooses how the value
arrives there: Linear, Out, In-out, Back (a small overshoot) or Hold (jumps at the keyframe).
Commands and agents can use many more, from `easeInOutSine` to `easeOutBounce`, `spring` and
`cubicBezier(…)`; see [the project file](../PROJECT_FORMAT.md#keyframes) for the list.

**Presets** write keyframes for a common move, which you can then adjust:

| Group | Presets |
| --- | --- |
| In | Fade, Rise, Slide, Pop, Zoom, Focus |
| Out | Fade, Sink, Slide, Pop, Zoom, Blur |
| Over the clip | Ken Burns, Pan, Pulse, Float, Shake, Spin |

In and out presets last 0.6 seconds. `clip.animate` has twelve more (slides in every direction,
spin in and out, drop in, Ken Burns out, wiggle…); `kimchi-cli motion.presets` lists them all.

**Clear animation** removes every keyframe from the clip and keeps the values it had at the
playhead.

A title's size, colour and letter spacing can also be keyframed, with `clip.addKeyframe` or
`clip.setKeyframes`; the inspector shows no diamond for them.

## Templates

The **Motion** tab (⌘4) lists the templates, 2D first, then 3D. Click one to add it at the
playhead. Select the clip, and the inspector's **Template** section edits its values: words,
colours, numbers, switches and choices. (Lists, such as a bar chart's values, and pictures are
set with `motion.setTemplate`.)

| 2D template | Length | What you can change |
| --- | --- | --- |
| Lower third | 5 s | Title, subtitle, accent, text colour, plate, side |
| Title card | 4 s | Title, subtitle, accent, text colour, background |
| Kinetic type | 4 s | Text (`*word*` takes the accent colour), colours |
| Counter | 4 s | From, to, decimals, prefix, suffix, label, ring, colours |
| Bar chart | 5 s | Title, values, labels, suffix, colours |
| Logo reveal | 3 s | Text or a picture, colours |
| Callout | 4 s | Label, the point it marks, offset, colours |
| Quote | 6 s | Quote, author, colours |
| Subscribe button | 3.5 s | Label, label once pressed, accent, position |
| Aurora background | 8 s | Colours, background |
| Wipe transition | 1.2 s | Style (slide or circle), colours, direction |
| Glitch title | 4 s | Text, colours |
| Particle burst | 3 s | Text, colours |
| Kinetic sweep | 4 s | Title, subtitle, colours |
| Radial burst | 6 s | Colours, number of rays, background |
| Liquid background | 10 s | Two colours, speed |

| 3D template | Length | What you can change |
| --- | --- | --- |
| 3D title | 4 s | Text, colour, metal, depth, background, floor |
| 3D logo spin | 4 s | Text or a picture, colours |
| Turntable | 6 s | A glTF or GLB model, colours |
| Floating shapes | 8 s | Colours, background |
| Product shot | 6 s | Liquid and cap colours, background, engine (path traced by default) |
| Extruded logo | 5 s | An SVG path, colour, background |
| Particle field | 8 s | Text, colour, background |
| Morphing blob | 8 s | Colour, background |

**New 2D scene** and **New 3D scene**, at the top of the tab, add a 4-second starter scene at the
playhead (text that reveals word by word in 2D; a camera, a turning box and a floor in 3D) and open
it in the Studio. **Ask the agent** opens the Agent panel, which can write a whole scene from a
description and then look at the frames it made.

For quick changes without the Studio, the inspector's **Scene** section picks a layer or object,
edits and keyframes its properties, and adds text, shapes (rectangle, circle, star, line in 2D; box,
sphere, cylinder, torus, plane, 3D text, light in 3D) or the picture or video selected in the Media
tab.

## The Studio

The Studio is a full workspace for one motion clip: like After Effects for 2D scenes and like
Blender for 3D ones.

**Opening and leaving.** Double-click a motion clip, press ⇧⌘O (Ctrl+Shift+O) with it
selected, or use **Open in the Studio** in its menu or the inspector. Only motion clips open in
the Studio. **Back to the edit** or Esc returns to the editor; Esc first closes a menu, cancels a
tool or leaves edit mode, so press it again if needed.

```
┌─────────────────────────── toolbar ────────────────────────────┐
│ outliner │              viewport / canvas          │ properties │
├──────────┴─────────────────────────────────────────┴────────────┤
│                dope sheet / graph editor                        │
└─────────────────────────────────────────────────────────────────┘
```

- **Outliner**: everything in the scene. In 3D: the world, cameras, lights, objects (with their
  children) and materials. In 2D: layers (the top of the list draws on top), groups and
  compositions. Click to select (Shift-click adds), double-click to rename, use the eye to hide,
  drag to reorder or parent, right-click for more (add a child, move out of its parent, set the
  active camera, precompose 2D layers, open a composition).
- **Viewport** (3D) or **canvas** (2D): see and move what you're editing.
- **Properties**: everything about the selection, in tabs. For a 3D object: Item, Modifiers,
  Constraints, Material and Expressions; for a light or camera: Item, Constraints and Expressions.
  For a 2D layer: Item, Effects, Masks, Operators (shapes), Animators (text) and Expressions. With
  nothing selected in 3D: **World** (the environment: colour, gradient, sky or a 360° picture, its
  strength and rotation) and **Render** (the engine, and the look: exposure, tone mapping, bloom,
  ambient occlusion, motion blur).
- **Timeline**: the clip's own keyframes. **Dope sheet** shows them as keys to select (click,
  Shift-click, or drag a box) and drag in time; right-click keys to set their easing or delete them.
  **Graph** shows the curves: drag keys in time and value, and shape a segment's easing with its
  two handles; **Fit the curve** frames them. ⇧⌘G switches between the two, and **Keyframe** keys the
  selection at the playhead.

In narrow windows the outliner and properties leave the row; the toolbar's **Objects** and
**Properties** buttons open them as drawers.

### Adding things

Press ⇧A (or **Add**) and type to search.

- **3D**: meshes (box, sphere, icosphere, cylinder, cone, capsule, torus, plane, grid), 3D text,
  an extruded SVG shape, a lathe (a turned profile), a curve, an empty, particles, an image card
  from the project's pictures, a model file (glTF, OBJ or STL), lights (sun, point, spot, area)
  and a camera placed at the current view.
- **2D**: rectangle, ellipse, polygon, star, line, text, a null (a handle to parent layers to), an
  adjustment layer, a group, particles, compositions, pictures and videos from the project, a
  picture from a file, and a new composition.

Stacks (modifiers, effects, constraints, shape operators, masks, text animators) each have their
own searchable add menu in Properties.

### Keyframes

Press **I** to keyframe the selection at the playhead (position, rotation and scale for 3D
objects; position, rotation, scale and opacity for 2D layers; position and target for cameras),
or click the diamond beside any property in Properties. Space plays the clip in a loop.

### Moving around in 3D

| To | Mouse | Keys |
| --- | --- | --- |
| Orbit | Middle-drag or ⌥-drag; two-finger scroll on a trackpad | |
| Pan | Shift + middle-drag, or Space-drag | |
| Zoom | Scroll (towards the pointer), pinch, or ⌘ + middle-drag | = and − |
| Look from a side | Click an axis on the axis ball (click again for the opposite side) | 1 front, 3 right, 7 top |
| Through the camera | | 0 |
| Perspective / orthographic | | 5 |
| Frame | | . the selection, Home everything |
| Fly | Hold the right button and drag | ⇧\` or ~ |

The **navigation gizmo** in the top right holds the axis ball and buttons to drag for orbit, pan
and zoom, with toggles to fly, look through the camera, lock the camera to the view, switch
perspective and frame.

**Flying**: W, A, S and D move, Q and E go down and up, Shift is faster, the mouse looks around and
the scroll wheel sets the speed. Click, Enter or Space keeps the new view; Esc or a right-click
puts it back.

### The camera

The **Camera** menu in the toolbar (also on a camera's right-click menu) can look through the
camera, **lock the camera to the view** (so moving around moves the camera, writing a keyframe
when it is animated), align the camera to the view (⌥⌘0), and add a camera where you are.

It also writes **camera moves** over the clip, as keyframes, constraints (a path to follow) and
expressions (a handheld wiggle) that you can edit afterwards: orbit around the selection (90°),
turntable (a whole turn), dolly in or out, truck left or right, crane up or down, zoom the lens in,
fly along a selected curve (or a new fly-by path), and a handheld shake. **Clear the camera's
moves** removes all of the camera's animation, keyframes you set yourself included, and leaves it
where it is at the playhead. Moves apply to the selected camera, or else the active one.

### Moving, rotating and scaling

In 3D, pick **Move**, **Rotate** or **Scale** in the toolbar for a gizmo (X red, Y green, Z blue),
or press **G**, **R** or **S** to start at the pointer. While moving: X, Y or Z locks an axis
(Shift+X locks the plane; pressing the axis again uses the object's own axis), typing a number sets
the amount, Ctrl snaps (to the grid, or 15° turns), and Enter or a click confirms while Esc or a
right-click cancels. The toolbar's magnet turns snapping on for every move, and **Global / Local**
sets the gizmo's axes. In 2D, **R** and **S** rotate and scale the selected layer the same way,
while **G** is the pen.

Other keys in the Studio: **⇧D** duplicates, **A** selects all or none, **B** box-selects, **H** hides
the selection and **⌥H** shows everything, **X** deletes, and **W** cycles the tools.

**Shading** in the toolbar switches the view between **Solid**, **Material** and **Rendered** (which
refines the final engine's picture as you wait). Two more toggles show or hide the floor grid and
the lights and cameras in the view.

### Modelling

Select a mesh and press **Tab** for edit mode. Other shapes are converted to a mesh first (their
parametric settings are then gone; **Convert to mesh** on the right-click menu does the same on
purpose).

- **1**, **2** and **3** select vertices, edges or faces.
- **E** extrudes, **I** insets, ⌘B bevels: move the mouse or type an amount.
- **G** moves along the normal, **⌘R** arms a loop cut (then click an edge), **M** merges at the
  centre, **F** fills, **X** deletes or dissolves.
- ⌥N flips and ⇧N recalculates normals.
- The **Mesh** menu (or a right-click) has every operation: extrude, extrude each, inset, bevel,
  subdivide, loop cut, delete, dissolve, merge, fill, bridge, flip and recalculate normals, move,
  rotate, scale, mirror, duplicate, triangulate, poke, smooth, spin, knife and unwrap.

Modifiers (subdivision, mirror, array, bevel, solidify, boolean, displace, twist, bend…) stay
editable and can be animated; **Apply modifiers** bakes them into the mesh.

### The 2D canvas

- **Select** (V): click a layer to select and drag it; drag its corner and edge handles to scale,
  or just outside a corner to rotate; drag on empty canvas to select several.
- **Anchor point** (Y): move the point a layer rotates and scales around.
- **Pen** (P): click for corners, drag for curves, click the first point to close; Enter
  finishes, Backspace removes the last point. With **Drawing a mask** it draws a mask on the
  selected layer instead of a new path layer.
- **Shapes** (Q cycles rectangle, ellipse, star, polygon): drag one out; Shift keeps it square
  or round.
- **Text** (T): click to add text.
- **Zoom and pan**: the mouse wheel, ⌘ (Ctrl) + scroll or a pinch zoom; two fingers on a trackpad,
  Space-drag or middle-drag pan; the controls in the
  bottom right zoom, show 100% (/), and fit (⇧Z).

The composition picker in the toolbar opens a nested composition to edit it.

### Expressions

Any property can be driven by an expression instead of keyframes, in the **Expressions** tab:
`wiggle(2, 30)`, `loopOut("pingpong")`, `time * 90`, `prop("ball", "x") + 100`, or staggering
copies with `index`. `motion.guide` (topic `expressions`) documents the language.

## Rendering ahead

A motion clip is normally drawn **live**: quickly in the preview, and at full quality in the
export. Heavy scenes (path tracing, large blurs, many particles) may not play smoothly live.
**Render** a clip ahead to draw every frame once, at full quality, into a file the timeline then
plays.

- Start it from the clip's right-click menu (**Render now**), the inspector's Motion clip section,
  or the Studio toolbar. The clip shows **Rendering** with its progress; **Cancel render** stops
  it. Meanwhile the clip keeps playing as it did.
- When it finishes, the clip shows **Rendered**. This is an undo step: undoing goes back to live.
- If the scene changes, or the project's size or frame rate, or a picture or model the scene uses,
  the clip shows **Out of date** and is drawn live again until you render it again.
- If you lengthen a rendered clip past what was rendered, the new part is drawn live; render it
  again.
- **Go live (forget the render)** returns to live drawing on purpose.

The rendered file is lossless and keeps transparency; it is stored in the project's cache folder.

## Render engines for 3D

Each 3D scene picks its engine for final frames (exports and renders ahead), with the toolbar's
**Standard / Path tracer** button or Properties › Render › Engine:

- **Standard**, like Blender's Eevee: fast, on the graphics card (Metal on Macs, Vulkan or
  DirectX 12 elsewhere), or on the processor when there is no suitable GPU. It does soft shadows,
  bloom, ambient occlusion and motion blur.
- **Path tracer**, like Blender's Cycles: real reflections, light through glass, soft light and
  bounced light, on the processor. Its **Samples** (default 64), **Bounces** (4) and **Denoise**
  (on) settings trade time for quality. A 960×540 frame of the Product shot template takes about 20
  seconds on a two-core laptop, so render path-traced clips ahead.

The live preview always uses the standard engine, so a path-traced scene looks simpler until it
is rendered.

To force the processor for 3D, start kimchi with `KIMCHI_GPU=0`; see
[Configuration](../CONFIGURATION.md#environment-variables).
