<script lang="ts">
  let { checked, onchange, label }: { checked: boolean; onchange: (v: boolean) => void; label?: string } = $props();
</script>

<button class="toggle" role="switch" aria-checked={checked} aria-label={label} onclick={() => onchange(!checked)}>
  <span class="track" class:on={checked}><span class="knob"></span></span>
  {#if label}<span class="label">{label}</span>{/if}
</button>

<style>
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    color: var(--text-2);
  }
  .track {
    width: 28px;
    height: 16px;
    border-radius: 99px;
    background: var(--surface-4);
    box-shadow: inset 0 0 0 1px var(--line-2);
    position: relative;
    transition: background 0.2s var(--ease);
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 12px;
    height: 12px;
    border-radius: 99px;
    background: var(--text);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.4);
    transition: transform 0.22s var(--ease-spring);
  }
  .on {
    background: var(--accent);
  }
  .on .knob {
    transform: translateX(12px);
  }
  .label {
    font-size: 12.5px;
  }
</style>
