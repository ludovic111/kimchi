<script lang="ts">
  import type { Snippet } from "svelte";
  import { X } from "@lucide/svelte";

  let {
    open,
    onclose,
    title,
    subtitle,
    width = 560,
    children,
    footer,
  }: {
    open: boolean;
    onclose: () => void;
    title: string;
    subtitle?: string;
    width?: number;
    children: Snippet;
    footer?: Snippet;
  } = $props();
</script>

<svelte:window onkeydown={(e) => open && e.key === "Escape" && onclose()} />

{#if open}
  <div class="backdrop" role="presentation" onpointerdown={(e) => e.target === e.currentTarget && onclose()}>
    <div class="dialog" role="dialog" aria-modal="true" aria-label={title} style="width:min({width}px, calc(100vw - 48px))">
      <header>
        <div>
          <h2>{title}</h2>
          {#if subtitle}<p>{subtitle}</p>{/if}
        </div>
        <button class="close" aria-label="Close" onclick={onclose}><X size={16} /></button>
      </header>
      <div class="body">{@render children()}</div>
      {#if footer}<footer>{@render footer()}</footer>{/if}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    background: rgba(8, 6, 5, 0.6);
    backdrop-filter: blur(6px);
    animation: fade 0.18s var(--ease);
  }
  .dialog {
    max-height: calc(100vh - 80px);
    display: flex;
    flex-direction: column;
    border-radius: var(--r-xl);
    background: var(--surface);
    box-shadow: var(--shadow-lift);
    animation: rise 0.26s var(--ease);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    padding: 20px 22px 6px;
  }
  h2 {
    margin: 0;
    font-size: 17px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  p {
    margin: 4px 0 0;
    color: var(--text-3);
    font-size: 12.5px;
  }
  .close {
    width: 28px;
    height: 28px;
    display: grid;
    place-items: center;
    border-radius: var(--r-sm);
    color: var(--text-3);
  }
  .close:hover {
    background: var(--hover);
    color: var(--text);
  }
  .body {
    padding: 14px 22px 22px;
    overflow: auto;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 14px 22px;
    border-top: 1px solid var(--line);
    background: var(--bg-sunken);
  }
</style>
