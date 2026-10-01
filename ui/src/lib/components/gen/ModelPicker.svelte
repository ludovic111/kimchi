<script lang="ts">
  import { AudioLines, Check, ChevronDown, Cpu, Cloud, RefreshCw, Search, Star } from "@lucide/svelte";
  import type { ModelInfo } from "$lib/bindings/ModelInfo";
  import { gen, modelKey } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";

  let open = $state(false);
  let query = $state("");
  let el = $state<HTMLDivElement>();
  let input = $state<HTMLInputElement>();

  const kindOf = (provider: string) => gen.providers.find((p) => p.info.id === provider)?.info.kind ?? "cloud";

  const groups = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const list = gen.available.filter(
      (m) => !q || m.name.toLowerCase().includes(q) || m.id.toLowerCase().includes(q) || gen.providerName(m.provider).toLowerCase().includes(q),
    );
    const byProvider = new Map<string, ModelInfo[]>();
    for (const m of list) byProvider.set(m.provider, [...(byProvider.get(m.provider) ?? []), m]);
    return [...byProvider.entries()]
      .map(([provider, models]) => ({
        provider,
        models: models.sort((a, b) => Number(b.featured) - Number(a.featured) || a.name.localeCompare(b.name)),
      }))
      .sort((a, b) => Number(b.models[0].featured) - Number(a.models[0].featured));
  });

  function pick(m: ModelInfo) {
    gen.pickModel(m);
    open = false;
    query = "";
  }

  // Fixed positioning so the popover escapes the panel's scroll clipping.
  let place = $state("");
  function toggle() {
    open = !open;
    if (!open || !el) return;
    const r = el.getBoundingClientRect();
    const above = r.top - 16;
    const below = window.innerHeight - r.bottom - 16;
    const height = Math.min(460, Math.max(above, below));
    const left = Math.max(8, r.left - 8);
    place = above >= below
      ? `left:${left}px;bottom:${window.innerHeight - r.top + 10}px;max-height:${height}px`
      : `left:${left}px;top:${r.bottom + 10}px;max-height:${height}px`;
    queueMicrotask(() => input?.focus());
  }
</script>

<svelte:window onpointerdown={(e) => open && !el?.contains(e.target as Node) && (open = false)} />

<div class="picker" bind:this={el}>
  <button class="trigger" onclick={toggle} aria-haspopup="listbox" aria-expanded={open}>
    {#if gen.model}
      <span class="pk {kindOf(gen.model.provider)}">
        {#if kindOf(gen.model.provider) === "local"}<Cpu size={11} />{:else}<Cloud size={11} />{/if}
      </span>
      <span class="label">
        <strong>{gen.model.name}</strong>
        <small>{gen.providerName(gen.model.provider)}{gen.model.price ? ` · ${gen.model.price}` : ""}</small>
      </span>
    {:else if gen.loadingModels}
      <span class="label"><strong>Loading models…</strong></span>
    {:else}
      <span class="label"><strong>No model for this</strong><small>Connect a provider</small></span>
    {/if}
    <ChevronDown size={14} />
  </button>

  {#if open}
    <div class="pop" role="listbox" style={place}>
      <div class="search">
        <Search size={14} />
        <input bind:this={input} bind:value={query} placeholder="Search {gen.available.length} models" onkeydown={(e) => e.stopPropagation()} />
        <button class="refresh" data-tip="Refresh model lists" onclick={() => gen.loadModels(true)}><RefreshCw size={13} class={gen.loadingModels ? "spin" : ""} /></button>
      </div>
      <div class="list">
        {#each groups as g (g.provider)}
          <div class="group">
            <span class="eyebrow">{gen.providerName(g.provider)}</span>
            {#each g.models as m (modelKey(m))}
              <button class="row" class:on={gen.model && modelKey(gen.model) === modelKey(m)} onclick={() => pick(m)} title={m.description ?? m.id}>
                <span class="main">
                  <span class="name">{#if m.featured}<Star size={10} fill="currentColor" class="star" />{/if}{m.name}</span>
                  <span class="caps">
                    {#if m.durations.length}<i>{m.durations[0]}–{m.durations[m.durations.length - 1]}s</i>{/if}
                    {#if m.resolutions.length}<i>{m.resolutions[m.resolutions.length - 1]}</i>{/if}
                    {#if m.end_frame}<i>first+last</i>{/if}
                    {#if m.max_images > 1}<i>{m.max_images} refs</i>{/if}
                    {#if m.audio}<i class="audio"><AudioLines size={10} /> sound</i>{/if}
                  </span>
                </span>
                {#if m.price}<span class="price">{m.price}</span>{/if}
                {#if gen.model && modelKey(gen.model) === modelKey(m)}<Check size={14} />{/if}
              </button>
            {/each}
          </div>
        {:else}
          <div class="none">
            <p>No connected model can do this yet.</p>
            <button onclick={() => ((open = false), ui.openSettings())}>Add a provider →</button>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    min-width: 0;
    flex: 1;
  }
  .trigger {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 10px 0 6px;
    border-radius: var(--r-md);
    color: var(--text-2);
    transition: background 0.15s;
  }
  .trigger:hover {
    background: var(--hover);
  }
  .pk {
    width: 24px;
    height: 24px;
    flex: none;
    border-radius: 7px;
    display: grid;
    place-items: center;
    background: var(--surface-4);
    color: var(--text-2);
  }
  .pk.local {
    background: var(--green-soft);
    color: var(--green);
  }
  .label {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    text-align: left;
    line-height: 1.2;
  }
  .label strong {
    color: var(--text);
    font-weight: 600;
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .label small {
    color: var(--text-3);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pop {
    position: fixed;
    width: min(400px, calc(100vw - 40px));
    display: flex;
    flex-direction: column;
    border-radius: var(--r-lg);
    background: rgba(30, 28, 26, 0.97);
    backdrop-filter: blur(20px);
    box-shadow: var(--shadow-lift);
    z-index: 160;
    animation: rise 0.18s var(--ease);
    overflow: hidden;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--line);
    color: var(--text-3);
  }
  .search input {
    flex: 1;
    background: none;
    border: 0;
    outline: 0;
    font-size: 13px;
  }
  .refresh {
    color: var(--text-3);
    width: 24px;
    height: 24px;
    display: grid;
    place-items: center;
    border-radius: 6px;
  }
  .refresh:hover {
    color: var(--text);
    background: var(--hover);
  }
  .refresh :global(.spin) {
    animation: spin 0.8s linear infinite;
  }
  .list {
    overflow: auto;
    padding: 4px 6px 8px;
  }
  .group .eyebrow {
    display: block;
    padding: 10px 8px 4px;
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 8px;
    border-radius: var(--r-sm);
    text-align: left;
  }
  .row:hover {
    background: var(--hover);
  }
  .row.on {
    background: var(--accent-soft);
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .name {
    font-size: 12.5px;
    font-weight: 550;
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .name :global(.star) {
    color: var(--amber);
  }
  .caps {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .caps i {
    font-style: normal;
    font-size: 10px;
    color: var(--text-3);
    padding: 0 5px;
    border-radius: 4px;
    background: var(--surface-3);
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }
  .caps .audio {
    color: var(--green);
  }
  .price {
    font-size: 11px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .none {
    padding: 18px 12px;
    color: var(--text-3);
    text-align: center;
  }
  .none p {
    margin: 0 0 8px;
  }
  .none button {
    color: var(--accent-hi);
    font-weight: 550;
  }
</style>
