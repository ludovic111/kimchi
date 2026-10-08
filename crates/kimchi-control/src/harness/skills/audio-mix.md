# Audio mix for video
When: the person wants the sound fixed, balanced, louder, ready to publish, or music under a voice.

## Steps

1. `audio.overview` (tracks, buses, master) and `audio.measure` (the whole mix), then `audio.measure
   {trackId}` per track: know what is speech, music and effects, and how loud each is.
2. **Dialogue** first: `audio.normalize {clipIds, target: -16}` brings speech clips to the same loudness.
   Clean it if needed: `audio.addEffect` (`audio.effects` lists them: EQ, compressor, de-esser, gate…),
   e.g. a high-pass at 80 Hz on voices.
3. **Music** under speech: `audio.autoDuck {amountDb: -12}` lowers the music while someone talks (music
   tracks are guessed from their names and content; name them with `track.update`). Without speech, music
   carries the piece at full level.
4. **Effects and ambience** below the voice; pan with `audio.setClip {pan}` when they come from a side.
5. Edges: fade every sound clip's ends (`audio.setClip {fadeIn: 0.05, fadeOut: 0.05}` at least, 1–3 s for
   music), so cuts don't click and music doesn't stop dead. Crossfade with a transition on cuts.
6. **Master**: `audio.setMaster {loudness: -14, limiter: true, ceilingDb: -1}` for web and social
   (-16 podcasts and Vimeo, -23 broadcast). Exports are brought to that loudness; the limiter keeps true
   peaks under the ceiling.

## Checks

- `audio.measure` on the whole mix: `integrated` within 1 LU of the target, or `target` set to it on the
  master (exports reach it), `truePeak` ≤ -1 dBTP.
- `audio.measure {trackId}` on the music during speech: 12 dB or more below the dialogue.
- No silent hole where sound is expected (`audio.measure {from, to}` on short spans).
