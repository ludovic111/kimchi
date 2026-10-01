<script lang="ts">
  import type { TextStyle } from "$lib/bindings/TextStyle";
  import type { Transform } from "$lib/bindings/Transform";
  import { drawText, ensureFont } from "$lib/util/text";

  let { style, transform, width, height, opacity }: { style: TextStyle; transform: Transform; width: number; height: number; opacity: number } = $props();
  let canvas = $state<HTMLCanvasElement>();

  $effect(() => {
    const c = canvas;
    if (!c) return;
    const s = $state.snapshot(style) as TextStyle;
    const t = $state.snapshot(transform) as Transform;
    const ctx = c.getContext("2d")!;
    drawText(ctx, s, t, width, height);
    // Redraw once the web font is in, if it wasn't yet.
    ensureFont(s).then(() => drawText(ctx, s, t, width, height));
  });
</script>

<canvas bind:this={canvas} {width} {height} style="opacity:{opacity}"></canvas>

<style>
  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }
</style>
