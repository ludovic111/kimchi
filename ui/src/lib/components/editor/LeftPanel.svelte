<script lang="ts">
  import { FolderOpen, Sparkles, Type } from "@lucide/svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { ui, type LeftTab } from "$lib/state/ui.svelte";
  import MediaPanel from "./MediaPanel.svelte";
  import TextPanel from "./TextPanel.svelte";
  import GeneratePanel from "../gen/GeneratePanel.svelte";

  const tabs: { id: LeftTab; label: string; icon: any }[] = [
    { id: "media", label: "Media", icon: FolderOpen },
    { id: "generate", label: "Generate", icon: Sparkles },
    { id: "text", label: "Text", icon: Type },
  ];
</script>

<aside class="left">
  <nav class="rail">
    {#each tabs as t (t.id)}
      <button class="tab" class:on={ui.leftTab === t.id} class:gen={t.id === "generate"} onclick={() => (ui.leftTab = t.id)} data-tip={t.label} data-tip-pos="right" aria-label={t.label}>
        <t.icon size={18} strokeWidth={1.8} />
        {#if t.id === "generate" && gen.active.length}<span class="badge">{gen.active.length}</span>{/if}
      </button>
    {/each}
  </nav>
  <div class="panel">
    {#if ui.leftTab === "media"}
      <MediaPanel />
    {:else if ui.leftTab === "generate"}
      <GeneratePanel />
    {:else}
      <TextPanel />
    {/if}
  </div>
</aside>

<style>
  .left {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: grid;
    grid-template-columns: 52px 1fr;
    background: var(--surface);
  }
  .rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 10px 0;
    border-right: 1px solid var(--line);
  }
  .tab {
    position: relative;
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    border-radius: var(--r-md);
    color: var(--text-3);
    transition:
      background 0.15s,
      color 0.15s;
  }
  .tab:hover {
    color: var(--text);
    background: var(--hover);
  }
  .tab.on {
    color: var(--text);
    background: var(--surface-3);
    box-shadow: inset 0 0 0 1px var(--line-2);
  }
  .tab.gen.on {
    color: #1b0c05;
    background: var(--gen);
    box-shadow: 0 6px 16px -6px rgba(255, 110, 60, 0.7);
  }
  .badge {
    position: absolute;
    top: 2px;
    right: 2px;
    min-width: 14px;
    height: 14px;
    padding: 0 3px;
    border-radius: 99px;
    background: var(--accent);
    color: #fff;
    font-size: 9px;
    font-weight: 700;
    display: grid;
    place-items: center;
  }
  .panel {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
