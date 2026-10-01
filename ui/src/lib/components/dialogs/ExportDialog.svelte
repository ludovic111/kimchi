<script lang="ts">
  import { onMount } from "svelte";
  import { Check, FolderSearch, LoaderCircle } from "@lucide/svelte";
  import { save } from "@tauri-apps/plugin-dialog";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { api, events, message } from "$lib/ipc";
  import type { ExportFormat } from "$lib/bindings/ExportFormat";
  import type { Quality } from "$lib/bindings/Quality";
  import { editor } from "$lib/state/editor.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { textPng } from "$lib/util/text";
  import { short } from "$lib/util/time";
  import Button from "../ui/Button.svelte";
  import Dialog from "../ui/Dialog.svelte";
  import Segmented from "../ui/Segmented.svelte";

  const FORMATS: { id: ExportFormat; name: string; ext: string; desc: string }[] = [
    { id: "mp4", name: "MP4", ext: "mp4", desc: "H.264 · plays everywhere" },
    { id: "hevc", name: "HEVC", ext: "mp4", desc: "H.265 · half the size" },
    { id: "prores", name: "ProRes", ext: "mov", desc: "422 HQ · for finishing" },
    { id: "webm", name: "WebM", ext: "webm", desc: "VP9 · for the web" },
    { id: "gif", name: "GIF", ext: "gif", desc: "Loops, no sound" },
    { id: "audio", name: "Audio", ext: "m4a", desc: "AAC soundtrack only" },
  ];

  let format = $state<ExportFormat>("mp4");
  let quality = $state<Quality>("standard");
  let size = $state<"project" | "720" | "1080" | "2160">("project");
  let job = $state<{ id: string; progress: number; done: boolean; error: string | null; path: string } | null>(null);
  let preparing = $state(false);

  const s = $derived(editor.project?.settings);
  const dims = $derived.by(() => {
    if (!s || size === "project") return { w: s?.width ?? 1920, h: s?.height ?? 1080 };
    const short = Number(size);
    const landscape = s.width >= s.height;
    const k = short / (landscape ? s.height : s.width);
    const even = (v: number) => Math.round((v * k) / 2) * 2;
    return { w: even(s.width), h: even(s.height) };
  });

  onMount(() => {
    const off = events.exportProgress((e) => {
      if (job && e.id === job.id) job = { ...job, progress: e.progress, done: e.done, error: e.error };
    });
    return () => void off.then((f) => f());
  });

  async function start() {
    const p = editor.project;
    if (!p) return;
    const f = FORMATS.find((x) => x.id === format)!;
    const path = await save({ defaultPath: `${p.name}.${f.ext}`, filters: [{ name: f.name, extensions: [f.ext] }] });
    if (!path) return;
    preparing = true;
    try {
      // Text is rasterised here, with the same code as the preview.
      const overlays: Record<string, string> = {};
      for (const t of p.tracks) {
        if (t.hidden || t.kind !== "video") continue;
        for (const c of t.clips) {
          if (c.content.type !== "text") continue;
          const png = await textPng(c.content.style, c.transform, p.settings.width, p.settings.height);
          overlays[c.id] = await api.writePng(`text-${c.id}`, png);
        }
      }
      const id = await api.exportStart(
        {
          path,
          format,
          quality,
          width: size === "project" ? null : dims.w,
          height: size === "project" ? null : dims.h,
          fps: null,
          range: null,
        },
        overlays,
      );
      job = { id, progress: 0, done: false, error: null, path };
    } catch (e) {
      ui.error(message(e));
    } finally {
      preparing = false;
    }
  }

  function close() {
    if (job && !job.done) return;
    ui.exportOpen = false;
    job = null;
  }
</script>

<Dialog open={ui.exportOpen} onclose={close} title="Export" subtitle={editor.project ? `${editor.project.name} · ${short(editor.duration)}` : ""} width={600}>
  {#if job}
    <div class="status">
      {#if job.error}
        <p class="err">{job.error}</p>
      {:else if job.done}
        <div class="done"><Check size={20} /></div>
        <strong>Exported</strong>
        <span class="path mono">{job.path}</span>
      {:else}
        <div class="big mono">{Math.round(job.progress * 100)}<span>%</span></div>
        <div class="bar"><div style="width:{job.progress * 100}%"></div></div>
        <span class="path mono">{job.path}</span>
      {/if}
    </div>
  {:else}
    <div class="formats">
      {#each FORMATS as f (f.id)}
        <button class="fmt" class:on={format === f.id} onclick={() => (format = f.id)}>
          <strong>{f.name}</strong>
          <span>{f.desc}</span>
        </button>
      {/each}
    </div>

    {#if format !== "audio"}
      <div class="row">
        <span class="eyebrow">Resolution</span>
        <Segmented
          size="sm"
          value={size}
          onchange={(v) => (size = v)}
          options={[
            { value: "project", label: `${s?.width}×${s?.height}` },
            { value: "720", label: "720p" },
            { value: "1080", label: "1080p" },
            { value: "2160", label: "4K" },
          ]}
        />
      </div>
    {/if}
    <div class="row">
      <span class="eyebrow">Quality</span>
      <Segmented
        size="sm"
        value={quality}
        onchange={(v) => (quality = v)}
        options={[
          { value: "draft", label: "Draft" },
          { value: "standard", label: "Standard" },
          { value: "high", label: "High" },
        ]}
      />
    </div>
  {/if}

  {#snippet footer()}
    {#if job?.done}
      {#if !job.error}<Button onclick={() => revealItemInDir(job!.path)}><FolderSearch size={13} /> Show in Finder</Button>{/if}
      <Button variant="primary" onclick={close}>Done</Button>
    {:else if job}
      <Button variant="danger" onclick={() => api.exportCancel(job!.id)}>Cancel</Button>
    {:else}
      <Button variant="ghost" onclick={close}>Cancel</Button>
      <Button variant="primary" disabled={preparing || editor.duration <= 0} onclick={start}>
        {#if preparing}<LoaderCircle size={13} class="spin" />{/if}
        Export {format === "audio" ? "audio" : `${dims.w}×${dims.h}`}
      </Button>
    {/if}
  {/snippet}
</Dialog>

<style>
  .formats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
    margin-bottom: 18px;
  }
  .fmt {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 12px;
    border-radius: var(--r-md);
    text-align: left;
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
    transition: box-shadow 0.15s, background 0.15s;
  }
  .fmt:hover {
    background: var(--surface-3);
  }
  .fmt.on {
    background: var(--accent-soft);
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .fmt strong {
    font-weight: 650;
  }
  .fmt span {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .row {
    display: grid;
    grid-template-columns: 110px 1fr;
    align-items: center;
    margin-bottom: 12px;
  }
  .status {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 24px 0 8px;
    text-align: center;
  }
  .big {
    font-size: 48px;
    font-weight: 300;
    letter-spacing: -0.03em;
  }
  .big span {
    font-size: 22px;
    color: var(--text-3);
  }
  .bar {
    width: 100%;
    height: 6px;
    border-radius: 6px;
    background: var(--surface-4);
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: linear-gradient(90deg, var(--accent), var(--accent-hi));
    transition: width 0.3s var(--ease);
  }
  .path {
    font-size: 11px;
    color: var(--text-3);
    overflow-wrap: anywhere;
  }
  .done {
    width: 44px;
    height: 44px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    background: var(--green-soft);
    color: var(--green);
  }
  .err {
    color: var(--red);
    white-space: pre-wrap;
    text-align: left;
    font-family: var(--font-mono);
    font-size: 11.5px;
    margin: 0;
  }
  :global(.spin) {
    animation: spin 0.8s linear infinite;
  }
</style>
