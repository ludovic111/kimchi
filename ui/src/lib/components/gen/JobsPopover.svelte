<script lang="ts">
  import { Check, CircleAlert, Square, X } from "@lucide/svelte";
  import { fileUrl } from "$lib/ipc";
  import { gen } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { ago } from "$lib/util/time";

  let el = $state<HTMLDivElement>();
  const jobs = $derived([...gen.jobs].sort((a, b) => b.created_at.localeCompare(a.created_at)));
</script>

<svelte:window
  onpointerdown={(e) => {
    const t = e.target as HTMLElement;
    if (!el?.contains(t) && !t.closest(".jobs")) ui.jobsOpen = false;
  }}
/>

<div class="pop" bind:this={el}>
  <div class="head">
    <strong>Generations</strong>
    {#if jobs.some((j) => j.status !== "queued" && j.status !== "running")}
      <button class="clear" onclick={() => gen.clearFinished()}>Clear finished</button>
    {/if}
  </div>
  {#if !jobs.length}
    <p class="empty">Nothing yet. Write a prompt in the Generate panel — results land in your library and on the timeline.</p>
  {/if}
  <div class="list">
    {#each jobs as j (j.id)}
      {@const out = j.outputs[0]}
      <div class="job {j.status}">
        <div class="thumb">
          {#if out?.kind === "image"}
            <img src={fileUrl(out.path)} alt="" />
          {:else if out?.kind === "video"}
            <video src={fileUrl(out.path)} muted preload="metadata"></video>
          {:else if j.status === "failed"}
            <CircleAlert size={15} />
          {:else if j.status === "cancelled"}
            <X size={15} />
          {:else}
            <span class="pulse"></span>
          {/if}
        </div>
        <div class="body">
          <p class="prompt">{j.request.prompt || "Untitled"}</p>
          <div class="sub">
            <span>{j.model_name}</span>
            <span class="dot">·</span>
            <span>{gen.providerName(j.provider)}</span>
            {#if j.status === "succeeded"}
              <span class="dot">·</span><span>{(j.elapsed_ms / 1000).toFixed(0)}s</span>
              {#if j.cost_usd}<span class="dot">·</span><span>${j.cost_usd.toFixed(3)}</span>{/if}
            {:else if j.status !== "running" && j.status !== "queued"}
              <span class="dot">·</span><span>{ago(j.created_at)}</span>
            {/if}
          </div>
          {#if j.status === "running" || j.status === "queued"}
            <div class="progress">
              <div class="fill" class:indeterminate={j.progress.fraction == null} style="width:{(j.progress.fraction ?? 0.3) * 100}%"></div>
            </div>
            <span class="msg">{j.progress.message ?? "Working"}</span>
          {:else if j.error}
            <span class="err">{j.error}</span>
          {/if}
        </div>
        {#if j.status === "running" || j.status === "queued"}
          <button class="stop" data-tip="Cancel" aria-label="Cancel" onclick={() => gen.cancel(j.id)}><Square size={11} fill="currentColor" /></button>
        {:else if j.status === "succeeded"}
          <Check size={14} class="ok" />
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .pop {
    position: absolute;
    top: calc(100% + 8px);
    right: -60px;
    width: 380px;
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    border-radius: var(--r-lg);
    background: rgba(28, 26, 24, 0.96);
    backdrop-filter: blur(20px);
    box-shadow: var(--shadow-lift);
    z-index: 80;
    animation: rise 0.18s var(--ease);
    overflow: hidden;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 14px 8px;
  }
  .clear {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .clear:hover {
    color: var(--text);
  }
  .empty {
    margin: 0;
    padding: 4px 14px 16px;
    color: var(--text-3);
    font-size: 12.5px;
  }
  .list {
    overflow: auto;
    padding: 0 6px 6px;
  }
  .job {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 8px;
    border-radius: var(--r-md);
  }
  .job:hover {
    background: var(--hover);
  }
  .thumb {
    width: 52px;
    height: 36px;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface-3);
    display: grid;
    place-items: center;
    color: var(--text-3);
  }
  .failed .thumb {
    color: var(--red);
  }
  .thumb img,
  .thumb video {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .pulse {
    width: 100%;
    height: 100%;
    background: var(--gen-soft), var(--surface-3);
    background-size: 200% 100%;
    animation: shimmer 1.8s linear infinite;
  }
  .body {
    flex: 1;
    min-width: 0;
  }
  .prompt {
    margin: 0;
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    display: flex;
    gap: 4px;
    color: var(--text-3);
    font-size: 11px;
    margin-top: 1px;
    white-space: nowrap;
    overflow: hidden;
  }
  .dot {
    color: var(--text-4);
  }
  .progress {
    height: 3px;
    margin-top: 7px;
    border-radius: 3px;
    background: var(--surface-4);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--gen);
    border-radius: 3px;
    transition: width 0.5s var(--ease);
  }
  .indeterminate {
    width: 40% !important;
    animation: slide 1.4s var(--ease) infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(250%);
    }
  }
  .msg {
    display: block;
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-3);
  }
  .err {
    display: block;
    margin-top: 3px;
    font-size: 11px;
    color: var(--red);
    overflow-wrap: anywhere;
  }
  .stop {
    width: 24px;
    height: 24px;
    display: grid;
    place-items: center;
    border-radius: 99px;
    color: var(--text-3);
    flex: none;
  }
  .stop:hover {
    background: var(--hover);
    color: var(--text);
  }
  .job :global(.ok) {
    color: var(--green);
    flex: none;
    margin-top: 4px;
  }
</style>
