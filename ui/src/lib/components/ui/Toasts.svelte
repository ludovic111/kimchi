<script lang="ts">
  import { ui } from "$lib/state/ui.svelte";
  import { CircleAlert, CircleCheck, Info, X } from "@lucide/svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each ui.toasts as t (t.id)}
    <div class="toast {t.kind}">
      {#if t.kind === "error"}<CircleAlert size={15} />{:else if t.kind === "success"}<CircleCheck size={15} />{:else}<Info size={15} />{/if}
      <p>{t.text}</p>
      <button aria-label="Dismiss" onclick={() => ui.dismiss(t.id)}><X size={13} /></button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 200;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 340px;
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 11px 12px;
    border-radius: var(--r-md);
    background: var(--surface-3);
    box-shadow: var(--shadow-pop);
    animation: rise 0.25s var(--ease);
  }
  .toast :global(svg:first-child) {
    flex: none;
    margin-top: 1px;
    color: var(--text-2);
  }
  .error :global(svg:first-child) {
    color: var(--red);
  }
  .success :global(svg:first-child) {
    color: var(--green);
  }
  p {
    margin: 0;
    flex: 1;
    font-size: 12.5px;
    color: var(--text);
    white-space: pre-line;
    overflow-wrap: anywhere;
  }
  button {
    color: var(--text-3);
    flex: none;
  }
  button:hover {
    color: var(--text);
  }
</style>
