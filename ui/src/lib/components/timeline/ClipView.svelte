<script lang="ts">
  import { AudioLines, Sparkles, Square, Type } from "@lucide/svelte";
  import { fileUrl } from "$lib/ipc";
  import type { Clip } from "$lib/bindings/Clip";
  import type { Edge } from "$lib/bindings/Edge";
  import { editor } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import Waveform from "./Waveform.svelte";

  let {
    clip,
    x,
    width,
    height,
    selected,
    moving,
    muted,
    onbody,
    onedge,
    onmenu,
  }: {
    clip: Clip;
    x: number;
    width: number;
    height: number;
    selected: boolean;
    moving: boolean;
    muted: boolean;
    onbody: (e: PointerEvent) => void;
    onedge: (e: PointerEvent, edge: Edge) => void;
    onmenu: (e: MouseEvent) => void;
  } = $props();

  const asset = $derived(editor.assetOf(clip));
  const kind = $derived(clip.content.type === "media" ? (asset?.kind ?? "video") : clip.content.type);
  const generated = $derived(asset?.origin.type === "generated");
  const job = $derived(clip.content.type === "pending" ? gen.jobs.find((j) => j.id === (clip.content as { job_id: string }).job_id) : undefined);
  const sourceSpan = $derived(clip.duration * clip.speed);
  const label = $derived(clip.content.type === "text" ? clip.content.style.content.split("\n")[0] : clip.name);

  // Filmstrip: tile frames across the clip, each showing the source time under it.
  const tiles = $derived.by(() => {
    const strip = asset?.filmstrip;
    if (!strip || kind !== "video") return [];
    const tileW = height * (strip.frame_width / strip.frame_height);
    const n = Math.min(160, Math.ceil(width / tileW));
    return Array.from({ length: n }, (_, i) => {
      const t = clip.in_point + ((i * tileW) / Math.max(width, 1)) * sourceSpan;
      const idx = Math.min(strip.frames - 1, Math.max(0, Math.floor(t / strip.interval)));
      return { left: i * tileW, w: tileW, pos: idx * tileW };
    });
  });
  const stripUrl = $derived(asset?.filmstrip ? fileUrl(asset.filmstrip.path) : "");
  const waveH = $derived(kind === "audio" ? height - 18 : Math.round(height * 0.32));
  const fadeInW = $derived((clip.fade_in / clip.duration) * width);
  const fadeOutW = $derived((clip.fade_out / clip.duration) * width);
</script>

<div
  class="clip {kind}"
  class:selected
  class:moving
  class:generated
  class:muted
  class:narrow={width < 36}
  style="left:{x}px;width:{Math.max(2, width)}px;height:{height}px"
  role="button"
  tabindex="-1"
  onpointerdown={onbody}
  oncontextmenu={onmenu}
>
  <div class="bg">
    {#if tiles.length}
      <div class="strip">
        {#each tiles as t, i (i)}
          <span style="left:{t.left}px;width:{t.w}px;background-image:url('{stripUrl}');background-size:auto {height}px;background-position:-{t.pos}px 0"></span>
        {/each}
      </div>
    {:else if kind === "image" && asset}
      <div class="stills" style="background-image:url('{fileUrl(asset.thumbnail ?? asset.path)}');background-size:auto {height}px"></div>
    {/if}
    {#if asset?.waveform && width > 8}
      <Waveform waveform={asset.waveform} from={clip.in_point} to={clip.in_point + sourceSpan} {width} height={waveH} color={kind === "audio" ? "rgba(178, 218, 110, 0.85)" : "rgba(255,255,255,0.55)"} />
    {/if}
    {#if kind === "pending"}
      <div class="pending-fill" style="width:{(job?.progress.fraction ?? 0) * 100}%"></div>
    {/if}
  </div>

  {#if fadeInW > 2}<svg class="fade in" width={fadeInW} height={height} preserveAspectRatio="none" viewBox="0 0 10 10"><path d="M0 10 L10 0 L0 0 Z" /></svg>{/if}
  {#if fadeOutW > 2}<svg class="fade out" width={fadeOutW} height={height} preserveAspectRatio="none" viewBox="0 0 10 10"><path d="M0 0 L10 0 L10 10 Z" /></svg>{/if}

  <div class="label">
    {#if generated || kind === "pending"}<Sparkles size={11} class="spark" />{/if}
    {#if kind === "text"}<Type size={11} />{:else if kind === "solid"}<Square size={10} />{:else if kind === "audio"}<AudioLines size={11} />{/if}
    <span>{label}</span>
    {#if kind === "pending"}<em>{job?.progress.message ?? "Queued"}</em>{/if}
    {#if clip.speed !== 1}<b class="mono">{clip.speed.toFixed(2).replace(/\.?0+$/, "")}×</b>{/if}
  </div>

  <span class="edge l" role="presentation" onpointerdown={(e) => onedge(e, "start")}></span>
  <span class="edge r" role="presentation" onpointerdown={(e) => onedge(e, "end")}></span>
</div>

<style>
  .clip {
    position: absolute;
    top: 0;
    border-radius: 7px;
    overflow: hidden;
    --c: var(--clip-video);
    --l: var(--clip-video-line);
    background: var(--c);
    box-shadow: inset 0 0 0 1px var(--l);
    transition: box-shadow 0.12s;
  }
  .image {
    --c: var(--clip-image);
    --l: var(--clip-image-line);
  }
  .text {
    --c: var(--clip-text);
    --l: var(--clip-text-line);
  }
  .audio {
    --c: var(--clip-audio);
    --l: var(--clip-audio-line);
  }
  .solid {
    --c: var(--clip-solid);
    --l: var(--clip-solid-line);
  }
  .pending {
    --c: #2a1d17;
    --l: rgba(255, 138, 61, 0.55);
    background:
      linear-gradient(110deg, transparent 30%, rgba(255, 255, 255, 0.07) 50%, transparent 70%),
      var(--gen-soft),
      var(--c);
    background-size:
      200% 100%,
      100% 100%,
      100% 100%;
    animation: shimmer 1.8s linear infinite;
  }
  .generated::before {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    height: 2px;
    background: var(--gen);
    z-index: 2;
  }
  .muted {
    opacity: 0.45;
  }
  .selected {
    box-shadow:
      inset 0 0 0 1.5px var(--text),
      0 0 0 1px rgba(0, 0, 0, 0.6);
    z-index: 3;
  }
  .moving {
    opacity: 0.85;
    box-shadow:
      inset 0 0 0 1.5px var(--accent),
      0 12px 26px -8px rgba(0, 0, 0, 0.8);
    z-index: 10;
  }
  .bg {
    position: absolute;
    inset: 0;
  }
  .strip span {
    position: absolute;
    top: 0;
    bottom: 0;
    background-repeat: no-repeat;
    opacity: 0.85;
  }
  .stills {
    position: absolute;
    inset: 0;
    background-repeat: repeat-x;
    opacity: 0.75;
  }
  .pending-fill {
    position: absolute;
    left: 0;
    bottom: 0;
    height: 3px;
    background: var(--gen);
    transition: width 0.5s var(--ease);
  }
  .fade {
    position: absolute;
    top: 0;
    pointer-events: none;
    fill: rgba(0, 0, 0, 0.35);
  }
  .fade.in {
    left: 0;
  }
  .fade.out {
    right: 0;
  }
  .label {
    position: relative;
    display: flex;
    align-items: center;
    gap: 5px;
    margin: 4px 5px 0;
    padding: 1px 6px;
    width: fit-content;
    max-width: calc(100% - 10px);
    border-radius: 5px;
    background: rgba(10, 8, 7, 0.55);
    backdrop-filter: blur(6px);
    font-size: 11px;
    font-weight: 550;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    pointer-events: none;
  }
  .label span {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .label em {
    font-style: normal;
    color: var(--text-2);
    font-weight: 450;
  }
  .label b {
    font-weight: 500;
    color: var(--amber);
    font-size: 10px;
  }
  .label :global(.spark) {
    color: var(--amber);
    flex: none;
  }
  .narrow .label {
    display: none;
  }
  .edge {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 7px;
    cursor: ew-resize;
    z-index: 4;
  }
  .edge.l {
    left: 0;
  }
  .edge.r {
    right: 0;
  }
  .edge::after {
    content: "";
    position: absolute;
    top: 30%;
    bottom: 30%;
    width: 2px;
    border-radius: 2px;
    background: var(--text);
    opacity: 0;
    transition: opacity 0.12s;
  }
  .edge.l::after {
    left: 2px;
  }
  .edge.r::after {
    right: 2px;
  }
  .clip:hover .edge::after,
  .selected .edge::after {
    opacity: 0.8;
  }
</style>
