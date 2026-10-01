<script lang="ts">
  // A number you can type into or drag sideways to scrub, Figma-style.
  let {
    value,
    label,
    unit = "",
    step = 1,
    min = -Infinity,
    max = Infinity,
    decimals = 0,
    onchange,
  }: {
    value: number;
    label: string;
    unit?: string;
    step?: number;
    min?: number;
    max?: number;
    decimals?: number;
    /** `final` is false while dragging. */
    onchange: (v: number, final: boolean) => void;
  } = $props();

  let editing = $state(false);
  let text = $state("");
  const clamp = (v: number) => Math.min(max, Math.max(min, v));
  const fmt = (v: number) => (decimals ? v.toFixed(decimals) : String(Math.round(v)));

  function down(e: PointerEvent) {
    if (editing) return;
    const el = e.currentTarget as HTMLElement;
    const startX = e.clientX;
    const start = value;
    let moved = false;
    el.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const dx = ev.clientX - startX;
      if (Math.abs(dx) > 2) moved = true;
      if (!moved) return;
      const mult = ev.shiftKey ? 10 : ev.altKey ? 0.1 : 1;
      onchange(clamp(start + Math.round(dx / 2) * step * mult), false);
    };
    const up = (ev: PointerEvent) => {
      el.releasePointerCapture(ev.pointerId);
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      if (moved) onchange(value, true);
      else {
        text = fmt(value);
        editing = true;
      }
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  }

  function commit() {
    const v = Number.parseFloat(text);
    if (Number.isFinite(v)) onchange(clamp(v), true);
    editing = false;
  }

  function focus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<div class="scrub" class:editing>
  <span class="label" role="presentation" onpointerdown={down}>{label}</span>
  {#if editing}
    <input
      use:focus
      bind:value={text}
      onblur={commit}
      onkeydown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") editing = false;
        e.stopPropagation();
      }}
    />
  {:else}
    <span class="value mono" role="presentation" onpointerdown={down}>{fmt(value)}<i>{unit}</i></span>
  {/if}
</div>

<style>
  .scrub {
    display: flex;
    align-items: center;
    height: 28px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
    overflow: hidden;
    min-width: 0;
  }
  .scrub:hover {
    box-shadow: inset 0 0 0 1px var(--line-2);
  }
  .editing {
    box-shadow: inset 0 0 0 1px var(--accent-line);
  }
  .label {
    padding: 0 6px 0 9px;
    color: var(--text-3);
    font-size: 11px;
    font-weight: 600;
    cursor: ew-resize;
    flex: none;
  }
  .value {
    flex: 1;
    text-align: right;
    padding-right: 9px;
    font-size: 11.5px;
    cursor: ew-resize;
    white-space: nowrap;
  }
  .value i {
    font-style: normal;
    color: var(--text-3);
    margin-left: 1px;
  }
  input {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    outline: 0;
    text-align: right;
    padding-right: 9px;
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
</style>
