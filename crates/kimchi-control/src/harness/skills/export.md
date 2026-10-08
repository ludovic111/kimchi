# Export for platforms
When: the person asks for the finished file, for a platform (YouTube, TikTok, Instagram, Vimeo, X, LinkedIn), for another editor, or sound only.

## Steps

1. Only when asked: exporting writes a file (the files permission).
2. Before rendering, run the finish routine on the whole cut: `harness.look` (no blank frames, text legible,
   no missing media in `project.overview`'s `problems`, no clips still generating).
3. Pick the preset for the destination (`export.presets`):
   - YouTube: `youtube-1080p` or `youtube-4k` (-14 LUFS). Vimeo: `vimeo` (-16).
   - TikTok, Reels, Shorts: `shorts` (1080×1920, 30 fps; the project should be 9:16 — skill
     `social-vertical`). Instagram feed: `instagram-portrait` (4:5) or `instagram-square`.
   - X: `x`; LinkedIn: `linkedin`; a looping GIF: `gif`.
   - Another editor or finishing: `editor` (ProRes 422 HQ); broadcast: `broadcast` (-23 LUFS); a project
     file for Premiere, Resolve, Final Cut: `project.exportTo {path, app}` instead.
   - Sound only: `podcast` (MP3 -16), `broadcast-audio` (WAV -23), or `format: "wav"`.
4. `export.start {path, preset, wait: true}`; the extension matches the format (.mp4, .mov, .webm, .gif).
   A draft check of a range first: `{quality: "draft", from, to}`. Captions are burned in by default;
   `captions: "file"` writes an .srt beside it instead.
5. Keep the project's frame rate unless the platform needs another.

## Checks

- `export.status`: done, with the file's path; the length matches the cut.
- Tell the person the path, the size and the settings (format, size, frame rate, loudness).
