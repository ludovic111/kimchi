<script lang="ts">
  import { Clapperboard, Plus, RotateCcw, Square, Wand2 } from "@lucide/svelte";
  import { fileUrl } from "$lib/ipc";
  import type { Asset } from "$lib/bindings/Asset";
  import { editor } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";

  const generated = $derived(
    [...(editor.project?.assets ?? [])].filter((a) => a.origin.type === "generated").reverse().slice(0, 24),
  );
  const running = $derived(gen.active);

  function reuse(a: Asset) {
    if (a.origin.type !== "generated") return;
    const g = a.origin;
    gen.compose({
      mode: g.task.includes("video") ? "video" : "image",
      prompt: g.prompt,
      negative: g.negative_prompt ?? "",
      model: `${g.provider}::${g.model}`,
      seed: "",
    });
  }

  function menu(e: MouseEvent, a: Asset) {
    ui.openMenu(e, [
      { label: "Insert at playhead", icon: Plus, action: () => editor.insertAsset(a.id) },
      { label: "Reuse prompt", icon: RotateCcw, gen: true, action: () => reuse(a) },
      ...(a.kind === "image"
        ? [
            { label: "Animate", icon: Clapperboard, gen: true, action: () => gen.compose({ mode: "video", refs: [{ role: "start_frame" as const, path: a.path, assetId: a.id, label: a.name }] }) },
            { label: "Edit", icon: Wand2, gen: true, action: () => gen.compose({ mode: "image", refs: [{ role: "reference" as const, path: a.path, assetId: a.id, label: a.name }] }) },
          ]
        : []),
    ]);
  }
</script>

{#if running.length || generated.length}
  <div class="recent">
    <span class="eyebrow">Recent</span>
    <div class="grid">
      {#each running as j (j.id)}
        <div class="card pending">
          <div class="shimmer"></div>
          <div class="info">
            <span class="p">{j.request.prompt}</span>
            <span class="m">{j.progress.message ?? j.status}</span>
          </div>
          {#if j.progress.fraction != null}
            <div class="bar"><div style="width:{j.progress.fraction * 100}%"></div></div>
          {/if}
          <button class="stop" aria-label="Cancel" onclick={() => gen.cancel(j.id)}><Square size={9} fill="currentColor" /></button>
        </div>
      {/each}
      {#each generated as a (a.id)}
        <button
          class="card"
          class:selected={editor.selectedAsset === a.id}
          title={a.origin.type === "generated" ? a.origin.prompt : a.name}
          onclick={() => {
            editor.selectedAsset = a.id;
            editor.selection = [];
          }}
          ondblclick={() => editor.insertAsset(a.id)}
          oncontextmenu={(e) => menu(e, a)}
        >
          {#if a.kind === "image"}
            <img src={fileUrl(a.path)} alt="" loading="lazy" />
          {:else if a.thumbnail}
            <img src={fileUrl(a.thumbnail)} alt="" loading="lazy" />
          {:else}
            <video src={fileUrl(a.proxy ?? a.path)} muted preload="metadata"></video>
          {/if}
          {#if a.kind === "video"}<span class="v">▶</span>{/if}
        </button>
      {/each}
    </div>
  </div>
{/if}

<style>
  .recent {
    margin-top: 22px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(90px, 1fr));
    gap: 6px;
  }
  .card {
    position: relative;
    aspect-ratio: 1;
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--surface-3);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .card img,
  .card video {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    transition: transform 0.35s var(--ease);
  }
  .card:hover img,
  .card:hover video {
    transform: scale(1.04);
  }
  .selected {
    box-shadow:
      0 0 0 2px var(--accent),
      0 0 0 4px rgba(255, 90, 54, 0.2);
  }
  .v {
    position: absolute;
    right: 5px;
    bottom: 4px;
    font-size: 9px;
    color: #fff;
    text-shadow: 0 1px 3px rgba(0, 0, 0, 0.8);
  }
  .pending {
    display: flex;
    align-items: flex-end;
  }
  .shimmer {
    position: absolute;
    inset: 0;
    background:
      linear-gradient(110deg, transparent 30%, rgba(255, 255, 255, 0.08) 50%, transparent 70%),
      var(--gen-soft);
    background-size:
      200% 100%,
      100% 100%;
    animation: shimmer 1.6s linear infinite;
  }
  .info {
    position: relative;
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }
  .p {
    font-size: 10.5px;
    line-height: 1.25;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .m {
    font-size: 9.5px;
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .bar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 2px;
    background: rgba(0, 0, 0, 0.3);
  }
  .bar div {
    height: 100%;
    background: var(--gen);
    transition: width 0.5s var(--ease);
  }
  .stop {
    position: absolute;
    top: 5px;
    right: 5px;
    width: 20px;
    height: 20px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    background: rgba(10, 8, 7, 0.6);
    color: var(--text-2);
  }
  .stop:hover {
    color: var(--text);
  }
</style>
