<script lang="ts">
  import { Heart } from "@lucide/svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { isTauri } from "$lib/ipc";
  import { SPONSOR_URL } from "$lib/util/platform";

  let { label = true }: { label?: boolean } = $props();
  const go = () => (isTauri ? openUrl(SPONSOR_URL) : window.open(SPONSOR_URL, "_blank"));
</script>

<button class="sponsor" class:icon={!label} onclick={go} data-tip={label ? null : "Sponsor kimchi on GitHub"} aria-label="Sponsor kimchi on GitHub">
  <Heart size={13} />
  {#if label}<span>Sponsor</span>{/if}
</button>

<style>
  .sponsor {
    -webkit-app-region: no-drag;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 11px;
    border-radius: 99px;
    color: var(--text-2);
    font-size: 12px;
    font-weight: 550;
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
    transition:
      color 0.15s,
      box-shadow 0.15s;
  }
  .sponsor :global(svg) {
    color: #ea4aaa;
    transition: transform 0.25s var(--ease-spring);
  }
  .sponsor:hover {
    color: var(--text);
    box-shadow: inset 0 0 0 1px rgba(234, 74, 170, 0.45);
  }
  .sponsor:hover :global(svg) {
    transform: scale(1.18);
    fill: #ea4aaa;
  }
  .icon {
    width: 28px;
    padding: 0;
    justify-content: center;
  }
</style>
