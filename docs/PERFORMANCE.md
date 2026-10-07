# Performance

## The window at rest, 0.10.0 (2026-10-07)

The 0.9.1 release (`kimchi_amd64.deb`) and a release build of 0.10.0, each started on the same
virtual screen (sway, 1600×1000) with empty data folders and no setup: time until the window
shows, then the app's own CPU over 30 s of rest 10 s after, and its resident memory on Home. The
machine was shared with other builds, so the times vary from run to run; the table keeps each run.

| | Start to window | CPU at rest | Memory |
| --- | ---: | ---: | ---: |
| 0.9.1 | 0.54 s, 0.61 s, 2.06 s | 0.70 %, 0.50 %, 0.56 % | 202–207 MB |
| 0.10.0 | 1.93 s, 1.50 s | 0.43 %, 0.43 % | 214–215 MB |

The start times are dominated by the shared machine (the same 0.9.1 binary took from 0.5 to 2 s).
At rest 0.10.0 wakes less (the captions panel no longer checks four times a second when nothing
is transcribing). It holds about 10 MB more: the stock SDK plugins, the plugin catalogue and the
lsuite account. The first start also scans the plugin folders in child processes (158 frei0r
plugins: 6.8 s of wall time in the background, 2.4 s of CPU); later starts read the cache (0.5 s).
How: start the app, poll the compositor until its window shows,
sample `/proc/<pid>/stat` and `VmRSS`.

# Renderer measurements for 0.8.0

The `render_bench` example exercises compositing, motion effects, 3D, path tracing,
video export, playback and seeking. It reports throughput; preview throughput is
uncapped and is not the display's refresh rate.

## Results (2026-10-05)

Saved release baseline from before the renderer changes (`6f1ebd7d`), compared with
the integrated 0.8.0 renderer. Both used the same media, CPU backend, warm-up frame
and `--frames 0.25`, and ran sequentially after the build and test jobs finished.
This is a shared host: unrelated background work ran during part of the updated
3D measurements. The table retains those timings and reports CPU work alongside
them; small wall-clock differences are not reliable speedup estimates.

| Scene | Frames | Wall ms/frame, baseline → update | CPU ms/frame, baseline → update |
| --- | ---: | ---: | ---: |
| cut 1920x1080 | 19 | 222.9 → 88.2 | 213.7 → 149.5 |
| blur2d liquidBackground 1920x1080 | 10 | 1474.6 → 460.4 | 3322.0 → 718.0 |
| glow2d radialBurst 1920x1080 | 10 | 27.5 → 40.9 | 45.0 → 47.0 |
| 3d logoExtrude 1920x1080 | 5 | 454.3 → 892.3 | 1234.0 → 1264.0 |
| 3d productShot 1920x1080 | 5 | 5038.2 → 6322.3 | 19018.0 → 11790.0 |
| path productShot 960x540 | 1 | 16985.3 → 20609.9 | 66060.0 → 60930.0 |
| export 20 s 1920x1080 (libx264) | 500 | 111.2 → 91.8 | 349.8 → 322.7 |
| preview stream 1280x720 | 37 | 61.7 → 23.4 | 95.9 → 77.8 |
| scrub 1280x720 (single frames) | 3 | 406.3 → 256.8 | 1220.0 → 1073.3 |

The clearest improvements are compositing and the blur-heavy template. CPU work
fell by about 30% and 78%, respectively. The product scene used about 38% less CPU
work. Glow and the extruded logo used essentially the same CPU work in this pass;
the quieter repeat below helps separate contention from renderer cost. There is
no single speedup multiplier for every scene. Export completed in 45.88 s versus 55.60 s.
Preview startup was 238 ms versus 375 ms; these short samples are indicative only.
CPU accounting includes the renderer and finished FFmpeg child processes.

A sequential repeat of the three scenes with apparent regressions, after the
unrelated work subsided, produced:

| Scene | Frames | Wall ms/frame, baseline → update | CPU ms/frame, baseline → update |
| --- | ---: | ---: | ---: |
| Glow | 10 | 22.9 → 21.1 | 41.0 → 35.0 |
| Extruded logo | 5 | 434.4 → 392.3 | 1204.0 → 1104.0 |
| Standard product scene | 5 | 4825.5 → 2991.3 | 18292.0 → 11048.0 |

The repeat shows why the first wall-clock 3D numbers alone would be misleading.
Both runs remain here rather than selecting only the fastest measurements.

Saved cut, logo and glow frames were identical. Mean per-channel differences for
the blur-heavy and standard product scenes were below 0.004 on the 0–255 scale;
the path-traced scene stayed below 0.13 and seeking below 0.000002. Transparency
matched. These samples supplement the renderer's existing image tests.

## Reproduce

Build once, then run the binary without other builds or tests competing for the CPU:

```sh
cargo build --release -p kimchi-media --example render_bench
KIMCHI_GPU=0 KIMCHI_BENCH_MEDIA=/path/to/media \
  target/release/examples/render_bench --frames 0.25 --dump /tmp/kimchi-bench
```

The optional media directory contains `city.mp4` and `fractal.mp4` (1080p).
Without it, the example creates test videos. Use the same files for both versions.
`--frames` scales the sample counts; export always processes a full 20-second cut.
The benchmark also writes representative PNGs when `--dump` is supplied.

## Validation and limits

The 0.8.0 integration was checked on Linux with an Intel i7-7500U (two physical
cores, four threads). CPU and Vulkan/llvmpipe renders passed the image comparison
suite, including point/spot/area lighting and Studio overlays. llvmpipe is a
software adapter: those results validate the GPU code path, not hardware GPU speed.
Metal, DirectX and physical audio-device checks remain platform-specific work.

The preview, render-ahead cache and export integration tests use real FFmpeg
processes. The audio mixer is now shared with exports, so comparisons with the
old version include that architectural change, not only renderer optimisations.

Final validation: `cargo test --workspace --locked -- --test-threads=2` completed
with 672 passing tests, 23 intentionally ignored tests and no failures;
`cargo clippy --workspace --all-targets --locked -- -D warnings` passed.
The `gpu_matches_cpu` test was also run explicitly with `KIMCHI_GPU=any` and passed.
The app was inspected on the virtual screen in light and dark themes, including
640×480, 800×600, 1200×800 and 1600×1000 layouts.
