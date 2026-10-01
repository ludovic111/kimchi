<script lang="ts">
  import { ArrowUp, Clapperboard, Dices, Film, ImagePlus, Image as ImageIcon, MapPin, ScanLine, Sparkles, X } from "@lucide/svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, fileUrl, message } from "$lib/ipc";
  import type { ImageRole } from "$lib/bindings/ImageRole";
  import { drag } from "$lib/state/drag.svelte";
  import { editor } from "$lib/state/editor.svelte";
  import { aspectOf, gen } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import { timecode } from "$lib/util/time";
  import Segmented from "../ui/Segmented.svelte";
  import Toggle from "../ui/Toggle.svelte";
  import ModelPicker from "./ModelPicker.svelte";
  import ParamField from "./ParamField.svelte";
  import RecentGenerations from "./RecentGenerations.svelte";

  const COMMON_RATIOS = ["16:9", "9:16", "1:1", "4:3", "3:4", "21:9"];
  const d = $derived(gen.draft);
  const m = $derived(gen.model);
  const projectRatio = $derived(editor.project ? aspectOf(editor.project.settings.width, editor.project.settings.height) : "16:9");
  const ratios = $derived(m?.aspect_ratios.length ? m.aspect_ratios : COMMON_RATIOS);
  const video = $derived(d.mode === "video");
  const maxRefs = $derived(video ? (m?.end_frame ? 2 : 1) : Math.max(1, m?.max_images ?? 1));
  const canSubmit = $derived(!!m && (d.prompt.trim().length > 0 || d.refs.length > 0));
  let showAdvanced = $state(false);
  let composerEl = $state<HTMLDivElement>();

  // Let library tiles be dropped on the composer.
  $effect(() => {
    if (!drag.active || !composerEl) return;
    const r = composerEl.getBoundingClientRect();
    drag.overComposer = drag.x >= r.left && drag.x <= r.right && drag.y >= r.top && drag.y <= r.bottom;
  });

  function nextRole(): ImageRole {
    if (!video) return "reference";
    return d.refs.some((r) => r.role === "start_frame") && m?.end_frame ? "end_frame" : "start_frame";
  }

  async function pickFile(role: ImageRole = nextRole()) {
    const p = await open({ multiple: false, filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "heic", "avif"] }] });
    if (typeof p === "string") gen.addRef({ role, path: p, assetId: null, label: p.split(/[\\/]/).pop() ?? "image" });
  }

  async function grabPlayhead(role: ImageRole = nextRole()) {
    const clip = editor.selectedClips.find((c) => c.content.type === "media" && editor.playhead >= c.start && editor.playhead < c.start + c.duration) ?? editor.clipAt(editor.playhead);
    if (!clip) return ui.toast("Move the playhead over a video or image clip first.");
    try {
      const path = await api.clipFrame(clip.id, editor.playhead);
      gen.addRef({ role, path, assetId: clip.content.type === "media" ? clip.content.asset_id : null, label: `${clip.name} @ ${timecode(editor.playhead, editor.fps)}` });
    } catch (e) {
      ui.error(message(e));
    }
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      if (canSubmit) gen.submit();
    }
    e.stopPropagation();
  }

  let promptEl = $state<HTMLTextAreaElement>();
  // Grow with the prompt, including when it's filled from elsewhere.
  $effect(() => {
    void gen.draft.prompt;
    if (!promptEl) return;
    promptEl.style.height = "auto";
    promptEl.style.height = `${Math.min(260, promptEl.scrollHeight)}px`;
  });

  const roleLabel = (r: ImageRole) => (r === "start_frame" ? "Start" : r === "end_frame" ? "End" : "Ref");
</script>

<div class="head">
  <h3><span class="gen-text">Generate</span></h3>
  <Segmented
    size="sm"
    value={d.mode}
    onchange={(v) => gen.setMode(v)}
    options={[
      { value: "image", label: "Image", icon: ImageIcon },
      { value: "video", label: "Video", icon: Film },
    ]}
  />
</div>

<div class="scroll">
  {#if !gen.connected.length && !gen.loadingModels}
    <div class="connect">
      <div class="glow"><Sparkles size={18} /></div>
      <strong>Connect a model</strong>
      <p>Bring an API key from OpenRouter, fal, Replicate, OpenAI, Google, Runway, Luma… or run models locally with ComfyUI, Forge or Ollama.</p>
      <div class="quick">
        {#each ["openrouter", "fal", "replicate", "openai", "google", "comfyui"] as id (id)}
          <button onclick={() => ui.openSettings(id)}>{gen.providerName(id)}</button>
        {/each}
      </div>
    </div>
  {/if}

  <div class="composer" class:drop={drag.overComposer} bind:this={composerEl}>
    {#if d.target}
      <div class="target">
        <MapPin size={12} />
        <span>Lands {d.target.label}</span>
        <button aria-label="Clear target" onclick={() => (gen.draft.target = null)}><X size={12} /></button>
      </div>
    {/if}
    <textarea
      id="prompt"
      bind:this={promptEl}
      rows="4"
      placeholder={video ? "A slow dolly through a neon-lit market at night, rain on the lens…" : "An overhead shot of a ceramic bowl of kimchi on linen, soft morning light…"}
      bind:value={gen.draft.prompt}
      onkeydown={keydown}
    ></textarea>

    <div class="refs">
      {#each d.refs as r, i (r.path + i)}
        <div class="ref">
          <img src={fileUrl(r.path)} alt={r.label} title={r.label} />
          <span class="role">{roleLabel(r.role)}</span>
          <button class="rm" aria-label="Remove" onclick={() => gen.removeRef(i)}><X size={10} /></button>
        </div>
      {/each}
      {#if d.refs.length < maxRefs}
        <button class="add" data-tip={video ? (d.refs.length ? "End frame" : "Start frame") : "Reference image"} onclick={() => pickFile()}>
          <ImagePlus size={15} />
        </button>
        <button class="add" data-tip="Frame at playhead" onclick={() => grabPlayhead()}>
          <ScanLine size={15} />
        </button>
      {/if}
      {#if drag.active}<span class="drophint">Drop an image</span>{/if}
    </div>

    <div class="bottom">
      <ModelPicker />
      <button class="go" disabled={!canSubmit} onclick={() => gen.submit()} data-tip="Generate (⌘↵)" aria-label="Generate">
        <ArrowUp size={17} strokeWidth={2.4} />
      </button>
    </div>
  </div>

  {#if m}
    <div class="settings">
      <div class="setting">
        <span class="eyebrow">Aspect</span>
        <div class="chips">
          {#each ratios as r (r)}
            <button class="chip" class:on={(d.aspect ?? projectRatio) === r} onclick={() => (gen.draft.aspect = r === projectRatio ? null : r)}>
              <span class="ratio" style="--r:{r.replace(':', '/')}"></span>{r}
            </button>
          {/each}
        </div>
      </div>

      {#if video && m.durations.length}
        <div class="setting">
          <span class="eyebrow">Length</span>
          <div class="chips">
            {#each m.durations as s (s)}
              <button class="chip" class:on={(d.duration ?? m.durations[0]) === s} onclick={() => (gen.draft.duration = s)}>{s}s</button>
            {/each}
          </div>
        </div>
      {/if}

      {#if m.resolutions.length > 1}
        <div class="setting">
          <span class="eyebrow">Quality</span>
          <div class="chips">
            {#each m.resolutions as r (r)}
              <button class="chip" class:on={(d.resolution ?? m.resolutions[0]) === r} onclick={() => (gen.draft.resolution = r)}>{r}</button>
            {/each}
          </div>
        </div>
      {/if}

      {#if m.max_outputs > 1}
        <div class="setting">
          <span class="eyebrow">Variations</span>
          <div class="chips">
            {#each [1, 2, 3, 4].filter((n) => n <= m.max_outputs) as n (n)}
              <button class="chip" class:on={d.count === n} onclick={() => (gen.draft.count = n)}>{n}</button>
            {/each}
          </div>
        </div>
      {/if}

      {#if m.audio}
        <div class="setting inline">
          <span class="eyebrow">Sound</span>
          <Toggle checked={d.audio} onchange={(v) => (gen.draft.audio = v)} />
        </div>
      {/if}

      {#if m.seed || m.negative_prompt || m.params.length}
        <button class="more" onclick={() => (showAdvanced = !showAdvanced)}>{showAdvanced ? "Fewer options" : "More options"}</button>
      {/if}

      {#if showAdvanced}
        <div class="advanced">
          {#if m.negative_prompt}
            <label class="field">
              <span class="eyebrow">Avoid</span>
              <input placeholder="blurry, text, watermark" bind:value={gen.draft.negative} onkeydown={(e) => e.stopPropagation()} />
            </label>
          {/if}
          {#if m.seed}
            <label class="field">
              <span class="eyebrow">Seed</span>
              <div class="seed">
                <input class="mono" placeholder="random" bind:value={gen.draft.seed} onkeydown={(e) => e.stopPropagation()} />
                <button aria-label="Random seed" data-tip="Roll" onclick={() => (gen.draft.seed = String(Math.floor(Math.random() * 2 ** 31)))}><Dices size={14} /></button>
              </div>
            </label>
          {/if}
          {#each m.params as p (p.key)}
            <ParamField spec={p} value={d.params[p.key]} onchange={(v) => (gen.draft.params = { ...gen.draft.params, [p.key]: v })} />
          {/each}
        </div>
      {/if}

      <div class="setting inline dest">
        <span>
          <Clapperboard size={13} />
          {#if d.toTimeline}
            On the timeline {d.target ? "" : `at ${timecode(editor.playhead, editor.fps)}`}
          {:else}
            Library only
          {/if}
        </span>
        <Toggle checked={d.toTimeline} onchange={(v) => (gen.draft.toTimeline = v)} />
      </div>
    </div>
  {/if}

  <RecentGenerations />
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px 10px;
    gap: 12px;
  }
  .head :global(.seg) {
    width: 170px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 650;
  }
  .scroll {
    flex: 1;
    overflow: auto;
    padding: 0 12px 16px;
  }
  .connect {
    margin-bottom: 12px;
    padding: 16px;
    border-radius: var(--r-lg);
    background: var(--gen-soft), var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line-2);
  }
  .glow {
    width: 32px;
    height: 32px;
    border-radius: 10px;
    display: grid;
    place-items: center;
    background: var(--gen);
    color: #1b0c05;
    margin-bottom: 10px;
  }
  .connect strong {
    font-weight: 650;
  }
  .connect p {
    margin: 4px 0 12px;
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.5;
  }
  .quick {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .quick button {
    height: 26px;
    padding: 0 10px;
    border-radius: 99px;
    font-size: 11.5px;
    font-weight: 550;
    background: rgba(0, 0, 0, 0.25);
    box-shadow: inset 0 0 0 1px var(--line-2);
  }
  .quick button:hover {
    background: rgba(0, 0, 0, 0.4);
  }
  .composer {
    border-radius: var(--r-lg);
    background: var(--surface-2);
    box-shadow:
      inset 0 0 0 1px var(--line-2),
      0 12px 30px -18px rgba(0, 0, 0, 0.8);
    transition: box-shadow 0.2s var(--ease);
  }
  .composer:focus-within {
    box-shadow:
      inset 0 0 0 1px rgba(255, 138, 61, 0.45),
      0 0 0 4px rgba(255, 110, 60, 0.07),
      0 12px 30px -18px rgba(0, 0, 0, 0.8);
  }
  .composer.drop {
    box-shadow:
      inset 0 0 0 1.5px var(--accent),
      0 0 0 4px rgba(255, 90, 54, 0.12);
  }
  .target {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 10px 12px 0;
    padding: 4px 6px 4px 8px;
    border-radius: 99px;
    width: fit-content;
    max-width: calc(100% - 24px);
    font-size: 11.5px;
    color: var(--amber);
    background: rgba(240, 180, 76, 0.1);
  }
  .target span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .target button {
    color: var(--amber);
    display: grid;
    place-items: center;
    opacity: 0.7;
  }
  .target button:hover {
    opacity: 1;
  }
  textarea {
    display: block;
    width: 100%;
    resize: none;
    background: none;
    border: 0;
    outline: 0;
    padding: 12px 14px 6px;
    font-size: 13.5px;
    line-height: 1.5;
    min-height: 92px;
  }
  textarea::placeholder {
    color: var(--text-4);
  }
  .refs {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px 8px;
    flex-wrap: wrap;
  }
  .ref {
    position: relative;
    width: 52px;
    height: 40px;
    border-radius: 8px;
    overflow: hidden;
    box-shadow: 0 0 0 1px var(--line-3);
  }
  .ref img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .role {
    position: absolute;
    left: 3px;
    bottom: 3px;
    padding: 0 4px;
    border-radius: 4px;
    font-size: 9px;
    font-weight: 650;
    background: rgba(10, 8, 7, 0.75);
  }
  .rm {
    position: absolute;
    top: 3px;
    right: 3px;
    width: 16px;
    height: 16px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    background: rgba(10, 8, 7, 0.75);
    opacity: 0;
    transition: opacity 0.12s;
  }
  .ref:hover .rm {
    opacity: 1;
  }
  .add {
    width: 40px;
    height: 40px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    color: var(--text-3);
    border: 1px dashed var(--line-3);
  }
  .add:hover {
    color: var(--text);
    border-color: var(--text-3);
  }
  .drophint {
    font-size: 11px;
    color: var(--accent-hi);
  }
  .bottom {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 6px 6px 4px;
    border-top: 1px solid var(--line);
  }
  .go {
    width: 36px;
    height: 36px;
    flex: none;
    border-radius: 11px;
    display: grid;
    place-items: center;
    background: var(--gen);
    background-size: 160% 100%;
    color: #1b0c05;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.35),
      0 8px 22px -8px rgba(255, 110, 60, 0.8);
    transition:
      transform 0.15s var(--ease-spring),
      background-position 0.4s var(--ease),
      opacity 0.2s;
  }
  .go:hover:not(:disabled) {
    transform: translateY(-1px);
    background-position: 100% 0;
  }
  .go:active:not(:disabled) {
    transform: scale(0.94);
  }
  .go:disabled {
    opacity: 0.3;
    box-shadow: none;
  }
  .settings {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px 2px 4px;
  }
  .setting {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .setting.inline {
    flex-direction: row;
    align-items: center;
    justify-content: space-between;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 9px;
    border-radius: 8px;
    font-size: 11.5px;
    font-family: var(--font-mono);
    color: var(--text-3);
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .chip:hover {
    color: var(--text-2);
  }
  .chip.on {
    color: var(--text);
    background: var(--surface-4);
    box-shadow: inset 0 0 0 1px var(--line-3);
  }
  .ratio {
    display: inline-block;
    height: 10px;
    aspect-ratio: var(--r);
    max-width: 20px;
    border-radius: 2px;
    border: 1.5px solid currentColor;
  }
  .more {
    align-self: flex-start;
    color: var(--text-3);
    font-size: 12px;
  }
  .more:hover {
    color: var(--text);
  }
  .advanced {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border-radius: var(--r-md);
    background: var(--bg-sunken);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .field input {
    background: var(--surface-2);
    border: 0;
    outline: 0;
    border-radius: var(--r-sm);
    box-shadow: inset 0 0 0 1px var(--line);
    padding: 6px 8px;
    font-size: 12px;
    width: 100%;
  }
  .seed {
    display: flex;
    gap: 6px;
  }
  .seed button {
    width: 30px;
    flex: none;
    border-radius: var(--r-sm);
    display: grid;
    place-items: center;
    color: var(--text-2);
    background: var(--surface-3);
  }
  .dest {
    padding: 10px 12px;
    border-radius: var(--r-md);
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
    font-size: 12px;
    color: var(--text-2);
  }
  .dest span {
    display: flex;
    align-items: center;
    gap: 7px;
  }
</style>
