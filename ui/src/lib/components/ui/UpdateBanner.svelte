<script lang="ts">
  import { ArrowDownToLine, X } from "@lucide/svelte";
  import { update } from "$lib/state/update.svelte";
</script>

{#if update.available && !update.dismissed}
  <div class="banner" role="status">
    <span class="dot"></span>
    <span class="text">
      {#if update.progress === null}
        kimchi <b>{update.available.version}</b> is available
      {:else}
        Downloading {Math.round(update.progress * 100)}%
      {/if}
    </span>
    {#if update.progress === null}
      <button class="go" onclick={() => update.install()}><ArrowDownToLine size={13} /> Restart to update</button>
      <button class="x" aria-label="Later" onclick={() => (update.dismissed = true)}><X size={13} /></button>
    {:else}
      <span class="bar"><span style="width:{update.progress * 100}%"></span></span>
    {/if}
  </div>
{/if}

<style>
  .banner {
    position: fixed;
    left: 16px;
    bottom: 16px;
    z-index: 190;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 6px 6px 12px;
    border-radius: 99px;
    background: rgba(34, 32, 30, 0.95);
    backdrop-filter: blur(16px);
    box-shadow: var(--shadow-pop);
    font-size: 12.5px;
    animation: rise 0.3s var(--ease);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 99px;
    background: var(--gen);
  }
  .text b {
    font-weight: 650;
  }
  .go {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 11px;
    border-radius: 99px;
    background: var(--accent);
    color: #fff;
    font-weight: 600;
    font-size: 12px;
  }
  .go:hover {
    background: var(--accent-hi);
  }
  .x {
    width: 26px;
    height: 26px;
    display: grid;
    place-items: center;
    border-radius: 99px;
    color: var(--text-3);
  }
  .x:hover {
    color: var(--text);
    background: var(--hover);
  }
  .bar {
    width: 120px;
    height: 4px;
    margin-right: 8px;
    border-radius: 4px;
    background: var(--surface-4);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--gen);
    transition: width 0.3s var(--ease);
  }
</style>
