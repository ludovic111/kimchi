# Scoring with ryolune
When: the cut needs music written or arranged for it, or the person wants to score, re-score or add a soundtrack made in ryolune (lsuite's music app).

## Steps

1. `handoff.apps` says whether ryolune is installed and running. Not installed: say so; offer music from
   the person's files instead (`media.import`, `audio.importSong` for a `.ryolune` song).
2. Mark the beats the music should hit: `timeline.addMarker {time, label}` at each section change, the
   climax and the end ("Drop", "Title", "End"). ryolune gets them with the cut.
3. Send the cut: `handoff.toRyolune {name}` renders the cut's sound and writes its length and markers next
   to it; with ryolune running it is imported there with the markers. `as: "session"` sends the whole
   audio timeline as a multitrack ryolune session instead (one track per kimchi track).
4. The score is made in ryolune (by the person, or by an agent driving ryolune's own commands): matching
   the cut's length, hitting the markers. Tell the person what to do there if no agent drives ryolune.
5. Bring it back: `handoff.fromRyolune {start: 0}` puts a fresh bounce of ryolune's open song on an audio
   track (or `path` to a file ryolune exported); `audio.importSong {path, as: "stems"}` brings a `.ryolune`
   song as stems that kimchi re-renders when the song changes (`audio.refreshSongs`).
6. Mix it: music under dialogue (`audio.autoDuck`), fades at the ends, the master at the delivery loudness
   (skill `audio-mix`).

## Checks

- The music clip starts at 0 (or where asked) and covers the cut: compare its end with the cut's length.
- `audio.measure` over the whole cut and over the last seconds: it ends with the picture (a fade or a
  final hit, not a cut-off).
- Markers and music hits line up: `audio.detectBeats` on the music; the main cuts fall on beats.
