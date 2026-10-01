<script lang="ts">
  import {
    ArrowRightToLine,
    AudioLines,
    Clapperboard,
    Copy,
    Eye,
    EyeOff,
    Film,
    Lock,
    LockOpen,
    Magnet,
    Minus,
    Plus,
    RefreshCw,
    Scissors,
    Sparkles,
    Trash2,
    Type,
    Volume2,
    VolumeX,
    Wand2,
    Waypoints,
    WrapText,
  } from "@lucide/svelte";
  import type { Clip } from "$lib/bindings/Clip";
  import type { Edge } from "$lib/bindings/Edge";
  import type { Track } from "$lib/bindings/Track";
  import { drag } from "$lib/state/drag.svelte";
  import { editor, MAX_PPS, MIN_PPS } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { ui, type MenuEntry } from "$lib/state/ui.svelte";
  import { short } from "$lib/util/time";
  import IconButton from "../ui/IconButton.svelte";
  import ClipView from "./ClipView.svelte";
  import { addText, animateFrame, bridge, extendClip, regenerate, restyleFrame } from "../editor/actions";

  const HEADER_W = 172;
  const RULER_H = 30;
  const TRACK_H = { video: 66, audio: 50 } as const;
  const SNAP_PX = 8;

  let scroller = $state<HTMLDivElement>();
  let scrollLeft = $state(0);
  let viewW = $state(800);

  const tracks = $derived(editor.project?.tracks ?? []);
  const pps = $derived(editor.pps);
  const contentW = $derived(Math.max(viewW - HEADER_W, (editor.duration + 30) * pps));
  const tops = $derived.by(() => {
    let y = 0;
    return tracks.map((t) => {
      const top = y;
      y += TRACK_H[t.kind] + 4;
      return top;
    });
  });
  const lanesH = $derived(tracks.reduce((h, t) => h + TRACK_H[t.kind] + 4, 0));

  // ---- ruler ---------------------------------------------------------------

  const ticks = $derived.by(() => {
    const steps = [1 / 30, 0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600];
    const major = steps.find((s) => s * pps >= 90) ?? 600;
    const minor = major / (major >= 1 ? 5 : 4);
    const from = Math.max(0, Math.floor(scrollLeft / pps / major) * major);
    const to = (scrollLeft + viewW) / pps;
    const out: { t: number; x: number; major: boolean }[] = [];
    for (let t = from; t <= to + major; t += minor) {
      const isMajor = Math.abs(t / major - Math.round(t / major)) < 1e-6;
      out.push({ t, x: t * pps, major: isMajor });
    }
    return out;
  });

  const tickLabel = (t: number) => {
    if (pps >= 300 && t % 1 !== 0) return `${Math.round((t % 1) * editor.fps)}f`;
    const m = Math.floor(t / 60);
    const s = Math.round(t % 60);
    return m ? `${m}:${String(s).padStart(2, "0")}` : `${s}s`;
  };

  function timeAt(clientX: number) {
    const r = scroller!.getBoundingClientRect();
    return Math.max(0, (clientX - r.left - HEADER_W + scroller!.scrollLeft) / pps);
  }

  function trackAt(clientY: number): Track | undefined {
    const r = scroller!.getBoundingClientRect();
    const y = clientY - r.top - RULER_H + scroller!.scrollTop;
    return tracks.find((t, i) => y >= tops[i] && y < tops[i] + TRACK_H[t.kind] + 4);
  }

  function scrub(e: PointerEvent) {
    if (e.button !== 0) return;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    editor.pause();
    editor.seek(timeAt(e.clientX));
    const move = (ev: PointerEvent) => editor.seek(snapTime(timeAt(ev.clientX), new Set(), true));
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  }

  // ---- snapping ------------------------------------------------------------

  function snapPoints(exclude: Set<string>, withPlayhead = true): number[] {
    const pts = [0];
    if (withPlayhead) pts.push(editor.playhead);
    for (const m of editor.project?.markers ?? []) pts.push(m.time);
    for (const t of tracks) for (const c of t.clips) if (!exclude.has(c.id)) pts.push(c.start, c.start + c.duration);
    return pts;
  }

  let guide = $state<number | null>(null);

  function snapTime(t: number, exclude: Set<string>, scrubbing = false): number {
    if (!editor.snapping) return t;
    let best = t;
    let dist = SNAP_PX / pps;
    for (const p of snapPoints(exclude, !scrubbing)) {
      if (Math.abs(p - t) < dist) {
        dist = Math.abs(p - t);
        best = p;
      }
    }
    return best;
  }

  /** Snaps a moving span by whichever of its edges is closest to a snap point. */
  function snapSpan(start: number, len: number, exclude: Set<string>): number {
    if (!editor.snapping) return start;
    let best = start;
    let dist = SNAP_PX / pps;
    guide = null;
    for (const p of snapPoints(exclude)) {
      for (const [edge, offset] of [
        [start, 0],
        [start + len, len],
      ] as const) {
        if (Math.abs(p - edge) < dist) {
          dist = Math.abs(p - edge);
          best = p - offset;
          guide = p;
        }
      }
    }
    return best;
  }

  // ---- moving & trimming ---------------------------------------------------

  let moving = $state<{ ids: Set<string>; dt: number; trackShift: number } | null>(null);
  let trimming = $state<{ id: string; edge: Edge; time: number } | null>(null);

  function clipDown(e: PointerEvent, clip: Clip, track: Track) {
    if (e.button !== 0) return;
    e.stopPropagation();
    const additive = e.shiftKey || e.metaKey || e.ctrlKey;
    if (!editor.selection.includes(clip.id) || additive) editor.select(clip.id, additive);
    if (track.locked) return;

    const ids = new Set(editor.selection.includes(clip.id) ? editor.selection : [clip.id]);
    const startX = e.clientX;
    const t0 = timeAt(e.clientX);
    const ti0 = tracks.indexOf(track);
    let started = false;

    const move = (ev: PointerEvent) => {
      if (!started && Math.abs(ev.clientX - startX) < 4 && trackAt(ev.clientY)?.id === track.id) return;
      started = true;
      const raw = clip.start + (timeAt(ev.clientX) - t0);
      const snapped = snapSpan(Math.max(0, raw), clip.duration, ids);
      const over = trackAt(ev.clientY);
      const ti = over && over.kind === track.kind && !over.locked ? tracks.indexOf(over) : ti0;
      moving = { ids, dt: snapped - clip.start, trackShift: ti - ti0 };
    };
    const up = async () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const m = moving;
      moving = null;
      guide = null;
      if (!m || (Math.abs(m.dt) < 1e-6 && m.trackShift === 0)) return;
      const moves = tracks.flatMap((t, ti) =>
        t.clips
          .filter((c) => m.ids.has(c.id))
          .map((c) => {
            const dest = tracks[ti + m.trackShift];
            const trackId = dest && dest.kind === t.kind ? dest.id : t.id;
            return { clip_id: c.id, track_id: trackId, start: Math.max(0, c.start + m.dt) };
          }),
      );
      await editor.edit({ op: "move_clips", moves });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function edgeDown(e: PointerEvent, clip: Clip, track: Track, edge: Edge) {
    if (e.button !== 0 || track.locked) return;
    e.stopPropagation();
    editor.select(clip.id);
    const exclude = new Set([clip.id]);
    const move = (ev: PointerEvent) => {
      const t = snapTime(timeAt(ev.clientX), exclude);
      guide = editor.snapping && snapPoints(exclude).some((p) => Math.abs(p - t) < 1e-9) ? t : null;
      trimming = { id: clip.id, edge, time: t };
    };
    const up = async () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const tr = trimming;
      trimming = null;
      guide = null;
      if (tr) await editor.edit({ op: "trim_clip", clip_id: tr.id, edge: tr.edge, time: tr.time });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function geometry(c: Clip, ti: number) {
    let start = c.start;
    let end = c.start + c.duration;
    let row = ti;
    if (moving?.ids.has(c.id)) {
      start += moving.dt;
      end += moving.dt;
      const dest = tracks[ti + moving.trackShift];
      if (dest && dest.kind === tracks[ti].kind) row = ti + moving.trackShift;
    }
    if (trimming?.id === c.id) {
      if (trimming.edge === "start") start = Math.min(trimming.time, end - 1 / editor.fps);
      else end = Math.max(trimming.time, start + 1 / editor.fps);
    }
    return { x: start * pps, w: (end - start) * pps, row };
  }

  // ---- library drops -------------------------------------------------------

  const dropHint = $derived.by(() => {
    if (!drag.active || !drag.target || drag.target.trackId === "") return null;
    const a = editor.assets.get(drag.assetId!);
    const ti = tracks.findIndex((t) => t.id === drag.target!.trackId);
    return ti === -1 ? null : { ti, x: drag.target.time * pps, w: (a?.meta.duration && a.kind !== "image" ? a.meta.duration : 5) * pps };
  });

  $effect(() => {
    if (!drag.active || !scroller) return;
    const r = scroller.getBoundingClientRect();
    const inside = drag.x > r.left + HEADER_W && drag.x < r.right && drag.y > r.top + RULER_H && drag.y < r.bottom;
    const a = editor.assets.get(drag.assetId!);
    if (!inside || !a) {
      if (drag.target && drag.target.trackId !== "") drag.target = null;
      return;
    }
    const want = a.kind === "audio" ? "audio" : "video";
    const over = trackAt(drag.y);
    const len = a.meta.duration && a.kind !== "image" ? a.meta.duration : 5;
    const time = snapSpan(timeAt(drag.x), len, new Set());
    drag.target = over && over.kind === want && !over.locked ? { kind: "track", trackId: over.id, time } : null;
  });

  // ---- follow the playhead -------------------------------------------------

  $effect(() => {
    if (!editor.playing || !scroller) return;
    const x = editor.playhead * pps;
    const visible = scroller.clientWidth - HEADER_W;
    if (x > scroller.scrollLeft + visible - 40) scroller.scrollLeft = x - 60;
  });

  function wheel(e: WheelEvent) {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    const t = timeAt(e.clientX);
    const next = Math.min(MAX_PPS, Math.max(MIN_PPS, editor.pps * Math.exp(-e.deltaY * 0.01)));
    const r = scroller!.getBoundingClientRect();
    editor.pps = next;
    queueMicrotask(() => {
      scroller!.scrollLeft = t * next - (e.clientX - r.left - HEADER_W);
    });
  }

  function fit() {
    const w = (scroller?.clientWidth ?? 800) - HEADER_W - 40;
    editor.pps = Math.min(MAX_PPS, Math.max(MIN_PPS, w / Math.max(5, editor.duration)));
    scroller?.scrollTo({ left: 0 });
  }

  // ---- menus ---------------------------------------------------------------

  function clipMenu(e: MouseEvent, clip: Clip, track: Track) {
    if (!editor.selection.includes(clip.id)) editor.select(clip.id);
    const asset = editor.assetOf(clip);
    const picture = clip.content.type === "media" && asset?.kind !== "audio";
    const sel = editor.selectedClips;
    const items: MenuEntry[] = [
      { label: "Split at playhead", icon: Scissors, shortcut: "S", action: () => editor.splitAtPlayhead() },
      { label: "Duplicate", icon: Copy, shortcut: "⌘D", action: () => editor.duplicateSelection() },
    ];
    if (picture) {
      items.push(
        "separator",
        { label: "Animate this frame", icon: Clapperboard, gen: true, action: () => animateFrame(clip, Math.min(Math.max(editor.playhead, clip.start), clip.start + clip.duration)) },
        { label: "Extend shot with AI", icon: ArrowRightToLine, gen: true, action: () => extendClip(clip) },
        { label: "Restyle frame", icon: Wand2, gen: true, action: () => restyleFrame(clip, Math.min(Math.max(editor.playhead, clip.start), clip.start + clip.duration - 0.01)) },
      );
    }
    if (asset?.origin.type === "generated") items.push({ label: "Regenerate", icon: RefreshCw, gen: true, action: () => regenerate(clip) });
    if (sel.length === 2 && sel.every((c) => editor.trackOf(c.id)?.kind === "video")) {
      items.push({ label: "Bridge with AI", icon: Waypoints, gen: true, action: () => bridge(sel[0], sel[1]) });
    }
    items.push(
      "separator",
      { label: "Delete", icon: Trash2, shortcut: "⌫", danger: true, disabled: track.locked, action: () => editor.deleteSelection(false) },
      { label: "Ripple delete", icon: WrapText, shortcut: "⇧⌫", danger: true, disabled: track.locked, action: () => editor.deleteSelection(true) },
    );
    ui.openMenu(e, items);
  }

  function laneMenu(e: MouseEvent, track: Track) {
    const t = timeAt(e.clientX);
    const before = track.clips.filter((c) => c.start + c.duration <= t).reduce((m, c) => Math.max(m, c.start + c.duration), 0);
    const after = track.clips.filter((c) => c.start >= t).reduce((m, c) => Math.min(m, c.start), Infinity);
    const inGap = Number.isFinite(after) && after > before;
    const start = inGap || before > 0 ? before : t;
    const length = inGap ? after - before : 5;
    const where = inGap ? `in the ${short(length)} gap` : `at ${short(start)}`;
    ui.openMenu(e, [
      {
        label: track.kind === "video" ? "Generate video here…" : "Generate here…",
        icon: Sparkles,
        gen: true,
        disabled: track.kind === "audio",
        action: () => gen.compose({ mode: "video", toTimeline: true, target: { trackId: track.id, start, duration: length, label: where } }),
      },
      {
        label: "Generate image here…",
        icon: Sparkles,
        gen: true,
        disabled: track.kind === "audio",
        action: () => gen.compose({ mode: "image", toTimeline: true, target: { trackId: track.id, start, duration: Math.min(length, 5), label: where } }),
      },
      "separator",
      {
        label: "Add text here",
        icon: Type,
        disabled: track.kind === "audio",
        action: () => {
          editor.seek(t);
          addText();
        },
      },
      { label: "Close gap", icon: WrapText, disabled: !inGap, action: () => editor.edit({ op: "close_gap", track_id: track.id, time: t }) },
    ]);
  }

  function trackMenu(e: MouseEvent, track: Track, i: number) {
    ui.openMenu(e, [
      { label: "Add video track above", icon: Film, action: () => editor.edit({ op: "add_track", kind: "video", index: i }) },
      { label: "Add audio track below", icon: AudioLines, action: () => editor.edit({ op: "add_track", kind: "audio", index: i + 1 }) },
      "separator",
      { label: "Move up", disabled: i === 0, action: () => editor.edit({ op: "move_track", track_id: track.id, index: i - 1 }) },
      { label: "Move down", disabled: i === tracks.length - 1, action: () => editor.edit({ op: "move_track", track_id: track.id, index: i + 1 }) },
      "separator",
      { label: "Delete track", icon: Trash2, danger: true, action: () => editor.edit({ op: "remove_track", track_id: track.id }) },
    ]);
  }

  const toggleTrack = (t: Track, key: "muted" | "hidden" | "locked") => editor.edit({ op: "update_track", track_id: t.id, patch: { [key]: !t[key] } });
</script>

<section class="timeline">
  <div class="toolbar">
    <div class="group">
      <IconButton title="Split at playhead (S)" onclick={() => editor.splitAtPlayhead()}><Scissors size={15} /></IconButton>
      <IconButton title="Delete (⌫)" disabled={!editor.selection.length} onclick={() => editor.deleteSelection()}><Trash2 size={15} /></IconButton>
      <IconButton title="Duplicate (⌘D)" disabled={!editor.selection.length} onclick={() => editor.duplicateSelection()}><Copy size={15} /></IconButton>
      <span class="sep"></span>
      <IconButton title="Snapping (N)" active={editor.snapping} tone="accent" onclick={() => (editor.snapping = !editor.snapping)}><Magnet size={15} /></IconButton>
      <IconButton title="Ripple delete" active={editor.ripple} tone="accent" onclick={() => (editor.ripple = !editor.ripple)}><WrapText size={15} /></IconButton>
      <span class="sep"></span>
      <IconButton title="Add video track" onclick={() => editor.edit({ op: "add_track", kind: "video", index: null })}><Film size={15} /></IconButton>
      <IconButton title="Add audio track" onclick={() => editor.edit({ op: "add_track", kind: "audio", index: null })}><AudioLines size={15} /></IconButton>
      <IconButton title="Add text (T)" onclick={() => addText()}><Type size={15} /></IconButton>
    </div>
    <div class="group">
      <button
        class="gen-here"
        onclick={() => gen.compose({ mode: "video", toTimeline: true, target: { trackId: null, start: editor.playhead, duration: 5, label: "at the playhead" } })}
      >
        <Sparkles size={13} /> Generate at playhead
      </button>
      <span class="sep"></span>
      <IconButton title="Zoom out (−)" onclick={() => (editor.pps = Math.max(MIN_PPS, editor.pps / 1.3))}><Minus size={14} /></IconButton>
      <input class="zoom" type="range" min={Math.log(MIN_PPS)} max={Math.log(MAX_PPS)} step="0.01" value={Math.log(editor.pps)} oninput={(e) => (editor.pps = Math.exp(Number((e.currentTarget as HTMLInputElement).value)))} />
      <IconButton title="Zoom in (+)" onclick={() => (editor.pps = Math.min(MAX_PPS, editor.pps * 1.3))}><Plus size={14} /></IconButton>
      <button class="fit" onclick={fit}>Fit</button>
    </div>
  </div>

  <div class="scroller" bind:this={scroller} bind:clientWidth={viewW} onscroll={() => (scrollLeft = scroller!.scrollLeft)} onwheel={wheel}>
    <div class="content" style="width:{contentW + HEADER_W}px">
      <div class="ruler-row" style="height:{RULER_H}px">
        <div class="corner" style="width:{HEADER_W}px"></div>
        <div class="ruler" style="width:{contentW}px" onpointerdown={scrub} role="slider" aria-label="Playhead" aria-valuenow={editor.playhead} tabindex="-1">
          {#each ticks as tk (tk.t)}
            <span class="tick" class:major={tk.major} style="left:{tk.x}px">{#if tk.major}<i>{tickLabel(tk.t)}</i>{/if}</span>
          {/each}
          {#each editor.project?.markers ?? [] as m (m.id)}
            <button
              class="marker"
              style="left:{m.time * pps}px;--c:{m.color}"
              title="Marker — right-click to remove"
              onpointerdown={(e) => (e.stopPropagation(), editor.seek(m.time))}
              oncontextmenu={(e) => ui.openMenu(e, [{ label: "Remove marker", icon: Trash2, danger: true, action: () => editor.edit({ op: "remove_marker", marker_id: m.id }) }])}
            ></button>
          {/each}
          <span class="knob" style="left:{editor.playhead * pps}px"></span>
        </div>
      </div>

      <div class="body" style="height:{lanesH + 40}px">
        <div class="headers" style="width:{HEADER_W}px">
          {#each tracks as track, ti (track.id)}
            <div class="header {track.kind}" style="top:{tops[ti]}px;height:{TRACK_H[track.kind]}px" oncontextmenu={(e) => trackMenu(e, track, ti)} role="row" tabindex="-1">
              <span class="ticon">{#if track.kind === "video"}<Film size={13} />{:else}<AudioLines size={13} />{/if}</span>
              <span class="tname">{track.name}</span>
              <span class="tbtns">
                {#if track.kind === "video"}
                  <IconButton size={24} title={track.hidden ? "Show" : "Hide"} active={track.hidden} onclick={() => toggleTrack(track, "hidden")}>{#if track.hidden}<EyeOff size={13} />{:else}<Eye size={13} />{/if}</IconButton>
                {/if}
                <IconButton size={24} title={track.muted ? "Unmute" : "Mute"} active={track.muted} onclick={() => toggleTrack(track, "muted")}>{#if track.muted}<VolumeX size={13} />{:else}<Volume2 size={13} />{/if}</IconButton>
                <IconButton size={24} title={track.locked ? "Unlock" : "Lock"} active={track.locked} onclick={() => toggleTrack(track, "locked")}>{#if track.locked}<Lock size={13} />{:else}<LockOpen size={13} />{/if}</IconButton>
              </span>
            </div>
          {/each}
        </div>

        <div class="lanes" style="width:{contentW}px">
          {#each tracks as track, ti (track.id)}
            <div
              class="lane {track.kind}"
              class:locked={track.locked}
              style="top:{tops[ti]}px;height:{TRACK_H[track.kind]}px"
              onpointerdown={(e) => {
                if (e.button !== 0) return;
                editor.clearSelection();
                scrub(e);
              }}
              oncontextmenu={(e) => laneMenu(e, track)}
              role="row"
              tabindex="-1"
            ></div>
          {/each}

          {#each tracks as track, ti (track.id)}
            {#each track.clips as clip (clip.id)}
              {@const g = geometry(clip, ti)}
              <div class="row" style="top:{tops[g.row]}px">
                <ClipView
                  {clip}
                  x={g.x}
                  width={g.w}
                  height={TRACK_H[track.kind]}
                  selected={editor.selection.includes(clip.id)}
                  moving={!!moving?.ids.has(clip.id)}
                  muted={track.muted || track.hidden}
                  onbody={(e) => clipDown(e, clip, track)}
                  onedge={(e, edge) => edgeDown(e, clip, track, edge)}
                  onmenu={(e) => clipMenu(e, clip, track)}
                />
              </div>
            {/each}
          {/each}

          {#if dropHint}
            <div class="drop-hint" style="top:{tops[dropHint.ti]}px;left:{dropHint.x}px;width:{dropHint.w}px;height:{TRACK_H[tracks[dropHint.ti].kind]}px"></div>
          {/if}
          {#if guide !== null}
            <div class="guide" style="left:{guide * pps}px"></div>
          {/if}
          <div class="playhead" style="left:{editor.playhead * pps}px"></div>
        </div>
      </div>
    </div>
  </div>
</section>

<style>
  .timeline {
    min-height: 0;
    display: grid;
    grid-template-rows: 42px minmax(0, 1fr);
    overflow: hidden;
    background: var(--surface);
  }
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 10px;
    border-bottom: 1px solid var(--line);
  }
  .group {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .sep {
    width: 1px;
    height: 16px;
    margin: 0 6px;
    background: var(--line-2);
  }
  .gen-here {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px;
    border-radius: 99px;
    font-size: 12px;
    font-weight: 550;
    color: var(--text-2);
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line-2);
  }
  .gen-here :global(svg) {
    color: var(--amber);
  }
  .gen-here:hover {
    color: var(--text);
    box-shadow: inset 0 0 0 1px rgba(255, 138, 61, 0.45);
  }
  .zoom {
    -webkit-appearance: none;
    appearance: none;
    width: 100px;
    height: 3px;
    border-radius: 3px;
    background: var(--surface-4);
  }
  .zoom::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 12px;
    height: 12px;
    border-radius: 99px;
    background: var(--text-2);
  }
  .fit {
    margin-left: 4px;
    height: 24px;
    padding: 0 8px;
    border-radius: 6px;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .fit:hover {
    color: var(--text);
    background: var(--hover);
  }
  .scroller {
    position: relative;
    overflow: auto;
    min-height: 0;
  }
  .content {
    position: relative;
    min-height: 100%;
  }
  .ruler-row {
    position: sticky;
    top: 0;
    z-index: 30;
    display: flex;
    background: var(--surface);
    border-bottom: 1px solid var(--line);
  }
  .corner {
    position: sticky;
    left: 0;
    z-index: 2;
    flex: none;
    background: var(--surface);
    border-right: 1px solid var(--line);
  }
  .ruler {
    position: relative;
    flex: none;
    cursor: text;
    overflow: hidden;
  }
  .tick {
    position: absolute;
    bottom: 0;
    width: 1px;
    height: 5px;
    background: var(--line-3);
  }
  .tick.major {
    height: 10px;
    background: var(--text-4);
  }
  .tick i {
    position: absolute;
    left: 5px;
    bottom: 9px;
    font-style: normal;
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .marker {
    position: absolute;
    bottom: 0;
    width: 10px;
    height: 12px;
    margin-left: -5px;
    background: var(--c);
    clip-path: polygon(0 0, 100% 0, 100% 60%, 50% 100%, 0 60%);
    z-index: 2;
  }
  .knob {
    position: absolute;
    bottom: 0;
    width: 13px;
    height: 16px;
    margin-left: -6.5px;
    background: var(--accent);
    clip-path: polygon(0 0, 100% 0, 100% 62%, 50% 100%, 0 62%);
    pointer-events: none;
    z-index: 3;
  }
  .body {
    position: relative;
    display: flex;
  }
  .headers {
    position: sticky;
    left: 0;
    z-index: 20;
    flex: none;
    background: var(--surface);
    border-right: 1px solid var(--line);
  }
  .header {
    position: absolute;
    left: 0;
    right: 0;
    margin-top: 2px;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 0 6px 0 12px;
  }
  .ticon {
    color: var(--text-3);
    display: grid;
  }
  .tname {
    flex: 1;
    font-size: 12px;
    font-weight: 550;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tbtns {
    display: flex;
    opacity: 0.55;
    transition: opacity 0.15s;
  }
  .header:hover .tbtns {
    opacity: 1;
  }
  .lanes {
    position: relative;
    flex: none;
  }
  .lane {
    position: absolute;
    left: 0;
    right: 0;
    margin-top: 2px;
    border-radius: 8px;
    background: rgba(255, 240, 225, 0.018);
  }
  .lane.audio {
    background: rgba(166, 207, 94, 0.025);
  }
  .lane.locked {
    background: repeating-linear-gradient(135deg, rgba(255, 255, 255, 0.025) 0 6px, transparent 6px 12px);
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    margin-top: 2px;
    pointer-events: none;
  }
  .row :global(.clip) {
    pointer-events: auto;
  }
  .drop-hint {
    position: absolute;
    margin-top: 2px;
    border-radius: 7px;
    background: rgba(255, 90, 54, 0.14);
    box-shadow: inset 0 0 0 1.5px var(--accent);
    pointer-events: none;
  }
  .guide {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--amber);
    z-index: 30;
    pointer-events: none;
  }
  .playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1.5px;
    margin-left: -0.75px;
    background: var(--accent);
    z-index: 25;
    pointer-events: none;
    box-shadow: 0 0 12px rgba(255, 90, 54, 0.45);
  }
</style>
