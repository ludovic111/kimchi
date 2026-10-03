//! The expressions section of `motion.guide`.

/// Markdown for people and agents: what a formula can say, with examples to copy.
pub const GUIDE: &str = r##"## Expressions

An expression computes a property every frame from a formula, like After Effects expressions or
Blender drivers. Put them in a layer's, object's, light's or camera's `expressions`, by property
name (the same names keyframes use):

```json
{"id": "logo", "type": "text", "text": "kimchi",
 "expressions": {"rotation": "wiggle(1.5, 4)", "opacity": "linear(time, 0, 0.5, 0, 1)"}}
```

The formula runs after the keyframes (`value` is the keyframed value) and, in 3D, before the
constraints (a `lookAt` still has the last word). If a formula fails while playing (a `prop()` that
goes round in a circle, a division by zero), the property keeps its keyframed value and the log
says why.

### The language

JavaScript-like and small: numbers, vectors `[x, y]` / `[x, y, z]`, text in quotes (colours like
`"#ff5a36"`), `true`/`false`.

- Maths: `+ - * / %`, powers `2 ** 3` or `2 ^ 3`, comparisons `< <= > >= == !=`, `&& || !`,
  `cond ? a : b`, brackets.
- Vectors work element by element: `[1, 2] * 3` is `[3, 6]`, `value + [0, 10]` moves 10 down.
  Read a part with `v.x`, `v.y`, `v.z` (or `v[0]`). A vector inside `[…]` spreads out:
  `[prop("ball", "position"), 0]` is a 3D point.
- Steps: `let a = time * 2; let b = a % 1; b * 100` — the last step is the value.
- Text: `"Score " + round(time * 10)`; `x.toFixed(1)` shows one decimal. A number given to a text
  property becomes its digits.
- `Math.sin`, `Math.PI` and other JavaScript habits work too. Comments: `// …` and `/* … */`.
- No loops, no functions of your own, no `if` blocks (use `a ? b : c`).

### Names

| name | what |
|---|---|
| `time` (or `t`) | seconds: the scene's, or the composition's inside one |
| `value` | the property's keyframed value now |
| `index` | the layer's (object's) position among its siblings, from 1 |
| `fps`, `frame` | frames per second and the frame number |
| `duration` | the scene's (composition's) length in seconds |
| `velocity`, `speed` | how fast the keyframed value changes (per second), and its size |
| `numKeys` | how many keyframes the property has |
| `pi`, `e`, `seed` | constants; `seed` is a number different for every property |

### Functions

- Maths: `sin cos tan asin acos atan atan2(y, x) sqrt pow exp log abs sign floor ceil round(x, decimals)
  trunc fract min max clamp(x, lo, hi) mod(a, b)` (angles in radians; `deg(r)` and `rad(d)` convert).
  One-number functions work on each part of a vector.
- Vectors: `length(v)`, `length(a, b)`, `distance(a, b)`, `normalize(v)`, `dot(a, b)`, `cross(a, b)`.
- Mapping: `linear(t, tMin, tMax, from, to)` maps `t` from one range to another (clamped);
  `ease`, `easeIn`, `easeOut` do the same with After Effects' curves; the 3-argument forms take
  `t` from 0 to 1 (`ease(t, from, to)`). `from`/`to` may be numbers, vectors or colours.
  Also `smoothstep(e0, e1, x)`, `step(edge, x)`, `mix(a, b, amount)` (or `lerp`).
- Randomness, the same every time the frame is drawn: `random()` (0–1), `random(max)`,
  `random(min, max)` (vectors too), `gaussRandom(…)` (bell-shaped), `seedRandom(n)` for another
  sequence, `seedRandom(n, true)` for one that doesn't change over time; `noise(x)`, `noise(x, y)`,
  `noise(x, y, z)` (smooth, −1..1).
- `wiggle(freq, amp, octaves = 1, ampMult = 0.5, t = time)`: smooth random motion around `value`,
  `freq` times a second, about `amp` away (numbers, vectors and colours).
- Keyframes: `loopOut(type = "cycle", numKeyframes = 0)` repeats them after the last one (`"cycle"`,
  `"pingpong"`, `"offset"` adds up each round, `"continue"` keeps going at the last speed);
  `loopIn(…)` does the same before the first. `valueAtTime(t)`, `velocityAtTime(t)`,
  `keyTime(n)`, `keyValue(n)` (from 1; also `key(n).time`, `key(n).value`).
- Other things: `prop("id", "name")` reads another layer's, object's, light's or camera's property
  (`"camera"`, `"scene"` too) with its keyframes and its own expression applied (before
  constraints, in its parent's space); `prop("id", "name", t)` at another time. Up to 8 deep; a
  circle (a reads b, b reads a) is an error.
- Colours: `rgb(255, 90, 54)`, `rgba(r, g, b, alpha 0–1)`, `hsl(hue°, sat 0–1, light 0–1)`, `hsla`,
  `hex([r, g, b])` (0–1 channels), `hexToRgb("#ff5a36")` → `[r, g, b, a]` 0–1. A colour property
  also takes `[r, g, b]` with 0–1 channels.
- Time: `posterizeTime(8)` makes the rest of the formula see time in steps of 1/8 s (stop-motion);
  `timeToFrames(t)`, `framesToTime(n)`.

### Examples

```js
wiggle(2, 30)                                  // shake: 2 times a second, about 30 px
value + [0, sin(time * 2 * pi) * 20]           // bob up and down once a second
time * 90                                      // turn 90° a second ("rotation")
loopOut()                                      // repeat the keyframes forever
loopOut("pingpong")                            // back and forth
loopOut("offset")                              // keep going: each round starts where the last ended
prop("ball", "position") + [0, -80]            // follow another layer, 80 px above it
valueAtTime(time - (index - 1) * 0.1)          // stagger: each layer 0.1 s after the one before
linear(time, 0, 2, 0, 1000).toFixed(0)         // a counter from 0 to 1000 over 2 s ("text")
round(linear(time, 0, 2, 0, 1000))             // the same, for a text layer's "value"
100 * exp(-3 * time) * sin(time * 18)          // a decaying spring (add to a position)
posterizeTime(6); wiggle(3, 10)                // stop-motion jitter
hsl(time * 60, 0.8, 0.55)                      // cycle through the colours ("fill", "color")
mix("#ff5a36", "#3a7bff", (sin(time * 3) + 1) / 2)
time < 1 ? 0 : (time - 1) * 360                // wait a second, then spin
let p = prop("ship", "position"); [p.x, p.y + noise(time * 2) * 0.2, p.z]   // 3D: hover near the ship
valueAtTime(duration - time)                   // play the keyframes backwards
```
"##;
