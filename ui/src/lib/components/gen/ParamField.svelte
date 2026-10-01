<script lang="ts">
  import type { ParamSpec } from "$lib/bindings/ParamSpec";
  import Scrub from "../ui/Scrub.svelte";
  import Toggle from "../ui/Toggle.svelte";

  let { spec, value, onchange }: { spec: ParamSpec; value: unknown; onchange: (v: unknown) => void } = $props();
  const k = $derived(spec.kind);
</script>

<label class="field" title={spec.help ?? ""}>
  {#if k.type === "int" || k.type === "float"}
    <Scrub
      label={spec.label}
      value={typeof value === "number" ? value : Number(spec.default ?? 0)}
      min={k.min}
      max={k.max}
      step={k.step}
      decimals={k.type === "float" ? Math.max(0, -Math.floor(Math.log10(k.step || 1))) : 0}
      onchange={(v) => onchange(v)}
    />
  {:else if k.type === "bool"}
    <div class="row"><span>{spec.label}</span><Toggle checked={Boolean(value ?? spec.default)} onchange={onchange} /></div>
  {:else if k.type === "select"}
    <div class="row">
      <span>{spec.label}</span>
      <select value={String(value ?? spec.default ?? "")} onchange={(e) => onchange((e.currentTarget as HTMLSelectElement).value)}>
        {#each k.options as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
      </select>
    </div>
  {:else}
    <span class="lbl">{spec.label}</span>
    {#if k.multiline}
      <textarea rows="3" value={String(value ?? spec.default ?? "")} oninput={(e) => onchange((e.currentTarget as HTMLTextAreaElement).value)}></textarea>
    {:else}
      <input value={String(value ?? spec.default ?? "")} oninput={(e) => onchange((e.currentTarget as HTMLInputElement).value)} />
    {/if}
  {/if}
</label>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    height: 28px;
    font-size: 12px;
    color: var(--text-2);
  }
  .lbl {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-3);
  }
  select,
  input,
  textarea {
    background: var(--surface-2);
    border: 0;
    outline: 0;
    border-radius: var(--r-sm);
    box-shadow: inset 0 0 0 1px var(--line);
    padding: 5px 8px;
    font-size: 12px;
  }
  select {
    max-width: 60%;
    -webkit-appearance: none;
    appearance: none;
    padding-right: 22px;
    background-image: linear-gradient(45deg, transparent 50%, var(--text-3) 50%), linear-gradient(135deg, var(--text-3) 50%, transparent 50%);
    background-position:
      calc(100% - 12px) 50%,
      calc(100% - 8px) 50%;
    background-size: 4px 4px;
    background-repeat: no-repeat;
  }
  textarea {
    resize: vertical;
  }
  input:focus,
  textarea:focus,
  select:focus {
    box-shadow: inset 0 0 0 1px var(--accent-line);
  }
</style>
