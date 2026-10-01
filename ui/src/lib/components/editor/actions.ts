// Editor actions shared by menus, the command palette and shortcuts —
// including the AI actions that tie generation into the timeline.
import { open } from "@tauri-apps/plugin-dialog";
import { api, message } from "$lib/ipc";
import type { Clip } from "$lib/bindings/Clip";
import type { TextStyle } from "$lib/bindings/TextStyle";
import { editor } from "$lib/state/editor.svelte";
import { gen } from "$lib/state/gen.svelte";
import { ui } from "$lib/state/ui.svelte";

export const MEDIA_EXTENSIONS = ["mp4", "mov", "m4v", "webm", "mkv", "avi", "png", "jpg", "jpeg", "webp", "gif", "heic", "avif", "mp3", "wav", "m4a", "aac", "flac", "ogg", "opus"];

export async function importDialog() {
  const picked = await open({ multiple: true, filters: [{ name: "Media", extensions: MEDIA_EXTENSIONS }] });
  const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
  if (!paths.length) return;
  try {
    const assets = await api.importMedia(paths);
    ui.toast(`Imported ${assets.length} file${assets.length > 1 ? "s" : ""}`, "success");
    ui.leftTab = "media";
  } catch (e) {
    ui.error(message(e));
  }
}

export const defaultText = (): TextStyle => ({
  content: "Your title",
  font_family: "Instrument Sans",
  font_size: 120,
  font_weight: 650,
  italic: false,
  color: "#ffffff",
  background: null,
  align: "center",
  line_height: 1.1,
  letter_spacing: -1,
  shadow: true,
});

export async function addText(style: Partial<TextStyle> = {}, y = 0) {
  const s = { ...defaultText(), ...style };
  const clip: Clip = {
    id: crypto.randomUUID(),
    name: s.content.split("\n")[0].slice(0, 32) || "Text",
    start: editor.playhead,
    duration: 4,
    in_point: 0,
    speed: 1,
    content: { type: "text", style: s },
    transform: { x: 0, y, scale: 1, rotation: 0, opacity: 1, fit: "contain" },
    volume: 1,
    fade_in: 0.2,
    fade_out: 0.2,
  };
  const out = await editor.edit({ op: "add_clip", track_id: null, clip });
  if (out?.created_clips.length) editor.selection = out.created_clips;
}

async function frame(clip: Clip, time: number) {
  try {
    return await api.clipFrame(clip.id, time);
  } catch (e) {
    ui.error(message(e));
    return null;
  }
}

const frameLabel = (clip: Clip, t: number) => `${clip.name} @ ${t.toFixed(2)}s`;

/** Turn the frame under the playhead into a moving shot. */
export async function animateFrame(clip: Clip, time = editor.playhead) {
  const path = await frame(clip, time);
  if (!path) return;
  gen.compose({
    mode: "video",
    refs: [{ role: "start_frame", path, assetId: clip.content.type === "media" ? clip.content.asset_id : null, label: frameLabel(clip, time) }],
    toTimeline: true,
    target: { trackId: null, start: clip.start + clip.duration, duration: 5, label: "after the clip" },
  });
}

/** Continue a clip from its last frame, landing right after it on the same track. */
export async function extendClip(clip: Clip) {
  const t = clip.start + clip.duration - 1 / editor.fps;
  const path = await frame(clip, t);
  if (!path) return;
  const track = editor.trackOf(clip.id);
  gen.compose({
    mode: "video",
    refs: [{ role: "start_frame", path, assetId: clip.content.type === "media" ? clip.content.asset_id : null, label: `Last frame of ${clip.name}` }],
    toTimeline: true,
    target: { trackId: track?.id ?? null, start: clip.start + clip.duration, duration: 5, label: `after “${clip.name}”` },
  });
}

/** Restyle / edit the frame as a still. */
export async function restyleFrame(clip: Clip, time = editor.playhead) {
  const path = await frame(clip, time);
  if (!path) return;
  gen.compose({
    mode: "image",
    refs: [{ role: "reference", path, assetId: clip.content.type === "media" ? clip.content.asset_id : null, label: frameLabel(clip, time) }],
    toTimeline: true,
    target: { trackId: null, start: time, duration: 3, label: "at the playhead" },
  });
}

/** Generate a transition shot between two clips: A's last frame → B's first. */
export async function bridge(a: Clip, b: Clip) {
  const [first, last] = a.start < b.start ? [a, b] : [b, a];
  const [start, end] = await Promise.all([frame(first, first.start + first.duration - 1 / editor.fps), frame(last, last.start)]);
  if (!start || !end) return;
  const gap = Math.max(0, last.start - (first.start + first.duration));
  gen.compose({
    mode: "video",
    refs: [
      { role: "start_frame", path: start, assetId: null, label: `End of ${first.name}` },
      { role: "end_frame", path: end, assetId: null, label: `Start of ${last.name}` },
    ],
    toTimeline: true,
    target: { trackId: editor.trackOf(first.id)?.id ?? null, start: first.start + first.duration, duration: gap || 4, label: "between the clips" },
  });
}

/** Re-open the composer with a generated clip's original settings. */
export function regenerate(clip: Clip, variation = false) {
  const asset = editor.assetOf(clip);
  if (asset?.origin.type !== "generated") return;
  const g = asset.origin;
  const req = g.params as Record<string, any>;
  const video = g.task.includes("video");
  gen.compose({
    mode: video ? "video" : "image",
    prompt: g.prompt,
    negative: g.negative_prompt ?? "",
    model: `${g.provider}::${g.model}`,
    seed: variation ? "" : g.seed != null ? String(g.seed) : "",
    duration: typeof req.duration === "number" ? req.duration : null,
    aspect: typeof req.aspect_ratio === "string" ? req.aspect_ratio : null,
    toTimeline: true,
    target: { trackId: editor.trackOf(clip.id)?.id ?? null, start: clip.start + clip.duration, duration: clip.duration, label: `after “${clip.name}”` },
  });
}
