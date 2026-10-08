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
- Only a few jobs have been run so far (cheap checks with Sonnet); the full set of 12 is still to
  run with the release model.

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
