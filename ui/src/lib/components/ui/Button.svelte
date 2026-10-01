<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    variant = "subtle",
    size = "md",
    disabled = false,
    title,
    type = "button",
    wide = false,
    onclick,
    children,
  }: {
    variant?: "primary" | "gen" | "subtle" | "ghost" | "danger";
    size?: "sm" | "md" | "lg";
    disabled?: boolean;
    title?: string;
    type?: "button" | "submit";
    wide?: boolean;
    onclick?: (e: MouseEvent) => void;
    children: Snippet;
  } = $props();
</script>

<button class="btn {variant} {size}" class:wide {type} {disabled} {title} {onclick}>
  {@render children()}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    height: 30px;
    padding: 0 12px;
    border-radius: var(--r-sm);
    font-weight: 550;
    font-size: 12.5px;
    white-space: nowrap;
    transition:
      background 0.15s var(--ease),
      box-shadow 0.15s var(--ease),
      transform 0.1s var(--ease),
      opacity 0.15s;
  }
  .btn:active:not(:disabled) {
    transform: scale(0.97);
  }
  .btn:disabled {
    opacity: 0.4;
  }
  .wide {
    width: 100%;
  }
  .sm {
    height: 26px;
    padding: 0 9px;
    font-size: 12px;
    gap: 5px;
  }
  .lg {
    height: 38px;
    padding: 0 16px;
    font-size: 13.5px;
    border-radius: var(--r-md);
  }
  .subtle {
    background: var(--surface-3);
    box-shadow: inset 0 0 0 1px var(--line-2);
  }
  .subtle:hover:not(:disabled) {
    background: var(--surface-4);
  }
  .ghost {
    color: var(--text-2);
  }
  .ghost:hover:not(:disabled) {
    background: var(--hover);
    color: var(--text);
  }
  .primary {
    background: var(--accent);
    color: #fff;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.25),
      0 1px 2px rgba(0, 0, 0, 0.3),
      0 6px 18px -6px rgba(255, 90, 54, 0.6);
  }
  .primary:hover:not(:disabled) {
    background: var(--accent-hi);
  }
  .gen {
    position: relative;
    background: var(--gen);
    background-size: 160% 100%;
    color: #1b0c05;
    font-weight: 650;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.35),
      0 8px 24px -8px rgba(255, 110, 60, 0.7);
    transition:
      background-position 0.5s var(--ease),
      transform 0.1s var(--ease),
      opacity 0.15s;
  }
  .gen:hover:not(:disabled) {
    background-position: 100% 0;
  }
  .danger {
    background: rgba(255, 77, 94, 0.12);
    color: var(--red);
    box-shadow: inset 0 0 0 1px rgba(255, 77, 94, 0.25);
  }
  .danger:hover:not(:disabled) {
    background: rgba(255, 77, 94, 0.2);
  }
</style>
