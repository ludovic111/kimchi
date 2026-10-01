<script lang="ts">
  import { ChevronLeft, Command, KeyRound, Redo2, Share, Undo2 } from "@lucide/svelte";
  import { api } from "$lib/ipc";
  import { editor } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import Button from "../ui/Button.svelte";
  import IconButton from "../ui/IconButton.svelte";
  import Mark from "../ui/Mark.svelte";
  import JobsPopover from "../gen/JobsPopover.svelte";

  let renaming = $state(false);
  let name = $state("");

  async function home() {
    editor.pause();
    await api.closeProject();
    editor.set(null);
  }

  function startRename() {
    name = editor.project?.name ?? "";
    renaming = true;
  }

  async function commitRename() {
    renaming = false;
    if (name.trim() && name.trim() !== editor.project?.name) await editor.edit({ op: "rename_project", name: name.trim() });
  }

  const focus = (n: HTMLInputElement) => {
    n.focus();
    n.select();
  };
</script>

<header class="bar drag">
  <div class="left">
    <button class="home" onclick={home} data-tip="All projects" aria-label="All projects">
      <ChevronLeft size={15} />
      <Mark size={18} />
    </button>
    <span class="slash">/</span>
    {#if renaming}
      <input
        class="name-input"
        use:focus
        bind:value={name}
        onblur={commitRename}
        onkeydown={(e) => {
          if (e.key === "Enter") commitRename();
          if (e.key === "Escape") renaming = false;
          e.stopPropagation();
        }}
      />
    {:else}
      <button class="name" ondblclick={startRename} onclick={startRename} title="Rename">{editor.project?.name}</button>
    {/if}
    <span class="spec mono">{editor.project?.settings.width}×{editor.project?.settings.height} · {editor.project?.settings.fps}fps</span>
  </div>

  <div class="right">
    <IconButton title="Undo (⌘Z)" disabled={!editor.view?.can_undo} onclick={() => editor.undo()}><Undo2 size={15} /></IconButton>
    <IconButton title="Redo (⇧⌘Z)" disabled={!editor.view?.can_redo} onclick={() => editor.redo()}><Redo2 size={15} /></IconButton>
    <span class="sep"></span>
    <div class="jobs-anchor">
      <button class="jobs" class:busy={gen.active.length > 0} onclick={() => (ui.jobsOpen = !ui.jobsOpen)} aria-label="Generations">
        <span class="ring"></span>
        {#if gen.active.length}
          <span>{gen.active.length} generating</span>
        {:else}
          <span>Generations</span>
        {/if}
      </button>
      {#if ui.jobsOpen}<JobsPopover />{/if}
    </div>
    <IconButton title="Command palette (⌘K)" onclick={() => (ui.paletteOpen = true)}><Command size={15} /></IconButton>
    <IconButton title="Models & keys" onclick={() => ui.openSettings()}><KeyRound size={15} /></IconButton>
    <Button variant="primary" size="sm" onclick={() => (ui.exportOpen = true)}><Share size={13} /> Export</Button>
  </div>
</header>

<style>
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 12px 0 84px;
    border-bottom: 1px solid var(--line);
    background: var(--surface);
  }
  .left,
  .right {
    display: flex;
    align-items: center;
    gap: 6px;
    -webkit-app-region: no-drag;
    min-width: 0;
  }
  .home {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 28px;
    padding: 0 6px 0 2px;
    border-radius: var(--r-sm);
    color: var(--text-3);
  }
  .home:hover {
    background: var(--hover);
    color: var(--text);
  }
  .slash {
    color: var(--text-4);
  }
  .name {
    font-weight: 600;
    font-size: 13px;
    padding: 4px 6px;
    border-radius: var(--r-xs);
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name:hover {
    background: var(--hover);
  }
  .name-input {
    font-weight: 600;
    font-size: 13px;
    padding: 3px 6px;
    border-radius: var(--r-xs);
    border: 0;
    outline: 1px solid var(--accent-line);
    background: var(--surface-2);
    width: 240px;
  }
  .spec {
    margin-left: 6px;
    color: var(--text-4);
    font-size: 11px;
  }
  .sep {
    width: 1px;
    height: 18px;
    background: var(--line-2);
    margin: 0 4px;
  }
  .jobs-anchor {
    position: relative;
  }
  .jobs {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 11px 0 9px;
    border-radius: 99px;
    color: var(--text-2);
    font-size: 12px;
    font-weight: 550;
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .jobs:hover {
    color: var(--text);
    background: var(--surface-3);
  }
  .ring {
    width: 12px;
    height: 12px;
    border-radius: 99px;
    background: var(--gen);
    -webkit-mask: radial-gradient(circle, transparent 3.2px, #000 3.6px);
    mask: radial-gradient(circle, transparent 3.2px, #000 3.6px);
    opacity: 0.55;
  }
  .busy {
    color: var(--text);
    box-shadow: inset 0 0 0 1px rgba(255, 138, 61, 0.35);
  }
  .busy .ring {
    opacity: 1;
    background: conic-gradient(from 0deg, #ff5a36, #f0b44c, #a6cf5e, transparent 75%);
    animation: spin 0.9s linear infinite;
  }
</style>
