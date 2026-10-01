<script lang="ts">
  import type { TextStyle } from "$lib/bindings/TextStyle";
  import { editor } from "$lib/state/editor.svelte";
  import { addText } from "./actions";

  const presets: { name: string; style: Partial<TextStyle>; y: (h: number) => number }[] = [
    { name: "Title", style: { content: "Your title", font_size: 140, font_weight: 700, letter_spacing: -3 }, y: () => 0 },
    {
      name: "Editorial",
      style: { content: "A quiet morning", font_family: "Instrument Serif", font_size: 150, font_weight: 400, italic: true, letter_spacing: -2 },
      y: () => 0,
    },
    {
      name: "Lower third",
      style: { content: "Name Surname\nRole, Company", font_size: 52, font_weight: 600, align: "left", line_height: 1.25, letter_spacing: 0 },
      y: (h) => h * 0.32,
    },
    {
      name: "Caption",
      style: { content: "and that's when it clicked", font_size: 60, font_weight: 650, background: "#000000cc", shadow: false, letter_spacing: 0 },
      y: (h) => h * 0.36,
    },
    {
      name: "Label",
      style: { content: "CHAPTER 01", font_family: "Geist Mono", font_size: 44, font_weight: 500, letter_spacing: 6, shadow: false, color: "#ff7a52" },
      y: (h) => -h * 0.3,
    },
    {
      name: "Shout",
      style: { content: "WAIT FOR IT", font_size: 180, font_weight: 800, letter_spacing: -4, color: "#f0b44c" },
      y: () => 0,
    },
  ];

  const css = (s: Partial<TextStyle>) =>
    `font-family:"${s.font_family ?? "Instrument Sans"}";font-weight:${s.font_weight ?? 600};font-style:${s.italic ? "italic" : "normal"};letter-spacing:${(s.letter_spacing ?? 0) / 6}px;color:${s.color ?? "#fff"};${s.background ? `background:${s.background};padding:2px 6px;border-radius:4px;` : ""}`;
</script>

<div class="head"><h3>Text</h3></div>
<div class="scroll">
  <div class="grid">
    {#each presets as p (p.name)}
      <button class="tile" onclick={() => addText(p.style, p.y(editor.project?.settings.height ?? 1080))}>
        <div class="sample" style={css(p.style)}>{p.style.content?.split("\n")[0]}</div>
        <span>{p.name}</span>
      </button>
    {/each}
  </div>
  <p class="hint">Click to drop at the playhead. Edit the words and style in the inspector.</p>
</div>

<style>
  .head {
    padding: 14px 14px 10px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }
  .scroll {
    flex: 1;
    overflow: auto;
    padding: 0 10px 14px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 6px;
    text-align: left;
  }
  .sample {
    height: 74px;
    border-radius: var(--r-md);
    background:
      radial-gradient(120% 120% at 20% 0%, #2a2421, #161312);
    box-shadow: inset 0 0 0 1px var(--line);
    display: grid;
    place-items: center;
    font-size: 17px;
    white-space: nowrap;
    overflow: hidden;
    padding: 0 8px;
    transition: box-shadow 0.15s;
  }
  .tile:hover .sample {
    box-shadow: inset 0 0 0 1px var(--line-3);
  }
  span {
    font-size: 11.5px;
    color: var(--text-2);
    padding: 0 2px;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
    margin: 14px 4px 0;
  }
</style>
