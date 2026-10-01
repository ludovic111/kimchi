<script lang="ts">
  // Keeps one <video>/<audio> element in step with the timeline clock.
  import type { Clip } from "$lib/bindings/Clip";
  import { editor } from "$lib/state/editor.svelte";
  import { volumeAt } from "$lib/util/layout";

  let {
    clip,
    src,
    kind,
    muted,
    style = "",
  }: { clip: Clip; src: string; kind: "video" | "audio"; muted: boolean; style?: string } = $props();

  let el = $state<HTMLMediaElement>();
  const active = $derived(editor.playhead >= clip.start && editor.playhead < clip.start + clip.duration);
  const target = $derived(clip.in_point + (Math.min(Math.max(editor.playhead, clip.start), clip.start + clip.duration) - clip.start) * clip.speed);

  $effect(() => {
    const m = el;
    if (!m) return;
    m.playbackRate = clip.speed;
    m.muted = muted;
    m.volume = muted ? 0 : Math.max(0, Math.min(1, volumeAt(clip, editor.playhead)));
    const drift = Math.abs(m.currentTime - target);
    if (editor.playing && active) {
      if (drift > 0.15) m.currentTime = target;
      if (m.paused) m.play().catch(() => {});
    } else {
      if (!m.paused) m.pause();
      // Scrubbing: land on the exact frame.
      if (drift > 0.5 / editor.fps) m.currentTime = target;
    }
  });
</script>

{#if kind === "video"}
  <video bind:this={el} {src} preload="auto" playsinline {style} class:hidden={!active}></video>
{:else}
  <audio bind:this={el} {src} preload="auto"></audio>
{/if}

<style>
  video {
    position: absolute;
    display: block;
    pointer-events: none;
  }
  .hidden {
    visibility: hidden;
  }
</style>
