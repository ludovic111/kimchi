<script lang="ts">
  // Drag handle between two panes. `invert` for panes anchored right/bottom.
  let {
    axis,
    value,
    min,
    max,
    invert = false,
    onchange,
  }: { axis: "x" | "y"; value: number; min: number; max: number; invert?: boolean; onchange: (v: number) => void } = $props();

  let active = $state(false);

  function down(e: PointerEvent) {
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    active = true;
    const start = axis === "x" ? e.clientX : e.clientY;
    const startValue = value;
    const move = (ev: PointerEvent) => {
      const d = (axis === "x" ? ev.clientX : ev.clientY) - start;
      onchange(Math.min(max, Math.max(min, startValue + (invert ? -d : d))));
    };
    const up = () => {
      active = false;
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  }
</script>

<div class="split {axis}" class:active role="separator" aria-orientation={axis === "x" ? "vertical" : "horizontal"} onpointerdown={down}></div>

<style>
  .split {
    position: relative;
    z-index: 5;
    background: var(--line);
  }
  .x {
    width: 1px;
    cursor: col-resize;
  }
  .y {
    height: 1px;
    cursor: row-resize;
  }
  .split::before {
    content: "";
    position: absolute;
    inset: -4px 0;
  }
  .x::before {
    inset: 0 -4px;
  }
  .split::after {
    content: "";
    position: absolute;
    background: var(--accent);
    opacity: 0;
    transition: opacity 0.15s;
  }
  .x::after {
    inset: 0 -1px;
  }
  .y::after {
    inset: -1px 0;
  }
  .split:hover::after,
  .active::after {
    opacity: 0.7;
    transition-delay: 0.1s;
  }
</style>
