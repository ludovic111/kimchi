# kimchi eval results

Runs of `evals/run.py` (lsuite HARNESS.md part 7), newest first. Each job passes when all its
checks pass; `looked` is the finish routine (a look at the work after the last change). Run the
whole set before a release: a harness change that lowers the pass rate doesn't ship.

Notes:

- 2026-10-08: Claude Code shows an MCP tool result's `structuredContent` instead of its text when
  both are there, so the `<context>` updates kimchi-mcp appended to the text never reached it.
  The notes (context, and now a finish-routine reminder after an edit until the agent looks) also
  ride in `structuredContent.harnessNotes`. Before: in an unrecorded run of 2026-10-07 (claude-code
  · sonnet, cut short after 7 jobs), `dissolves` made its edit and reported "I haven't rendered
  frames to check" (`clip.list`, `transition.set`). After: `transition.set`, `harness.look`, pass.
- 2026-10-08, release check for 0.11.0 (Opus): 11/12 on the full set. The one failure,
  `music-ends-with-picture`, was the scorer's: the agent checked its fade with `audio.measure`
  (the right check for a sound-only job, "look and listen" in the brief), but `looked` only counted
  pictures. Sound jobs now count a measurement after the last change too (`looked {sound: true}`);
  re-run with both sound jobs, 2/2 (the first entry below). The first attempt at the full set had
  stopped on `youtube-loudness`'s setup (media names keep their extension: `music-quiet.wav`); a
  setup error is now one failed job, not a stopped run.

## 2026-10-08 · claude-code · opus · 2/2 passed

| Job | Result | Commands | Time | Failed checks |
| --- | --- | --- | --- | --- |
| youtube-loudness | pass | 8 | 27 s | — |
| music-ends-with-picture | pass | 4 | 19 s | — |

## 2026-10-08 · claude-code · opus · 11/12 passed

| Job | Result | Commands | Time | Failed checks |
| --- | --- | --- | --- | --- |
| title-card | pass | 3 | 16 s | — |
| rough-cut | pass | 8 | 67 s | — |
| lower-third | pass | 4 | 19 s | — |
| social-vertical | pass | 4 | 15 s | — |
| warm-grade | pass | 5 | 19 s | — |
| dissolves | pass | 4 | 17 s | — |
| youtube-loudness | pass | 10 | 34 s | — |
| music-ends-with-picture | FAIL | 4 | 11 s | looked: no look after the last change |
| chapter-markers | pass | 4 | 12 s | — |
| 3d-title | pass | 3 | 13 s | — |
| ken-burns-photo | pass | 6 | 16 s | — |
| trim-to-length | pass | 3 | 29 s | — |

## 2026-10-08 · claude-code · sonnet · 2/2 passed

| Job | Result | Commands | Time | Failed checks |
| --- | --- | --- | --- | --- |
| title-card | pass | 3 | 21 s | — |
| dissolves | pass | 2 | 22 s | — |

## 2026-10-07 · claude-code · sonnet · 2/2 passed

| Job | Result | Commands | Time | Failed checks |
| --- | --- | --- | --- | --- |
| title-card | pass | 3 | 14 s | — |
| rough-cut | pass | 9 | 33 s | — |
