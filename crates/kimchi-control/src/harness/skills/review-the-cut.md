# Review the cut
When: the person asks what is wrong with the cut, for notes, a quality check before export, or to fix whatever needs fixing.

## Steps

1. `project.overview`: read `problems` first (missing files, clips still generating, hidden or muted tracks
   with clips).
2. `harness.look` over the whole cut: framing, exposure, blank frames, text. Zoom into suspicious moments
   with `project.renderFrame {times}`.
3. Go through the quality bar:
   - Picture: gaps on the main track (`timeline.closeGap`), jump cuts, shots too long or too short for
     their content, inconsistent colour between shots of a scene.
   - Text: spelling, reading time, safe area, contrast, overlaps.
   - Sound: `audio.measure` (integrated, true peak), clips much louder or quieter than their neighbours,
     music not ducked under speech, hard starts and stops (no fades).
   - Motion: entrances and exits clean, nothing left half on screen at a clip's end.
   - The ending: a held last shot or a fade, the music resolved.
4. Without "fix it": list each issue with its time and what you would do, most important first.
   With "fix it": fix them, grouping related edits in `project.batch`, and look again.

## Checks

- After fixes: `project.overview` `problems` empty (or explained), `harness.look` clean, `audio.measure`
  on target.
- The report lists what changed and anything left for the person to decide.
