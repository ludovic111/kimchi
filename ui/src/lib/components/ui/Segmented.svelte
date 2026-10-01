<script lang="ts" generics="T extends string">
  import type { Component } from "svelte";

  let {
    options,
    value,
    onchange,
    size = "md",
  }: {
    options: { value: T; label?: string; icon?: Component<any>; title?: string }[];
    value: T;
    onchange: (v: T) => void;
    size?: "sm" | "md";
  } = $props();

  const index = $derived(Math.max(0, options.findIndex((o) => o.value === value)));
</script>

<div class="seg {size}" style="--n:{options.length};--i:{index}" role="tablist">
  <span class="thumb"></span>
  {#each options as o (o.value)}
    <button role="tab" aria-selected={o.value === value} class:on={o.value === value} title={o.title} onclick={() => onchange(o.value)}>
      {#if o.icon}<o.icon size={14} strokeWidth={2} />{/if}
      {#if o.label}<span>{o.label}</span>{/if}
    </button>
  {/each}
</div>

<style>
  .seg {
    position: relative;
    display: grid;
    grid-template-columns: repeat(var(--n), 1fr);
    padding: 3px;
    border-radius: var(--r-md);
    background: var(--bg-sunken);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .thumb {
    position: absolute;
    top: 3px;
    bottom: 3px;
    left: 3px;
    width: calc((100% - 6px) / var(--n));
    transform: translateX(calc(100% * var(--i)));
    border-radius: 7px;
    background: var(--surface-4);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.06),
      0 1px 3px rgba(0, 0, 0, 0.4);
    transition: transform 0.28s var(--ease);
  }
  button {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    height: 28px;
    color: var(--text-3);
    font-weight: 550;
    font-size: 12.5px;
    transition: color 0.15s;
  }
  .sm button {
    height: 22px;
    font-size: 11.5px;
  }
  button:hover {
    color: var(--text-2);
  }
  .on,
  .on:hover {
    color: var(--text);
  }
</style>
