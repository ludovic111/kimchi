<script lang="ts" module>
  import { api } from "$lib/ipc";

  const cache = new Map<string, Promise<Float32Array>>();

  function peaks(path: string): Promise<Float32Array> {
    let p = cache.get(path);
    if (!p) {
      p = api.readPeaks(path).then((buf) => new Float32Array(buf));
      cache.set(path, p);
    }
    return p;
  }
</script>

<script lang="ts">
  import type { Waveform } from "$lib/bindings/Waveform";

  let {
    waveform,
    from,
    to,
    width,
    height,
    color = "rgba(166, 207, 94, 0.75)",
  }: { waveform: Waveform; from: number; to: number; width: number; height: number; color?: string } = $props();

  let canvas = $state<HTMLCanvasElement>();
  const px = $derived(Math.max(1, Math.min(4096, Math.round(width))));

  $effect(() => {
    const c = canvas;
    const w = px;
    const h = height;
    const [a, b] = [from, to];
    const fill = color;
    if (!c) return;
    let cancelled = false;
    peaks(waveform.path).then((data) => {
      if (cancelled) return;
      const dpr = window.devicePixelRatio || 1;
      c.width = w * dpr;
      c.height = h * dpr;
      const ctx = c.getContext("2d")!;
      ctx.scale(dpr, dpr);
      ctx.clearRect(0, 0, w, h);
      ctx.fillStyle = fill;
      const pps = waveform.peaks_per_second;
      const span = Math.max(1e-6, b - a);
      const mid = h / 2;
      for (let x = 0; x < w; x += 2) {
        const i0 = Math.floor((a + (x / w) * span) * pps);
        const i1 = Math.max(i0 + 1, Math.floor((a + ((x + 2) / w) * span) * pps));
        let peak = 0;
        for (let i = i0; i < i1 && i < data.length; i++) peak = Math.max(peak, data[i]);
        const bar = Math.max(1, peak * (h - 2));
        ctx.fillRect(x, mid - bar / 2, 1.4, bar);
      }
    });
    return () => (cancelled = true);
  });
</script>

<canvas bind:this={canvas} style="width:{width}px;height:{height}px"></canvas>

<style>
  canvas {
    position: absolute;
    left: 0;
    bottom: 0;
    pointer-events: none;
  }
</style>
