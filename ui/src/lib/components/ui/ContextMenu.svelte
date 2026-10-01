<script lang="ts">
  import { ui, type MenuItem } from "$lib/state/ui.svelte";

  let el = $state<HTMLDivElement>();
  let pos = $state({ x: 0, y: 0 });

  $effect(() => {
    if (!ui.menu || !el) return;
    // Keep the menu on screen.
    const r = el.getBoundingClientRect();
    pos = {
      x: Math.min(ui.menu.x, window.innerWidth - r.width - 8),
      y: Math.min(ui.menu.y, window.innerHeight - r.height - 8),
    };
  });

  function run(item: MenuItem) {
    ui.menu = null;
    item.action?.();
  }
</script>

<svelte:window
  onpointerdown={(e) => ui.menu && !el?.contains(e.target as Node) && (ui.menu = null)}
  onkeydown={(e) => e.key === "Escape" && (ui.menu = null)}
  onblur={() => (ui.menu = null)}
/>

{#if ui.menu}
  <div class="menu" bind:this={el} style="left:{pos.x || ui.menu.x}px;top:{pos.y || ui.menu.y}px" role="menu">
    {#each ui.menu.items as item}
      {#if item === "separator"}
        <div class="sep"></div>
      {:else}
        <button role="menuitem" class:danger={item.danger} class:gen={item.gen} disabled={item.disabled} onclick={() => run(item)}>
          <span class="icon">{#if item.icon}<item.icon size={14} strokeWidth={2} />{/if}</span>
          <span class="label">{item.label}</span>
          {#if item.shortcut}<kbd>{item.shortcut}</kbd>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 150;
    min-width: 210px;
    padding: 5px;
    border-radius: var(--r-md);
    background: rgba(34, 32, 30, 0.92);
    backdrop-filter: blur(20px) saturate(1.4);
    box-shadow: var(--shadow-pop);
    animation: rise 0.14s var(--ease);
  }
  button {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 9px;
    height: 29px;
    padding: 0 9px 0 7px;
    border-radius: 6px;
    text-align: left;
    font-size: 12.5px;
  }
  button:hover:not(:disabled) {
    background: var(--accent);
    color: #fff;
  }
  button:hover:not(:disabled) .icon,
  button:hover:not(:disabled) kbd {
    color: #fff;
  }
  button:disabled {
    opacity: 0.4;
  }
  .icon {
    width: 16px;
    display: grid;
    place-items: center;
    color: var(--text-2);
  }
  .gen .icon {
    color: var(--amber);
  }
  .danger {
    color: var(--red);
  }
  .danger .icon {
    color: var(--red);
  }
  .label {
    flex: 1;
  }
  kbd {
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--text-3);
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--line-2);
  }
</style>
