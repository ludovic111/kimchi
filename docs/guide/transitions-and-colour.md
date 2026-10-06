# Transitions and colour

## Transitions

A transition belongs to the clip it leads into: it describes how that clip comes in.

**On a cut** (the previous clip on the track ends where this one starts), the transition is
centred on the cut. Both clips play on past it, using the media beyond their in and out points
(or holding their first or last frame when there is none), so nothing moves on the timeline.
**With nothing right before the clip**, it comes in over whatever is on the tracks below,
starting at its own start.

### Adding one

- Hover a cut and click the **+** to add a dissolve. (The + also shows when a clip at the cut is
  selected; it doesn't appear on locked tracks.)
- Or pick a kind in the inspector's **Transition in** section.
- Or right-click the clip: **Dissolve from the previous clip** (on a cut), **Dissolve in**
  (otherwise), or **Crossfade in** on an audio track.

The transition appears as a badge over its span. Drag either edge of the badge to change its
length, and right-click it to switch to another kind or remove it.

### Kinds

| Kind | What it does |
| --- | --- |
| Dissolve | Blends one picture into the other. |
| Dip to black, Dip to white | Fades out to a colour, then in from it. |
| Wipe left, right, up, down | An edge sweeps across, revealing the new clip. |
| Slide left, right, up, down | The new clip slides in over the old one. |
| Push left, right, up, down | The new clip pushes the old one out of the frame. |
| Zoom | The new clip grows in from 60%. |
| Iris | A circle opens from the centre. |
| Blur | Blurs out of one and into the other. |

Audio tracks have a single kind, **Crossfade**.

### Length

New transitions last 0.8 seconds; the inspector's **Length** goes from 0.05 to 30 seconds. On a
cut, a transition can't be longer than the shorter of the two clips; with nothing before the
clip, it can't be longer than half the clip. When the clips are too short for the length you
set, the transition plays shorter and the inspector says so ("Plays for 0.60 s: the clips are too short
for more.").

Transitions ease in and out (a sine curve). A different easing can be set with
`transition.set --easing`; see [Keyframes](motion.md#keyframes-on-any-clip) for the names.

### Sound

The two clips' sound crossfades over the same span as the picture. The crossfade is equal-power, so
the level doesn't dip in the middle, unless a clip has a fade shape other than the default in its
Audio section; each clip's shape is used for its own side. A transition with nothing before it fades
the clip's sound in.

## Colour

The inspector's **Colour** section grades a picture clip: video, images, titles, solids and
motion clips. Clips with colour effects show a palette badge on the timeline.

### Looks

A look sets several corrections at once; adjust the sliders afterwards. **None** returns every
correction to neutral (it keeps the chroma key and the LUT).

| Look | Character |
| --- | --- |
| Punchy | More contrast and saturation, a little sharpening |
| Warm | Warmer, slightly more saturated and brighter |
| Cool | Cooler, a little more contrast |
| Mono | Black and white with extra contrast |
| Faded | Lower contrast and saturation, lifted |
| Vintage | Warm, slightly magenta, faded and desaturated, with a vignette |
| Noir | Black and white, hard contrast, a little darker, strong vignette |
| Teal & orange | Slightly warmer and greener, more contrast and saturation |
| Dreamy | Brighter and softer, slightly magenta |

### Corrections

| Slider | Range | Effect |
| --- | --- | --- |
| Bright | −1 to 1 | Brightness |
| Contrast | −1 to 1 | Contrast |
| Saturate | −1 to 1 | Saturation (−1 is black and white) |
| Warmth | −1 to 1 | Colour temperature, cool to warm |
| Tint | −1 to 1 | Green to magenta |
| Vignette | 0 to 1 | Darkens the corners |
| Sharpen | 0 to 1 | Sharpening |

All start at 0. Double-click a slider to reset it; the arrow keys nudge it by a hundredth of its
range, with Shift by a tenth. Every correction can be keyframed with the diamond beside it (see
[Keyframes on any clip](motion.md#keyframes-on-any-clip)).

### Green screen

Turn on **Key out a colour (green screen)** to make a colour transparent so the tracks below show
through.

| Control | Default | Effect |
| --- | --- | --- |
| Colour | #00b140 | The colour to remove |
| Range | 50% | How far from that colour still counts as it |
| Edge | 10% | How soft the edge of the key is |
| Spill | 50% | How much of the colour's cast is removed from what remains |

There is no eyedropper yet: type the colour. The default is a standard green screen.

### LUTs

**Load a LUT (.cube)…** applies a 3D lookup table, with a **Strength** from 0 to 100%. kimchi reads
3D `.cube` files with sizes from 2 to 256 and honours `DOMAIN_MIN` and `DOMAIN_MAX`; 1D LUTs are
refused. The project refers to the file where it is: if the file is moved, the inspector shows it
as "(missing)", and the clip plays and exports without it.

The section's reset button removes every colour effect, the key and the LUT, along with their
keyframes.
