<script lang="ts">
  import {
    AlignCenter,
    AlignLeft,
    AlignRight,
    ArrowRightToLine,
    Clapperboard,
    Copy,
    FolderSearch,
    Plus,
    RefreshCw,
    RotateCcw,
    Shuffle,
    Sparkles,
    Wand2,
    Waypoints,
  } from "@lucide/svelte";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { fileUrl } from "$lib/ipc";
  import type { Asset } from "$lib/bindings/Asset";
  import type { Clip } from "$lib/bindings/Clip";
  import type { ClipPatch } from "$lib/bindings/ClipPatch";
  import type { ProjectSettings } from "$lib/bindings/ProjectSettings";
  import type { TextStyle } from "$lib/bindings/TextStyle";
  import type { Transform } from "$lib/bindings/Transform";
  import { editor } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { bytes, short, timecode } from "$lib/util/time";
  import { FONTS } from "$lib/util/text";
  import { keys } from "$lib/util/platform";
  import Button from "../ui/Button.svelte";
  import Scrub from "../ui/Scrub.svelte";
  import Segmented from "../ui/Segmented.svelte";
  import Slider from "../ui/Slider.svelte";
  import Toggle from "../ui/Toggle.svelte";
  import { animateFrame, bridge, extendClip, regenerate, restyleFrame } from "./actions";

  const clips = $derived(editor.selectedClips);
  const clip = $derived(clips.length === 1 ? clips[0] : null);
  const asset = $derived(clip ? editor.assetOf(clip) : editor.selectedAsset ? editor.assets.get(editor.selectedAsset) : undefined);
  const hasPicture = $derived(!!clip && clip.content.type === "media" && asset?.kind !== "audio");
  const hasSound = $derived(!!clip && ((asset?.kind === "audio") || (asset?.kind === "video" && asset.meta.has_audio)));

  function patch(p: ClipPatch, key?: string, final = true) {
    if (!clip) return;
    const edit = { op: "update_clip" as const, clip_id: clip.id, patch: p };
    if (key && !final) editor.live(edit, `${clip.id}:${key}`);
    else editor.edit(edit, key ? `${clip.id}:${key}` : undefined);
  }

  const tf = (k: keyof Transform, v: number | string, final: boolean) => clip && patch({ transform: { ...clip.transform, [k]: v } }, `tf-${k}`, final);
  const text = (s: Partial<TextStyle>, key = "text", final = true) => {
    if (clip?.content.type !== "text") return;
    const style = { ...clip.content.style, ...s };
    patch({ text: style, ...(s.content !== undefined ? { name: s.content.split("\n")[0].slice(0, 32) || "Text" } : {}) }, key, final);
  };

  function settings(s: Partial<ProjectSettings>) {
    if (!editor.project) return;
    editor.edit({ op: "set_settings", settings: { ...editor.project.settings, ...s } });
  }

  const presets = [
    { label: "1080p", w: 1920, h: 1080 },
    { label: "4K", w: 3840, h: 2160 },
    { label: "Vertical", w: 1080, h: 1920 },
    { label: "Square", w: 1080, h: 1080 },
    { label: "4:5", w: 1080, h: 1350 },
  ];

  const gap = $derived.by(() => {
    if (clips.length !== 2) return null;
    const [a, b] = [...clips].sort((x, y) => x.start - y.start);
    return { a, b };
  });

  function genOrigin(a: Asset | undefined) {
    return a?.origin.type === "generated" ? a.origin : null;
  }
</script>

<aside class="inspector">
  {#if clip}
    {@const g = genOrigin(asset)}
    <div class="title">
      <input class="name" value={clip.name} onchange={(e) => patch({ name: (e.currentTarget as HTMLInputElement).value })} onkeydown={(e) => e.stopPropagation()} />
      <span class="kind">{clip.content.type === "media" ? asset?.kind : clip.content.type}</span>
    </div>

    <div class="scroll">
      {#if g}
        <section class="prov">
          <div class="prov-head"><Sparkles size={12} /> <span>{g.model_name}</span></div>
          <p class="prompt">{g.prompt}</p>
          <div class="facts">
            <span>{gen.providerName(g.provider)}</span>
            {#if g.seed != null}<span class="mono">seed {g.seed}</span>{/if}
            <span>{(g.elapsed_ms / 1000).toFixed(0)}s</span>
            {#if g.cost_usd}<span>${g.cost_usd.toFixed(3)}</span>{/if}
          </div>
          <div class="row2">
            <Button size="sm" onclick={() => regenerate(clip)}><RefreshCw size={12} /> Regenerate</Button>
            <Button size="sm" onclick={() => regenerate(clip, true)}><Shuffle size={12} /> Variation</Button>
          </div>
        </section>
      {/if}

      {#if hasPicture}
        <section class="ai">
          <span class="eyebrow">With AI</span>
          <div class="ai-actions">
            <button onclick={() => animateFrame(clip)}><Clapperboard size={14} /><span>Animate this frame</span></button>
            {#if asset?.kind === "video" || asset?.kind === "image"}
              <button onclick={() => extendClip(clip)}><ArrowRightToLine size={14} /><span>Extend shot</span></button>
            {/if}
            <button onclick={() => restyleFrame(clip)}><Wand2 size={14} /><span>Restyle frame</span></button>
          </div>
        </section>
      {/if}

      {#if clip.content.type === "text"}
        {@const s = clip.content.style}
        <section>
          <span class="eyebrow">Text</span>
          <textarea rows="3" value={s.content} oninput={(e) => text({ content: (e.currentTarget as HTMLTextAreaElement).value }, "content", false)} onkeydown={(e) => e.stopPropagation()}></textarea>
          <div class="row">
            <select value={s.font_family} onchange={(e) => text({ font_family: (e.currentTarget as HTMLSelectElement).value })}>
              {#each FONTS as f (f)}<option value={f}>{f}</option>{/each}
            </select>
            <input type="color" value={s.color} oninput={(e) => text({ color: (e.currentTarget as HTMLInputElement).value }, "color", false)} />
          </div>
          <div class="grid2">
            <Scrub label="Size" value={s.font_size} min={8} max={800} onchange={(v, f) => text({ font_size: v }, "size", f)} />
            <Scrub label="Weight" value={s.font_weight} min={100} max={900} step={50} onchange={(v, f) => text({ font_weight: v }, "weight", f)} />
            <Scrub label="Track" value={s.letter_spacing} min={-20} max={60} decimals={1} step={0.5} onchange={(v, f) => text({ letter_spacing: v }, "ls", f)} />
            <Scrub label="Line" value={s.line_height} min={0.6} max={3} decimals={2} step={0.05} onchange={(v, f) => text({ line_height: v }, "lh", f)} />
          </div>
          <div class="row">
            <Segmented
              size="sm"
              value={s.align}
              onchange={(v) => text({ align: v })}
              options={[
                { value: "left", icon: AlignLeft, title: "Left" },
                { value: "center", icon: AlignCenter, title: "Center" },
                { value: "right", icon: AlignRight, title: "Right" },
              ]}
            />
          </div>
          <div class="toggles">
            <Toggle label="Italic" checked={s.italic} onchange={(v) => text({ italic: v })} />
            <Toggle label="Shadow" checked={s.shadow} onchange={(v) => text({ shadow: v })} />
            <Toggle label="Box" checked={!!s.background} onchange={(v) => text({ background: v ? "#000000cc" : null })} />
            {#if s.background}
              <input type="color" value={s.background.slice(0, 7)} oninput={(e) => text({ background: `${(e.currentTarget as HTMLInputElement).value}cc` }, "bg", false)} />
            {/if}
          </div>
        </section>
      {:else if clip.content.type === "solid"}
        <section>
          <span class="eyebrow">Color</span>
          <input type="color" class="wide" value={clip.content.color} oninput={(e) => patch({ color: (e.currentTarget as HTMLInputElement).value }, "color", false)} />
        </section>
      {/if}

      {#if clip.content.type !== "pending" && editor.trackOf(clip.id)?.kind === "video"}
        <section>
          <div class="sec-head">
            <span class="eyebrow">Transform</span>
            <button class="reset" data-tip="Reset" onclick={() => patch({ transform: { x: 0, y: 0, scale: 1, rotation: 0, opacity: 1, fit: clip.transform.fit } })}><RotateCcw size={12} /></button>
          </div>
          <div class="grid2">
            <Scrub label="X" value={clip.transform.x} onchange={(v, f) => tf("x", v, f)} />
            <Scrub label="Y" value={clip.transform.y} onchange={(v, f) => tf("y", v, f)} />
            <Scrub label="Scale" unit="%" value={clip.transform.scale * 100} min={1} max={1000} onchange={(v, f) => tf("scale", v / 100, f)} />
            <Scrub label="Rotate" unit="°" value={clip.transform.rotation} min={-360} max={360} onchange={(v, f) => tf("rotation", v, f)} />
          </div>
          <div class="labeled">
            <span>Opacity</span>
            <Slider value={clip.transform.opacity} onchange={(v, f) => tf("opacity", v, f)} />
            <span class="mono val">{Math.round(clip.transform.opacity * 100)}%</span>
          </div>
          {#if clip.content.type === "media"}
            <Segmented
              size="sm"
              value={clip.transform.fit}
              onchange={(v) => tf("fit", v, true)}
              options={[
                { value: "contain", label: "Fit" },
                { value: "cover", label: "Fill" },
                { value: "stretch", label: "Stretch" },
              ]}
            />
          {/if}
        </section>
      {/if}

      <section>
        <span class="eyebrow">Timing</span>
        <div class="facts big mono">
          <span>{timecode(clip.start, editor.fps)}</span>
          <span>→</span>
          <span>{timecode(clip.start + clip.duration, editor.fps)}</span>
          <span class="dim">{short(clip.duration)}</span>
        </div>
        <div class="grid2">
          {#if clip.content.type === "media" && asset?.kind !== "image"}
            <Scrub label="Speed" unit="×" value={clip.speed} min={0.1} max={16} step={0.05} decimals={2} onchange={(v, f) => patch({ speed: v }, "speed", f)} />
          {/if}
          <Scrub label="Fade in" unit="s" value={clip.fade_in} min={0} max={clip.duration} step={0.05} decimals={2} onchange={(v, f) => patch({ fade_in: v }, "fi", f)} />
          <Scrub label="Fade out" unit="s" value={clip.fade_out} min={0} max={clip.duration} step={0.05} decimals={2} onchange={(v, f) => patch({ fade_out: v }, "fo", f)} />
        </div>
      </section>

      {#if hasSound}
        <section>
          <span class="eyebrow">Sound</span>
          <div class="labeled">
            <span>Volume</span>
            <Slider value={clip.volume} max={2} onchange={(v, f) => patch({ volume: v }, "vol", f)} />
            <span class="mono val">{Math.round(clip.volume * 100)}%</span>
          </div>
        </section>
      {/if}
    </div>
  {:else if clips.length > 1}
    <div class="title"><span class="name static">{clips.length} clips</span></div>
    <div class="scroll">
      {#if gap}
        <section class="ai">
          <span class="eyebrow">With AI</span>
          <div class="ai-actions">
            <button onclick={() => bridge(gap.a, gap.b)}><Waypoints size={14} /><span>Bridge these two shots</span></button>
          </div>
          <p class="note">Generates a transition from the end of the first clip to the start of the second. Works best with models that take first + last frames.</p>
        </section>
      {/if}
      <section>
        <div class="row2">
          <Button size="sm" onclick={() => editor.duplicateSelection()}><Copy size={12} /> Duplicate</Button>
          <Button size="sm" variant="danger" onclick={() => editor.deleteSelection()}>Delete</Button>
        </div>
      </section>
    </div>
  {:else if asset}
    {@const g = genOrigin(asset)}
    <div class="title"><span class="name static">{asset.name}</span><span class="kind">{asset.kind}</span></div>
    <div class="scroll">
      <div class="poster">
        {#if asset.kind === "video"}
          <video src={fileUrl(asset.proxy ?? asset.path)} controls muted preload="metadata"></video>
        {:else if asset.kind === "image"}
          <img src={fileUrl(asset.path)} alt="" />
        {:else}
          <audio src={fileUrl(asset.path)} controls></audio>
        {/if}
      </div>
      {#if g}
        <section class="prov">
          <div class="prov-head"><Sparkles size={12} /> <span>{g.model_name}</span></div>
          <p class="prompt">{g.prompt}</p>
          <div class="facts">
            <span>{gen.providerName(g.provider)}</span>
            {#if g.seed != null}<span class="mono">seed {g.seed}</span>{/if}
            <span>{(g.elapsed_ms / 1000).toFixed(0)}s</span>
          </div>
        </section>
      {/if}
      <section>
        <div class="kv">
          {#if asset.meta.width}<span>Size</span><span class="mono">{asset.meta.width}×{asset.meta.height}</span>{/if}
          {#if asset.meta.duration && asset.kind !== "image"}<span>Length</span><span class="mono">{timecode(asset.meta.duration, asset.meta.fps ?? 30)}</span>{/if}
          {#if asset.meta.fps}<span>Frame rate</span><span class="mono">{asset.meta.fps.toFixed(2)}</span>{/if}
          {#if asset.meta.video_codec}<span>Video</span><span class="mono">{asset.meta.video_codec}</span>{/if}
          {#if asset.meta.audio_codec}<span>Audio</span><span class="mono">{asset.meta.audio_codec}</span>{/if}
          <span>File</span><span class="mono">{bytes(asset.meta.size_bytes)}</span>
        </div>
      </section>
      <section>
        <div class="row2">
          <Button size="sm" variant="primary" onclick={() => editor.insertAsset(asset.id)}><Plus size={12} /> Insert</Button>
          <Button size="sm" onclick={() => revealItemInDir(asset.path)}><FolderSearch size={12} /> Reveal</Button>
        </div>
      </section>
    </div>
  {:else if editor.project}
    {@const s = editor.project.settings}
    <div class="title"><span class="name static">Project</span></div>
    <div class="scroll">
      <section>
        <span class="eyebrow">Canvas</span>
        <div class="chips">
          {#each presets as p (p.label)}
            <button class="chip" class:on={s.width === p.w && s.height === p.h} onclick={() => settings({ width: p.w, height: p.h })}>{p.label}</button>
          {/each}
        </div>
        <div class="grid2">
          <Scrub label="W" value={s.width} min={16} max={7680} step={2} onchange={(v, f) => f && settings({ width: Math.round(v / 2) * 2 })} />
          <Scrub label="H" value={s.height} min={16} max={7680} step={2} onchange={(v, f) => f && settings({ height: Math.round(v / 2) * 2 })} />
        </div>
      </section>
      <section>
        <span class="eyebrow">Frame rate</span>
        <div class="chips">
          {#each [24, 25, 30, 50, 60] as fps (fps)}
            <button class="chip" class:on={s.fps === fps} onclick={() => settings({ fps })}>{fps}</button>
          {/each}
        </div>
      </section>
      <section>
        <span class="eyebrow">Background</span>
        <input type="color" class="wide" value={s.background} onchange={(e) => settings({ background: (e.currentTarget as HTMLInputElement).value })} />
      </section>
      <section class="keys">
        <span class="eyebrow">Shortcuts</span>
        <div class="kv">
          <span>Play / pause</span><kbd>{keys("Space")}</kbd>
          <span>Split</span><kbd>{keys("S")}</kbd>
          <span>Ripple delete</span><kbd>{keys("⇧⌫")}</kbd>
          <span>Generate</span><kbd>{keys("⌘G")}</kbd>
          <span>Command palette</span><kbd>{keys("⌘K")}</kbd>
          <span>Snapping</span><kbd>{keys("N")}</kbd>
          <span>Marker</span><kbd>{keys("M")}</kbd>
        </div>
      </section>
    </div>
  {/if}
</aside>

<style>
  .inspector {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    background: var(--surface);
  }
  .title {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px 10px 10px;
    border-bottom: 1px solid var(--line);
  }
  .name {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    outline: 0;
    font-weight: 600;
    font-size: 13px;
    padding: 4px;
    border-radius: var(--r-xs);
  }
  input.name:hover,
  input.name:focus {
    background: var(--surface-2);
  }
  .static {
    padding-left: 4px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind {
    font-size: 10.5px;
    color: var(--text-3);
    text-transform: capitalize;
    padding: 2px 7px;
    border-radius: 99px;
    background: var(--surface-3);
  }
  .scroll {
    flex: 1;
    overflow: auto;
    padding: 4px 0 20px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 9px;
    padding: 14px 14px;
    border-bottom: 1px solid var(--line);
  }
  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .reset {
    color: var(--text-3);
  }
  .reset:hover {
    color: var(--text);
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .row,
  .row2 {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .row :global(.seg) {
    width: 132px;
  }
  .row2 > :global(*) {
    flex: 1;
  }
  .labeled {
    display: grid;
    grid-template-columns: 58px 1fr 40px;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-2);
  }
  .val {
    text-align: right;
    font-size: 11px;
    color: var(--text-3);
  }
  textarea,
  select {
    width: 100%;
    background: var(--surface-2);
    border: 0;
    outline: 0;
    border-radius: var(--r-sm);
    box-shadow: inset 0 0 0 1px var(--line);
    padding: 7px 9px;
    font-size: 12.5px;
    resize: vertical;
  }
  textarea:focus,
  select:focus {
    box-shadow: inset 0 0 0 1px var(--accent-line);
  }
  select {
    flex: 1;
    height: 28px;
    padding: 0 8px;
  }
  input[type="color"] {
    -webkit-appearance: none;
    appearance: none;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: var(--r-sm);
    background: none;
    flex: none;
  }
  input[type="color"]::-webkit-color-swatch-wrapper {
    padding: 0;
  }
  input[type="color"]::-webkit-color-swatch {
    border: 0;
    border-radius: 7px;
    box-shadow: inset 0 0 0 1px var(--line-3);
  }
  input.wide {
    width: 100%;
  }
  .toggles {
    display: flex;
    flex-wrap: wrap;
    gap: 10px 14px;
    align-items: center;
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    font-size: 11px;
    color: var(--text-3);
  }
  .facts.big {
    font-size: 11.5px;
    color: var(--text-2);
  }
  .dim {
    color: var(--text-3);
  }
  .prov {
    margin: 10px 10px 0;
    border-radius: var(--r-md);
    background: var(--gen-soft), var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line-2);
    border-bottom: 0;
  }
  .prov-head {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--amber);
  }
  .prompt {
    margin: 0;
    font-family: var(--font-display);
    font-size: 16px;
    line-height: 1.3;
    color: var(--text);
    user-select: text;
  }
  .ai-actions {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .ai-actions button {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 32px;
    padding: 0 10px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
    font-size: 12.5px;
    text-align: left;
    transition: box-shadow 0.15s, background 0.15s;
  }
  .ai-actions button :global(svg) {
    color: var(--amber);
  }
  .ai-actions button:hover {
    background: var(--surface-3);
    box-shadow: inset 0 0 0 1px rgba(255, 138, 61, 0.4);
  }
  .note {
    margin: 0;
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.5;
  }
  .poster {
    margin: 10px 10px 0;
    border-radius: var(--r-md);
    overflow: hidden;
    background: #000;
  }
  .poster video,
  .poster img {
    width: 100%;
    display: block;
    max-height: 220px;
    object-fit: contain;
  }
  .poster audio {
    width: 100%;
  }
  .kv {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 12px;
    font-size: 12px;
    color: var(--text-3);
  }
  .kv > :nth-child(2n) {
    text-align: right;
    color: var(--text-2);
  }
  kbd {
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chip {
    height: 26px;
    padding: 0 9px;
    border-radius: 8px;
    font-size: 11.5px;
    color: var(--text-3);
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .chip.on {
    color: var(--text);
    background: var(--surface-4);
    box-shadow: inset 0 0 0 1px var(--line-3);
  }
  .keys {
    border-bottom: 0;
  }
</style>
