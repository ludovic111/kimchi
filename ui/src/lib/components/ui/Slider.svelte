<script lang="ts">
  let {
    value,
    min = 0,
    max = 1,
    step = 0.01,
    onchange,
  }: { value: number; min?: number; max?: number; step?: number; onchange: (v: number, final: boolean) => void } = $props();
  const pct = $derived(((value - min) / (max - min)) * 100);
</script>

<input
  type="range"
  {min}
  {max}
  {step}
  {value}
  style="--p:{pct}%"
  oninput={(e) => onchange(Number((e.currentTarget as HTMLInputElement).value), false)}
  onchange={(e) => onchange(Number((e.currentTarget as HTMLInputElement).value), true)}
/>

<style>
  input {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    height: 18px;
    background: transparent;
    margin: 0;
  }
  input::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: 4px;
    background: linear-gradient(to right, var(--text-2) var(--p), var(--surface-4) var(--p));
  }
  input::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    margin-top: -5px;
    border-radius: 99px;
    background: var(--text);
    box-shadow:
      0 1px 3px rgba(0, 0, 0, 0.5),
      0 0 0 3px rgba(0, 0, 0, 0.15);
    transition: transform 0.15s var(--ease-spring);
  }
  input:active::-webkit-slider-thumb {
    transform: scale(1.15);
  }
</style>
