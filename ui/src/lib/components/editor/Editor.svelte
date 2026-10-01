<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { api, isTauri, message } from "$lib/ipc";
  import { editor, MAX_PPS, MIN_PPS } from "$lib/state/editor.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import TopBar from "./TopBar.svelte";
  import LeftPanel from "./LeftPanel.svelte";
  import Preview from "./Preview.svelte";
  import Inspector from "./Inspector.svelte";
  import Timeline from "../timeline/Timeline.svelte";
  import Splitter from "../ui/Splitter.svelte";
  import ExportDialog from "../dialogs/ExportDialog.svelte";
  import CommandPalette from "../dialogs/CommandPalette.svelte";
  import { addText, importDialog } from "./actions";

  let dropping = $state(false);

  onMount(() => {
    if (!isTauri) return;
    // Files dragged in from Finder/Explorer.
    const off = getCurrentWebview().onDragDropEvent(async (e) => {
      if (e.payload.type === "over" || e.payload.type === "enter") dropping = true;
      else if (e.payload.type === "leave") dropping = false;
      else if (e.payload.type === "drop") {
        dropping = false;
        if (!e.payload.paths.length) return;
        try {
          const assets = await api.importMedia(e.payload.paths);
          ui.toast(`Imported ${assets.length} file${assets.length > 1 ? "s" : ""}`, "success");
          ui.leftTab = "media";
        } catch (err) {
          ui.error(message(err));
        }
      }
    });
    return () => void off.then((f) => f());
  });

  function onKey(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    if (target.closest("input, textarea, [contenteditable]")) return;
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key.toLowerCase();
    const handled = () => e.preventDefault();

    if (mod && k === "z") return handled(), e.shiftKey ? editor.redo() : editor.undo();
    if (mod && k === "y") return handled(), editor.redo();
    if (mod && k === "k") return handled(), (ui.paletteOpen = true);
    if (mod && k === "e") return handled(), (ui.exportOpen = true);
    if (mod && k === "i") return handled(), importDialog();
    if (mod && k === "d") return handled(), editor.duplicateSelection();
    if (mod && k === "b") return handled(), editor.splitAtPlayhead();
    if (mod && k === "g") return handled(), (ui.leftTab = "generate"), document.querySelector<HTMLTextAreaElement>("#prompt")?.focus();
    if (mod && k === "a") {
      handled();
      editor.selection = editor.project?.tracks.flatMap((t) => t.clips.map((c) => c.id)) ?? [];
      return;
    }
    if (mod) return;

    switch (e.key) {
      case " ":
        handled();
        editor.toggle();
        break;
      case "ArrowLeft":
        handled();
        editor.step(e.shiftKey ? -editor.fps : -1);
        break;
      case "ArrowRight":
        handled();
        editor.step(e.shiftKey ? editor.fps : 1);
        break;
      case "Home":
        handled();
        editor.seek(0);
        break;
      case "End":
        handled();
        editor.seek(editor.duration);
        break;
      case "Backspace":
      case "Delete":
        handled();
        editor.deleteSelection(e.shiftKey || editor.ripple);
        break;
      case "Escape":
        editor.clearSelection();
        break;
      case "s":
        editor.splitAtPlayhead();
        break;
      case "n":
        editor.snapping = !editor.snapping;
        ui.toast(editor.snapping ? "Snapping on" : "Snapping off", "info", 1400);
        break;
      case "=":
      case "+":
        editor.pps = Math.min(MAX_PPS, editor.pps * 1.25);
        break;
      case "-":
        editor.pps = Math.max(MIN_PPS, editor.pps / 1.25);
        break;
      case "m":
        editor.edit({ op: "add_marker", time: editor.playhead, label: "" });
        break;
      case "t":
        addText();
        break;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="editor" style="--left:{ui.leftWidth}px;--right:{ui.rightWidth}px;--tl:{ui.timelineHeight}px">
  <TopBar />
  <div class="workspace">
    <LeftPanel />
    <Splitter axis="x" value={ui.leftWidth} min={280} max={520} onchange={(v) => (ui.leftWidth = v)} />
    <Preview />
    <Splitter axis="x" value={ui.rightWidth} min={260} max={440} invert onchange={(v) => (ui.rightWidth = v)} />
    <Inspector />
  </div>
  <Splitter axis="y" value={ui.timelineHeight} min={180} max={620} invert onchange={(v) => (ui.timelineHeight = v)} />
  <Timeline />

  {#if dropping}
    <div class="drop">
      <div>Drop to import</div>
    </div>
  {/if}
</div>

<ExportDialog />
<CommandPalette />

<style>
  .editor {
    height: 100%;
    display: grid;
    grid-template-rows: var(--topbar-h) minmax(0, 1fr) auto var(--tl);
    overflow: hidden;
    background: var(--bg);
  }
  .workspace {
    min-height: 0;
    display: grid;
    grid-template-columns: var(--left) auto minmax(0, 1fr) auto var(--right);
    overflow: hidden;
  }
  .drop {
    position: fixed;
    inset: 8px;
    z-index: 90;
    border-radius: var(--r-xl);
    border: 1.5px dashed var(--accent-line);
    background: rgba(255, 90, 54, 0.06);
    display: grid;
    place-items: center;
    pointer-events: none;
    animation: fade 0.15s;
  }
  .drop div {
    padding: 10px 16px;
    border-radius: 99px;
    background: var(--surface-3);
    box-shadow: var(--shadow-pop);
    font-weight: 600;
  }
</style>
