<script lang="ts">
  import { onMount } from "svelte";
  import { ArrowUp, Copy, Film, Image as ImageIcon, KeyRound, Plus, Sparkles, Trash2 } from "@lucide/svelte";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, fileUrl, message } from "$lib/ipc";
  import type { AppInfo } from "$lib/bindings/AppInfo";
  import type { ProjectSummary } from "$lib/bindings/ProjectSummary";
  import { editor } from "$lib/state/editor.svelte";
  import { gen, type Mode } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { ago, short } from "$lib/util/time";
  import Mark from "../ui/Mark.svelte";

  const formats = [
    { id: "16:9", label: "Landscape", w: 1920, h: 1080 },
    { id: "9:16", label: "Vertical", w: 1080, h: 1920 },
    { id: "1:1", label: "Square", w: 1080, h: 1080 },
    { id: "4:5", label: "Portrait", w: 1080, h: 1350 },
    { id: "21:9", label: "Cinema", w: 2560, h: 1080 },
  ];

  let projects = $state<ProjectSummary[]>([]);
  let info = $state<AppInfo | null>(null);
  let prompt = $state("");
  let mode = $state<Mode>("video");
  let format = $state(formats[0]);
  let loaded = $state(false);

  const hour = new Date().getHours();
  const greeting = hour < 5 ? "Up late" : hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";

  onMount(async () => {
    try {
      [projects, info] = await Promise.all([api.listProjects(), api.appInfo()]);
    } catch (e) {
      ui.error(message(e));
    }
    loaded = true;
  });

  async function create(withPrompt = false) {
    try {
      const name = withPrompt && prompt.trim() ? titleFrom(prompt) : "Untitled";
      const view = await api.createProject(name, {
        width: format.w,
        height: format.h,
        fps: 30,
        background: "#000000",
        sample_rate: 48000,
      });
      editor.set(view);
      if (withPrompt && prompt.trim()) {
        gen.compose({ mode, prompt: prompt.trim(), toTimeline: true, target: null });
      }
    } catch (e) {
      ui.error(message(e));
    }
  }

  function titleFrom(p: string) {
    const words = p.trim().split(/\s+/).slice(0, 5).join(" ");
    return words.charAt(0).toUpperCase() + words.slice(1);
  }

  async function open(id: string) {
    try {
      editor.set(await api.openProject(id));
    } catch (e) {
      ui.error(message(e));
    }
  }

  function menu(e: MouseEvent, p: ProjectSummary) {
    ui.openMenu(e, [
      { label: "Open", action: () => open(p.id) },
      {
        label: "Duplicate",
        icon: Copy,
        action: async () => {
          const copy = await api.duplicateProject(p.id);
          projects = [copy, ...projects];
        },
      },
      "separator",
      {
        label: "Delete project",
        icon: Trash2,
        danger: true,
        action: async () => {
          const ok = await ask(`Delete “${p.name}”? Media generated inside it is deleted too.`, { title: "Delete project", kind: "warning", okLabel: "Delete" });
          if (!ok) return;
          await api.deleteProject(p.id);
          projects = projects.filter((x) => x.id !== p.id);
        },
      },
    ]);
  }
</script>

<div class="home">
  <header class="drag" data-tauri-drag-region>
    <div class="brand">
      <Mark size={22} />
      <span>kimchi</span>
    </div>
    <button class="keys" onclick={() => ui.openSettings()}>
      <KeyRound size={14} />
      {#if gen.connected.length}
        {gen.connected.length} model provider{gen.connected.length > 1 ? "s" : ""} connected
      {:else}
        Connect a model provider
      {/if}
    </button>
  </header>

  <main>
    <section class="hero">
      <p class="eyebrow">{greeting}</p>
      <h1>What are we <em>making</em> today?</h1>

      <form
        class="composer"
        onsubmit={(e) => {
          e.preventDefault();
          create(true);
        }}
      >
        <textarea
          rows="2"
          bind:value={prompt}
          placeholder="Describe a shot, a scene, a whole idea — or start from an empty timeline."
          onkeydown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              create(true);
            }
          }}
        ></textarea>
        <div class="row">
          <div class="chips">
            <button type="button" class="chip" class:on={mode === "video"} onclick={() => (mode = "video")}><Film size={13} /> Video</button>
            <button type="button" class="chip" class:on={mode === "image"} onclick={() => (mode = "image")}><ImageIcon size={13} /> Image</button>
            <span class="divider"></span>
            {#each formats as f (f.id)}
              <button type="button" class="chip fmt" class:on={format.id === f.id} title="{f.label} · {f.w}×{f.h}" onclick={() => (format = f)}>
                <span class="ratio" style="--w:{f.w};--h:{f.h}"></span>{f.id}
              </button>
            {/each}
          </div>
          <div class="actions">
            <button type="button" class="empty" onclick={() => create(false)}>Empty project</button>
            <button type="submit" class="go" disabled={!prompt.trim()} aria-label="Create and generate">
              <ArrowUp size={16} strokeWidth={2.4} />
            </button>
          </div>
        </div>
      </form>
      {#if !gen.connected.length && loaded}
        <button class="nudge" onclick={() => ui.openSettings()}>
          <Sparkles size={14} />
          <span>Bring your own key — OpenRouter, fal, Replicate, OpenAI, Google… — or point kimchi at ComfyUI on your machine.</span>
        </button>
      {/if}
    </section>

    <section class="recent">
      <div class="section-head">
        <h2>Projects</h2>
        <button class="new" onclick={() => create(false)}><Plus size={14} /> New</button>
      </div>
      {#if loaded && !projects.length}
        <p class="none">Nothing here yet. Your projects will show up here.</p>
      {/if}
      <div class="grid">
        {#each projects as p (p.id)}
          <button class="card" onclick={() => open(p.id)} oncontextmenu={(e) => menu(e, p)}>
            <div class="cover" style="aspect-ratio:16/10">
              {#if p.cover}
                <img src={fileUrl(p.cover)} alt="" loading="lazy" />
              {:else}
                <div class="placeholder"><Mark size={28} mono /></div>
              {/if}
              {#if p.generated_count}
                <span class="badge"><Sparkles size={11} /> {p.generated_count}</span>
              {/if}
              {#if p.duration > 0}<span class="dur mono">{short(p.duration)}</span>{/if}
            </div>
            <div class="meta">
              <strong>{p.name}</strong>
              <span>{ago(p.updated_at)} · {p.width}×{p.height}</span>
            </div>
          </button>
        {/each}
      </div>
    </section>
  </main>

  {#if info && !info.ffmpeg}
    <div class="warn">ffmpeg wasn't found — import and export need it. Install it with <code>brew install ffmpeg</code>.</div>
  {/if}
</div>

<style>
  .home {
    height: 100%;
    display: flex;
    flex-direction: column;
    background:
      radial-gradient(1200px 520px at 78% -10%, rgba(255, 90, 54, 0.11), transparent 60%),
      radial-gradient(900px 420px at 10% 0%, rgba(166, 207, 94, 0.05), transparent 60%),
      var(--bg);
    overflow: auto;
  }
  header {
    flex: none;
    height: var(--topbar-h);
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 18px 0 92px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 650;
    font-size: 15px;
    letter-spacing: -0.02em;
  }
  .keys {
    -webkit-app-region: no-drag;
    display: flex;
    align-items: center;
    gap: 7px;
    height: 28px;
    padding: 0 11px;
    border-radius: 99px;
    color: var(--text-2);
    font-size: 12px;
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .keys:hover {
    color: var(--text);
    background: var(--surface-3);
  }
  main {
    width: min(1080px, 100% - 64px);
    margin: 0 auto;
    padding: 6vh 0 64px;
  }
  .hero {
    max-width: 760px;
    margin: 0 auto 72px;
    text-align: center;
  }
  h1 {
    margin: 10px 0 28px;
    font-family: var(--font-display);
    font-weight: 400;
    font-size: clamp(40px, 5.4vw, 64px);
    line-height: 1.02;
    letter-spacing: -0.02em;
  }
  h1 em {
    font-style: italic;
    background: var(--gen);
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
    padding-right: 0.06em;
  }
  .composer {
    text-align: left;
    padding: 14px 14px 12px;
    border-radius: 18px;
    background: linear-gradient(var(--surface-2), var(--surface-2)) padding-box;
    box-shadow:
      0 0 0 1px var(--line-2),
      0 24px 60px -24px rgba(0, 0, 0, 0.8),
      0 1px 0 rgba(255, 255, 255, 0.04) inset;
    transition: box-shadow 0.2s var(--ease);
  }
  .composer:focus-within {
    box-shadow:
      0 0 0 1px var(--accent-line),
      0 0 0 5px rgba(255, 90, 54, 0.08),
      0 24px 60px -24px rgba(0, 0, 0, 0.8);
  }
  textarea {
    width: 100%;
    resize: none;
    border: 0;
    outline: 0;
    background: none;
    font-size: 15.5px;
    line-height: 1.5;
    padding: 2px 4px 10px;
    color: var(--text);
  }
  textarea::placeholder {
    color: var(--text-3);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .chips {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 9px;
    border-radius: 99px;
    color: var(--text-3);
    font-size: 12px;
    font-weight: 550;
  }
  .chip:hover {
    color: var(--text-2);
    background: var(--hover);
  }
  .chip.on {
    color: var(--text);
    background: var(--surface-4);
    box-shadow: inset 0 0 0 1px var(--line-2);
  }
  .fmt {
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .ratio {
    display: inline-block;
    height: 10px;
    width: calc(10px * var(--w) / var(--h));
    max-width: 18px;
    border-radius: 2px;
    border: 1.5px solid currentColor;
  }
  .divider {
    width: 1px;
    height: 16px;
    background: var(--line-2);
    margin: 0 4px;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
    padding: 0 8px;
    height: 30px;
    border-radius: 99px;
  }
  .empty:hover {
    color: var(--text);
  }
  .go {
    width: 34px;
    height: 34px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    background: var(--gen);
    color: #1b0c05;
    box-shadow: 0 6px 20px -6px rgba(255, 110, 60, 0.8);
    transition:
      transform 0.15s var(--ease-spring),
      opacity 0.2s;
  }
  .go:hover:not(:disabled) {
    transform: scale(1.07);
  }
  .go:disabled {
    opacity: 0.25;
    box-shadow: none;
  }
  .nudge {
    margin-top: 16px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    color: var(--text-3);
    font-size: 12.5px;
    text-align: left;
  }
  .nudge :global(svg) {
    color: var(--amber);
    flex: none;
  }
  .nudge:hover {
    color: var(--text-2);
  }
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 14px;
  }
  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }
  .new {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--text-2);
    font-size: 12.5px;
    height: 28px;
    padding: 0 10px;
    border-radius: var(--r-sm);
  }
  .new:hover {
    background: var(--hover);
    color: var(--text);
  }
  .none {
    color: var(--text-3);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 22px 18px;
  }
  .card {
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .cover {
    position: relative;
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
    transition:
      transform 0.25s var(--ease),
      box-shadow 0.25s var(--ease);
  }
  .card:hover .cover {
    transform: translateY(-2px);
    box-shadow:
      0 0 0 1px var(--line-3),
      0 18px 40px -18px rgba(0, 0, 0, 0.8);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .placeholder {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    opacity: 0.25;
  }
  .badge,
  .dur {
    position: absolute;
    bottom: 8px;
    padding: 2px 7px;
    border-radius: 99px;
    font-size: 10.5px;
    background: rgba(10, 8, 7, 0.7);
    backdrop-filter: blur(8px);
  }
  .badge {
    left: 8px;
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--amber);
  }
  .dur {
    right: 8px;
    color: var(--text-2);
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 0 2px;
  }
  .meta strong {
    font-weight: 600;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta span {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .warn {
    position: fixed;
    left: 50%;
    bottom: 18px;
    transform: translateX(-50%);
    padding: 9px 14px;
    border-radius: 99px;
    background: rgba(240, 180, 76, 0.12);
    color: var(--amber);
    font-size: 12px;
    box-shadow: inset 0 0 0 1px rgba(240, 180, 76, 0.25);
  }
  code {
    font-family: var(--font-mono);
  }
</style>
