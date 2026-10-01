<script lang="ts">
  import type { Snippet } from "svelte";
  import { keys } from "$lib/util/platform";

  let {
    title,
    active = false,
    disabled = false,
    size = 28,
    tone = "default",
    onclick,
    children,
  }: {
    title: string;
    active?: boolean;
    disabled?: boolean;
    size?: number;
    tone?: "default" | "accent";
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
  } = $props();
</script>

<button class="ib {tone}" class:active {disabled} aria-label={keys(title)} data-tip={keys(title)} style="--s:{size}px" {onclick}>
  {@render children()}
</button>

<style>
  .ib {
    width: var(--s);
    height: var(--s);
    display: inline-grid;
    place-items: center;
    border-radius: var(--r-sm);
    color: var(--text-2);
    transition:
      background 0.12s,
      color 0.12s,
      transform 0.1s var(--ease);
    flex: none;
  }
  .ib:hover:not(:disabled) {
    background: var(--hover);
    color: var(--text);
  }
  .ib:active:not(:disabled) {
    transform: scale(0.92);
  }
  .ib:disabled {
    opacity: 0.35;
  }
  .active {
    background: var(--press);
    color: var(--text);
  }
  .accent.active {
    background: var(--accent-soft);
    color: var(--accent-hi);
  }
</style>
