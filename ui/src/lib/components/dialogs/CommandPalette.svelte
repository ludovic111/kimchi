<script lang="ts">
  import {
    ArrowLeft,
    AudioLines,
    Film,
    Image as ImageIcon,
    Import,
    KeyRound,
    Magnet,
    Redo2,
    Scissors,
    Search,
    Share,
    Sparkles,
    Trash2,
    Type,
    Undo2,
  } from "@lucide/svelte";
  import { api } from "$lib/ipc";
  import { editor } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { addText, importDialog } from "../editor/actions";

  interface Cmd {
    label: string;
    hint?: string;
    icon: any;
    gen?: boolean;
    run: () => void;
  }

  let query = $state("");
  let index = $state(0);
  let input = $state<HTMLInputElement>();

  const base: Cmd[] = [
    { label: "Generate video…", icon: Film, gen: true, run: () => gen.compose({ mode: "video" }) },
    { label: "Generate image…", icon: ImageIcon, gen: true, run: () => gen.compose({ mode: "image" }) },
    { label: "Import media", hint: "⌘I", icon: Import, run: importDialog },
    { label: "Add text", icon: Type, run: () => addText() },
    { label: "Split at playhead", hint: "S", icon: Scissors, run: () => editor.splitAtPlayhead() },
    { label: "Delete selection", hint: "⌫", icon: Trash2, run: () => editor.deleteSelection() },
    { label: "Undo", hint: "⌘Z", icon: Undo2, run: () => editor.undo() },
    { label: "Redo", hint: "⇧⌘Z", icon: Redo2, run: () => editor.redo() },
    { label: "Toggle snapping", hint: "N", icon: Magnet, run: () => (editor.snapping = !editor.snapping) },
    { label: "Add video track", icon: Film, run: () => editor.edit({ op: "add_track", kind: "video", index: null }) },
    { label: "Add audio track", icon: AudioLines, run: () => editor.edit({ op: "add_track", kind: "audio", index: null }) },
    { label: "Export…", hint: "⌘E", icon: Share, run: () => (ui.exportOpen = true) },
    { label: "Models & keys", icon: KeyRound, run: () => ui.openSettings() },
    {
      label: "Back to projects",
      icon: ArrowLeft,
      run: async () => {
        await api.closeProject();
        editor.set(null);
      },
    },
  ];

  const results = $derived.by((): Cmd[] => {
    const q = query.trim().toLowerCase();
    if (!q) return base;
    const matches = base.filter((c) => c.label.toLowerCase().includes(q));
    // Anything typed can become a prompt.
    const prompt = query.trim();
    const make: Cmd[] = [
      { label: `Generate video: “${prompt}”`, icon: Sparkles, gen: true, run: () => submit("video", prompt) },
      { label: `Generate image: “${prompt}”`, icon: Sparkles, gen: true, run: () => submit("image", prompt) },
    ];
    return matches.length ? [...matches, ...make] : make;
  });

  function submit(mode: "image" | "video", prompt: string) {
    gen.compose({ mode, prompt, toTimeline: true, target: null });
    if (gen.model) void gen.submit();
  }

  function run(c: Cmd) {
    ui.paletteOpen = false;
    query = "";
    c.run();
  }

  $effect(() => {
    if (ui.paletteOpen) {
      index = 0;
      queueMicrotask(() => input?.focus());
    }
  });

  $effect(() => {
    void query;
    index = 0;
  });

  function key(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "ArrowDown") {
      e.preventDefault();
      index = Math.min(results.length - 1, index + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      index = Math.max(0, index - 1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const c = results[index];
      if (c) run(c);
    } else if (e.key === "Escape") {
      ui.paletteOpen = false;
    }
  }
</script>

{#if ui.paletteOpen}
  <div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && (ui.paletteOpen = false)}>
    <div class="palette" role="dialog" aria-label="Command palette">
      <div class="search">
        <Search size={16} />
        <input bind:this={input} bind:value={query} placeholder="Type a command — or describe something to generate" onkeydown={key} />
        <kbd>esc</kbd>
      </div>
      <div class="list">
        {#each results as c, i (c.label)}
          <button class="cmd" class:on={i === index} class:gen={c.gen} onpointerenter={() => (index = i)} onclick={() => run(c)}>
            <c.icon size={15} />
            <span>{c.label}</span>
            {#if c.hint}<kbd>{c.hint}</kbd>{/if}
          </button>
        {/each}
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 120;
    background: rgba(8, 6, 5, 0.45);
    display: flex;
    justify-content: center;
    padding-top: 14vh;
    animation: fade 0.12s;
  }
  .palette {
    width: min(600px, calc(100vw - 40px));
    max-height: 420px;
    display: flex;
    flex-direction: column;
    border-radius: var(--r-xl);
    background: rgba(30, 28, 26, 0.96);
    backdrop-filter: blur(24px) saturate(1.3);
    box-shadow: var(--shadow-lift);
    overflow: hidden;
    animation: rise 0.18s var(--ease);
    align-self: flex-start;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--line);
    color: var(--text-3);
  }
  .search input {
    flex: 1;
    background: none;
    border: 0;
    outline: 0;
    font-size: 15px;
    color: var(--text);
  }
  kbd {
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--text-3);
    padding: 2px 6px;
    border-radius: 5px;
    background: var(--surface-3);
  }
  .list {
    overflow: auto;
    padding: 6px;
  }
  .cmd {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 11px;
    height: 36px;
    padding: 0 10px;
    border-radius: var(--r-sm);
    text-align: left;
    color: var(--text-2);
  }
  .cmd span {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cmd.on {
    background: var(--surface-4);
    color: var(--text);
  }
  .cmd.gen :global(svg) {
    color: var(--amber);
  }
</style>
