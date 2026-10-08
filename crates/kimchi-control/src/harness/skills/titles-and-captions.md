# Titles and captions
When: the person wants a title, lower thirds, on-screen text, or captions/subtitles of what is said.

## Steps

1. `project.overview` for the canvas size and length. Text goes on a video track above the picture
   (`track.add {kind: "video"}` puts a new one on top).
2. **A title**: `clip.addText {text, start, duration, style, y}` or a template: `motion.addTemplate
   {template: "titleCard", values: {title, subtitle}}` (animated), `kineticType` for punchy words.
   Style: `{fontSize, fontWeight, color, background, shadow, fontFamily}`; `app.fonts` lists families.
   Size: 90–160 px titles, at least 1/20 of the frame height for body text. Fade in and out
   (`clip.update {fadeIn: 0.3, fadeOut: 0.3}`) or `clip.animate {preset: "riseIn"}`.
3. **Lower thirds** (names, places): `motion.addTemplate {template: "lowerThird", values: {title,
   subtitle}, start}` a second after the person appears, 4–5 s, one at a time.
4. **Captions from speech**: `captions.transcribe {language}` listens to the mix with Whisper on this
   computer (a model downloads the first time; `captions.status` follows it) and fills the captions track.
   From a script or a file: `captions.import {path}` (SRT, WebVTT) or `captions.add {text, start, duration}`
   per line, in one `project.batch`. Never invent words someone says: ask for them.
5. Style the captions together: `captions.setStyle {style: {fontSize: 54, background: "#000000"}, y}`;
   42 characters a line, two lines at most, each on screen 1–6 s, not overlapping each other.
6. Spelling: copy names exactly as given.

## Checks

- Reading time: each text clip lasts at least 2 s and about 0.3 s per word plus 1 s.
- `project.renderFrame` at the middle of each title and a few captions: inside the safe area (5 % margins;
  vertical video: away from the top 10 % and bottom 20 %), readable against the picture, not covering a face.
- `captions.list`: no overlaps, no empty captions, times inside the cut.
