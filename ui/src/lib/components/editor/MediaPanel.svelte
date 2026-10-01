<script lang="ts">
  import { AudioLines, Clapperboard, Film, Image as ImageIcon, Import, Plus, Sparkles, Trash2, Wand2 } from "@lucide/svelte";
  import { fileUrl } from "$lib/ipc";
  import type { Asset } from "$lib/bindings/Asset";
  import { drag } from "$lib/state/drag.svelte";
  import { editor } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { short } from "$lib/util/time";
  import Button from "../ui/Button.svelte";
  import { importDialog } from "./actions";

  type Filter = "all" | "video" | "image" | "audio" | "generated";
  let filter = $state<Filter>("all");

  const assets = $derived(
    [...(editor.project?.assets ?? [])]
      .reverse()
      .filter((a) => (filter === "all" ? true : filter === "generated" ? a.origin.type === "generated" : a.kind === filter)),
  );
  const counts = $derived({
    generated: editor.project?.assets.filter((a) => a.origin.type === "generated").length ?? 0,
  });

  function down(e: PointerEvent, a: Asset) {
    if (e.button !== 0) return;
    const sx = e.clientX;
    const sy = e.clientY;
    let started = false;
    const move = (ev: PointerEvent) => {
      if (!started && Math.hypot(ev.clientX - sx, ev.clientY - sy) > 5) {
        started = true;
        drag.start(a.id, ev);
      }
      if (started) {
        drag.x = ev.clientX;
        drag.y = ev.clientY;
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (!started) return;
      const t = drag.target;
      if (t) editor.insertAsset(a.id, t.trackId || null, t.time);
      else if (drag.overComposer && a.kind !== "audio") useAsRef(a);
      drag.end();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function useAsRef(a: Asset) {
    if (a.kind !== "image") return ui.toast("Drop an image — for videos, use a frame from the timeline.");
    gen.addRef({ role: gen.draft.mode === "video" ? "start_frame" : "reference", path: a.path, assetId: a.id, label: a.name });
  }

  function menu(e: MouseEvent, a: Asset) {
    editor.selectedAsset = a.id;
    editor.selection = [];
    ui.openMenu(e, [
      { label: "Insert at playhead", icon: Plus, action: () => editor.insertAsset(a.id) },
      ...(a.kind === "image"
        ? [
            "separator" as const,
            {
              label: "Animate with AI",
              icon: Clapperboard,
              gen: true,
              action: () => gen.compose({ mode: "video", refs: [{ role: "start_frame", path: a.path, assetId: a.id, label: a.name }] }),
            },
            {
              label: "Edit with AI",
              icon: Wand2,
              gen: true,
              action: () => gen.compose({ mode: "image", refs: [{ role: "reference", path: a.path, assetId: a.id, label: a.name }] }),
            },
          ]
        : []),
      "separator",
      { label: "Remove from project", icon: Trash2, danger: true, action: () => editor.edit({ op: "remove_asset", asset_id: a.id }) },
    ]);
  }
</script>

<div class="head">
  <h3>Media</h3>
  <Button size="sm" onclick={importDialog}><Import size={13} /> Import</Button>
</div>

<div class="filters">
  {#each [["all", "All"], ["video", "Video"], ["image", "Images"], ["audio", "Audio"], ["generated", "Generated"]] as [id, label] (id)}
    <button class="f" class:on={filter === id} onclick={() => (filter = id as Filter)}>
      {#if id === "generated"}<Sparkles size={11} />{/if}{label}
      {#if id === "generated" && counts.generated}<span class="n">{counts.generated}</span>{/if}
    </button>
  {/each}
</div>

<div class="scroll">
  {#if !assets.length}
    <button class="empty" onclick={importDialog}>
      <div class="icon"><Import size={20} /></div>
      <strong>{filter === "generated" ? "Nothing generated yet" : "Drop files anywhere"}</strong>
      <span>{filter === "generated" ? "Generations land here and on your timeline." : "or click to browse — video, images and audio."}</span>
    </button>
  {/if}
  <div class="grid">
    {#each assets as a (a.id)}
      <button
        class="tile"
        class:selected={editor.selectedAsset === a.id}
        class:generated={a.origin.type === "generated"}
        onpointerdown={(e) => down(e, a)}
        onclick={() => {
          editor.selectedAsset = a.id;
          editor.selection = [];
        }}
        ondblclick={() => editor.insertAsset(a.id)}
        oncontextmenu={(e) => menu(e, a)}
        title={a.name}
      >
        <div class="thumb">
          {#if a.kind === "audio"}
            <div class="audio"><AudioLines size={22} /></div>
          {:else if a.thumbnail || a.kind === "image"}
            <img src={fileUrl(a.thumbnail ?? a.path)} alt="" loading="lazy" draggable="false" />
          {:else}
            <div class="loading"></div>
          {/if}
          <span class="kind">
            {#if a.kind === "video"}<Film size={10} />{:else if a.kind === "image"}<ImageIcon size={10} />{:else}<AudioLines size={10} />{/if}
            {#if a.meta.duration && a.kind !== "image"}<span class="mono">{short(a.meta.duration)}</span>{/if}
          </span>
          {#if a.origin.type === "generated"}<span class="spark"><Sparkles size={10} /></span>{/if}
        </div>
        <span class="name">{a.name}</span>
      </button>
    {/each}
  </div>
</div>

{#if drag.active}
  {@const a = editor.assets.get(drag.assetId!)}
  {#if a}
    <div class="ghost" style="left:{drag.x}px;top:{drag.y}px">
      {#if a.thumbnail || a.kind === "image"}<img src={fileUrl(a.thumbnail ?? a.path)} alt="" />{:else}<AudioLines size={16} />{/if}
      <span>{a.name}</span>
    </div>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 14px 10px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }
  .filters {
    display: flex;
    gap: 2px;
    padding: 0 10px 10px;
    flex-wrap: wrap;
  }
  .f {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 8px;
    border-radius: 99px;
    color: var(--text-3);
    font-size: 11.5px;
    font-weight: 550;
  }
  .f:hover {
    color: var(--text-2);
  }
  .f.on {
    color: var(--text);
    background: var(--surface-3);
  }
  .n {
    font-size: 10px;
    color: var(--amber);
  }
  .scroll {
    flex: 1;
    overflow: auto;
    padding: 0 10px 14px;
  }
  .empty {
    width: 100%;
    margin-top: 8px;
    padding: 28px 16px;
    border-radius: var(--r-lg);
    border: 1px dashed var(--line-3);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    text-align: center;
    color: var(--text-3);
  }
  .empty:hover {
    border-color: var(--accent-line);
    background: rgba(255, 90, 54, 0.03);
  }
  .empty .icon {
    width: 40px;
    height: 40px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: var(--surface-3);
    color: var(--text-2);
    margin-bottom: 4px;
  }
  .empty strong {
    color: var(--text);
    font-weight: 600;
  }
  .empty span {
    font-size: 12px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(118px, 1fr));
    gap: 12px 8px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 6px;
    text-align: left;
    min-width: 0;
  }
  .thumb {
    position: relative;
    aspect-ratio: 16 / 10;
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--surface-3);
    box-shadow: inset 0 0 0 1px var(--line);
    transition: box-shadow 0.15s;
  }
  .tile:hover .thumb {
    box-shadow: inset 0 0 0 1px var(--line-3);
  }
  .selected .thumb {
    box-shadow:
      0 0 0 2px var(--accent),
      0 0 0 4px rgba(255, 90, 54, 0.2);
  }
  .generated .thumb::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: inherit;
    padding: 1px;
    background: var(--gen);
    -webkit-mask:
      linear-gradient(#000 0 0) content-box,
      linear-gradient(#000 0 0);
    mask:
      linear-gradient(#000 0 0) content-box,
      linear-gradient(#000 0 0);
    -webkit-mask-composite: xor;
    mask-composite: exclude;
    opacity: 0.7;
    pointer-events: none;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    pointer-events: none;
  }
  .audio {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--green);
    background: linear-gradient(160deg, #22301b, #161d12);
  }
  .loading {
    position: absolute;
    inset: 0;
    background: linear-gradient(90deg, transparent, rgba(255, 255, 255, 0.04), transparent), var(--surface-3);
    background-size: 200% 100%;
    animation: shimmer 1.6s linear infinite;
  }
  .kind {
    position: absolute;
    left: 5px;
    bottom: 5px;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 1px 5px;
    border-radius: 5px;
    font-size: 10px;
    background: rgba(10, 8, 7, 0.72);
    color: var(--text-2);
  }
  .spark {
    position: absolute;
    top: 5px;
    right: 5px;
    width: 18px;
    height: 18px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    background: var(--gen);
    color: #1b0c05;
  }
  .name {
    font-size: 11.5px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0 2px;
  }
  .ghost {
    position: fixed;
    z-index: 300;
    pointer-events: none;
    transform: translate(10px, 10px);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 10px 5px 5px;
    border-radius: var(--r-md);
    background: var(--surface-4);
    box-shadow: var(--shadow-lift);
    font-size: 12px;
    max-width: 240px;
  }
  .ghost img {
    width: 40px;
    height: 26px;
    object-fit: cover;
    border-radius: 5px;
  }
  .ghost span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
