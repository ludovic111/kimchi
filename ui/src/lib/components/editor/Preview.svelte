<script lang="ts">
  import { Pause, Play, Repeat, SkipBack, SkipForward, Sparkles, StepBack, StepForward } from "@lucide/svelte";
  import { fileUrl } from "$lib/ipc";
  import type { Clip } from "$lib/bindings/Clip";
  import type { Track } from "$lib/bindings/Track";
  import type { Transform } from "$lib/bindings/Transform";
  import { drag } from "$lib/state/drag.svelte";
  import { editor } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { fitBox, opacityAt, type Box } from "$lib/util/layout";
  import { measure } from "$lib/util/text";
  import { timecode } from "$lib/util/time";
  import IconButton from "../ui/IconButton.svelte";
  import MediaSync from "./MediaSync.svelte";
  import TextLayer from "./TextLayer.svelte";

  const W = $derived(editor.project?.settings.width ?? 1920);
  const H = $derived(editor.project?.settings.height ?? 1080);

  let host = $state<HTMLDivElement>();
  let hostW = $state(800);
  let hostH = $state(450);
  const scale = $derived(Math.max(0.05, Math.min((hostW - 48) / W, (hostH - 40) / H)));

  /** Live transform while dragging on the canvas, committed on release. */
  let override = $state<{ id: string; t: Transform } | null>(null);
  const measureCtx = document.createElement("canvas").getContext("2d")!;

  interface Layer {
    clip: Clip;
    track: Track;
    t: Transform;
    box: Box;
    visible: boolean;
  }

  const layers = $derived.by(() => {
    const p = editor.project;
    if (!p) return [] as Layer[];
    const ph = editor.playhead;
    const out: Layer[] = [];
    // Bottom track first so later layers paint on top.
    for (const track of [...p.tracks].reverse()) {
      if (track.kind !== "video" || track.hidden) continue;
      for (const clip of track.clips) {
        // Mount a little early so videos are buffered when they come in.
        if (ph < clip.start - 1.5 || ph > clip.start + clip.duration + 0.25) continue;
        const t = override?.id === clip.id ? override.t : clip.transform;
        out.push({ clip, track, t, box: boxOf(clip, t), visible: ph >= clip.start && ph < clip.start + clip.duration });
      }
    }
    return out;
  });

  const audioClips = $derived(
    (editor.project?.tracks ?? [])
      .filter((t) => t.kind === "audio")
      .flatMap((t) => t.clips.filter((c) => c.content.type === "media" && editor.playhead >= c.start - 1.5 && editor.playhead <= c.start + c.duration + 0.25).map((c) => ({ c, muted: t.muted }))),
  );

  function boxOf(clip: Clip, t: Transform): Box {
    const c = clip.content;
    if (c.type === "media") {
      const a = editor.assets.get(c.asset_id);
      return fitBox(t, a?.meta.width ?? W, a?.meta.height ?? H, W, H);
    }
    if (c.type === "text") {
      const m = measure(measureCtx, c.style);
      const pad = c.style.background ? c.style.font_size * 0.35 : 0;
      return { cx: W / 2 + t.x, cy: H / 2 + t.y, w: (m.w + pad * 2) * t.scale, h: (m.h + pad) * t.scale, rotation: t.rotation };
    }
    if (c.type === "pending") return { cx: W / 2, cy: H / 2, w: W, h: H, rotation: 0 };
    return fitBox(t, W, H, W, H);
  }

  const boxStyle = (b: Box) =>
    `left:${b.cx - b.w / 2}px;top:${b.cy - b.h / 2}px;width:${b.w}px;height:${b.h}px;transform:rotate(${b.rotation}deg)`;

  function hit(b: Box, x: number, y: number) {
    const r = (-b.rotation * Math.PI) / 180;
    const dx = x - b.cx;
    const dy = y - b.cy;
    const lx = dx * Math.cos(r) - dy * Math.sin(r);
    const ly = dx * Math.sin(r) + dy * Math.cos(r);
    return Math.abs(lx) <= b.w / 2 && Math.abs(ly) <= b.h / 2;
  }

  const selected = $derived(layers.find((l) => l.visible && editor.selection.includes(l.clip.id) && !l.track.locked));

  function toStage(e: PointerEvent, stage: HTMLElement) {
    const r = stage.getBoundingClientRect();
    return { x: (e.clientX - r.left) / scale, y: (e.clientY - r.top) / scale };
  }

  function stageDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const stage = e.currentTarget as HTMLElement;
    const p = toStage(e, stage);
    const top = [...layers].reverse().find((l) => l.visible && l.clip.content.type !== "pending" && hit(l.box, p.x, p.y));
    if (!top) {
      editor.clearSelection();
      return;
    }
    if (!editor.selection.includes(top.clip.id)) editor.select(top.clip.id, e.shiftKey);
    if (top.track.locked) return;
    startDrag(e, stage, top, "move");
  }

  function startDrag(e: PointerEvent, stage: HTMLElement, layer: Layer, mode: "move" | "scale") {
    e.stopPropagation();
    const start = toStage(e, stage);
    const t0 = { ...layer.t };
    const d0 = Math.hypot(start.x - layer.box.cx, start.y - layer.box.cy) || 1;
    stage.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const p = toStage(ev, stage);
      let t: Transform;
      if (mode === "move") {
        let x = t0.x + p.x - start.x;
        let y = t0.y + p.y - start.y;
        // Snap to the canvas centre lines.
        if (Math.abs(x) < 12 / scale) x = 0;
        if (Math.abs(y) < 12 / scale) y = 0;
        t = { ...t0, x: Math.round(x), y: Math.round(y) };
      } else {
        const d = Math.hypot(p.x - layer.box.cx, p.y - layer.box.cy);
        t = { ...t0, scale: Math.max(0.05, Math.round((t0.scale * d) / d0 * 100) / 100) };
      }
      override = { id: layer.clip.id, t };
    };
    const up = async () => {
      stage.removeEventListener("pointermove", move);
      stage.removeEventListener("pointerup", up);
      const o = override;
      if (o) await editor.edit({ op: "update_clip", clip_id: o.id, patch: { transform: o.t } });
      override = null;
    };
    stage.addEventListener("pointermove", move);
    stage.addEventListener("pointerup", up);
  }

  function jobOf(clip: Clip) {
    return clip.content.type === "pending" ? gen.jobs.find((j) => j.id === (clip.content as { job_id: string }).job_id) : undefined;
  }

  // Dropping a library tile on the canvas inserts it at the playhead.
  let stageEl = $state<HTMLDivElement>();
  $effect(() => {
    if (!drag.active || !stageEl) return;
    const r = stageEl.getBoundingClientRect();
    const inside = drag.x >= r.left && drag.x <= r.right && drag.y >= r.top && drag.y <= r.bottom;
    if (inside) drag.target = { kind: "track", trackId: "", time: editor.playhead };
    else if (drag.target?.trackId === "") drag.target = null;
  });
</script>

<section class="preview">
  <div class="viewport" bind:this={host} bind:clientWidth={hostW} bind:clientHeight={hostH}>
    <div class="frame" style="width:{W * scale}px;height:{H * scale}px">
      <div
        class="stage"
        class:drop={drag.target?.trackId === ""}
        bind:this={stageEl}
        style="width:{W}px;height:{H}px;transform:scale({scale});background:{editor.project?.settings.background}"
        onpointerdown={stageDown}
        role="presentation"
      >
        {#each layers as l (l.clip.id)}
          {@const c = l.clip.content}
          {@const op = opacityAt({ ...l.clip, transform: l.t }, editor.playhead)}
          {#if c.type === "media"}
            {@const a = editor.assets.get(c.asset_id)}
            {#if a?.kind === "video"}
              <MediaSync clip={l.clip} kind="video" src={fileUrl(a.proxy ?? a.path)} muted={false} style="{boxStyle(l.box)};opacity:{op}" />
            {:else if a?.kind === "image" && l.visible}
              <img class="layer" src={fileUrl(a.path)} alt="" style="{boxStyle(l.box)};opacity:{op}" draggable="false" />
            {/if}
          {:else if c.type === "solid" && l.visible}
            <div class="layer" style="{boxStyle(l.box)};opacity:{op};background:{c.color}"></div>
          {:else if c.type === "text" && l.visible}
            <TextLayer style={c.style} transform={l.t} width={W} height={H} opacity={op} />
          {:else if c.type === "pending" && l.visible}
            {@const job = jobOf(l.clip)}
            <div class="pending layer" style={boxStyle(l.box)}>
              <div class="pending-inner" style="font-size:{Math.max(18, H / 36)}px">
                <Sparkles size={Math.max(20, H / 28)} />
                <p>{c.prompt}</p>
                <span>{c.model_name} · {job?.progress.message ?? "Queued"}</span>
                <div class="pbar"><div style="width:{(job?.progress.fraction ?? 0.04) * 100}%"></div></div>
              </div>
            </div>
          {/if}
        {/each}

        {#if selected}
          {@const b = selected.box}
          <div class="sel" style="{boxStyle(b)};--s:{1 / scale}">
            {#each ["nw", "ne", "sw", "se"] as corner (corner)}
              <span class="handle {corner}" role="presentation" onpointerdown={(e) => startDrag(e, stageEl!, selected, "scale")}></span>
            {/each}
          </div>
        {/if}
      </div>
      {#each audioClips as { c, muted } (c.id)}
        {@const a = c.content.type === "media" ? editor.assets.get(c.content.asset_id) : undefined}
        {#if a}<MediaSync clip={c} kind="audio" src={fileUrl(a.proxy ?? a.path)} {muted} />{/if}
      {/each}
    </div>
    {#if !editor.project?.tracks.some((t) => t.clips.length)}
      <div class="hint">Drop media here, or generate something from the left panel.</div>
    {/if}
  </div>

  <div class="transport">
    <div class="tc mono">
      <span class="now">{timecode(editor.playhead, editor.fps)}</span>
      <span class="total">/ {timecode(editor.duration, editor.fps)}</span>
    </div>
    <div class="controls">
      <IconButton title="Start (Home)" onclick={() => editor.seek(0)}><SkipBack size={15} /></IconButton>
      <IconButton title="Previous frame (←)" onclick={() => editor.step(-1)}><StepBack size={15} /></IconButton>
      <button class="play" onclick={() => editor.toggle()} aria-label={editor.playing ? "Pause" : "Play"} data-tip="Play / pause (Space)" data-tip-pos="top">
        {#if editor.playing}<Pause size={16} fill="currentColor" />{:else}<Play size={16} fill="currentColor" />{/if}
      </button>
      <IconButton title="Next frame (→)" onclick={() => editor.step(1)}><StepForward size={15} /></IconButton>
      <IconButton title="End (End)" onclick={() => editor.seek(editor.duration)}><SkipForward size={15} /></IconButton>
    </div>
    <div class="right">
      <IconButton title="Loop" active={editor.loop} tone="accent" onclick={() => (editor.loop = !editor.loop)}><Repeat size={14} /></IconButton>
      <span class="zoom mono">{Math.round(scale * 100)}%</span>
    </div>
  </div>
</section>

<style>
  .preview {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: grid;
    grid-template-rows: minmax(0, 1fr) auto;
    background:
      radial-gradient(60% 60% at 50% 40%, rgba(255, 255, 255, 0.025), transparent),
      var(--bg-sunken);
  }
  .viewport {
    position: relative;
    min-height: 0;
    display: grid;
    place-items: center;
    overflow: hidden;
  }
  .frame {
    position: relative;
    border-radius: 4px;
    box-shadow:
      0 0 0 1px var(--line-2),
      0 30px 80px -30px rgba(0, 0, 0, 0.9);
    overflow: hidden;
  }
  .stage {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
    overflow: hidden;
  }
  .stage.drop {
    outline: 6px solid var(--accent);
    outline-offset: -6px;
  }
  .layer {
    position: absolute;
    display: block;
    pointer-events: none;
    object-fit: fill;
  }
  .stage :global(video) {
    object-fit: fill;
  }
  .pending {
    display: grid;
    place-items: center;
    background:
      linear-gradient(110deg, transparent 35%, rgba(255, 255, 255, 0.05) 50%, transparent 65%),
      radial-gradient(70% 90% at 30% 20%, rgba(255, 90, 54, 0.3), transparent),
      radial-gradient(70% 90% at 80% 90%, rgba(166, 207, 94, 0.22), transparent),
      #120e0c;
    background-size:
      200% 100%,
      100% 100%,
      100% 100%,
      100% 100%;
    animation: shimmer 2.2s linear infinite;
  }
  .pending-inner {
    width: 60%;
    text-align: center;
    color: var(--text);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5em;
  }
  .pending-inner :global(svg) {
    color: var(--amber);
  }
  .pending-inner p {
    margin: 0;
    font-family: var(--font-display);
    font-style: italic;
    font-size: 1.7em;
    line-height: 1.15;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .pending-inner span {
    color: var(--text-2);
    font-size: 0.75em;
  }
  .pbar {
    width: 50%;
    height: 0.25em;
    border-radius: 1em;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }
  .pbar div {
    height: 100%;
    background: var(--gen);
    transition: width 0.6s var(--ease);
  }
  .sel {
    position: absolute;
    pointer-events: none;
    outline: calc(1.5px * var(--s)) solid var(--accent);
    box-shadow: 0 0 0 calc(4px * var(--s)) rgba(255, 90, 54, 0.15);
  }
  .handle {
    position: absolute;
    width: calc(11px * var(--s));
    height: calc(11px * var(--s));
    background: #fff;
    border: calc(1.5px * var(--s)) solid var(--accent);
    border-radius: calc(3px * var(--s));
    pointer-events: auto;
  }
  .nw {
    left: calc(-6px * var(--s));
    top: calc(-6px * var(--s));
    cursor: nwse-resize;
  }
  .ne {
    right: calc(-6px * var(--s));
    top: calc(-6px * var(--s));
    cursor: nesw-resize;
  }
  .sw {
    left: calc(-6px * var(--s));
    bottom: calc(-6px * var(--s));
    cursor: nesw-resize;
  }
  .se {
    right: calc(-6px * var(--s));
    bottom: calc(-6px * var(--s));
    cursor: nwse-resize;
  }
  .hint {
    position: absolute;
    bottom: 18px;
    left: 50%;
    transform: translateX(-50%);
    color: var(--text-3);
    font-size: 12px;
    pointer-events: none;
  }
  .transport {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    height: 48px;
    padding: 0 14px;
    border-top: 1px solid var(--line);
    background: var(--surface);
  }
  .tc {
    font-size: 12px;
  }
  .now {
    color: var(--text);
  }
  .total {
    color: var(--text-3);
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .play {
    width: 36px;
    height: 36px;
    margin: 0 6px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    background: var(--text);
    color: var(--bg);
    transition: transform 0.15s var(--ease-spring);
  }
  .play:hover {
    transform: scale(1.06);
  }
  .play:active {
    transform: scale(0.94);
  }
  .right {
    justify-self: end;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .zoom {
    font-size: 11px;
    color: var(--text-3);
  }
</style>
