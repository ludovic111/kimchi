// The open project as the UI sees it. All mutations go through Rust (`edit`),
// which returns the new document; this module only holds view state
// (selection, playhead, zoom) and the playback clock.

import { api, message } from "../ipc";
import type { Asset } from "../bindings/Asset";
import type { Clip } from "../bindings/Clip";
import type { Edit } from "../bindings/Edit";
import type { EditOutcome } from "../bindings/EditOutcome";
import type { Project } from "../bindings/Project";
import type { ProjectView } from "../bindings/ProjectView";
import type { Track } from "../bindings/Track";
import { ui } from "./ui.svelte";

export const MIN_PPS = 4;
export const MAX_PPS = 600;

class EditorState {
  view = $state<ProjectView | null>(null);
  selection = $state<string[]>([]);
  selectedAsset = $state<string | null>(null);
  playhead = $state(0);
  playing = $state(false);
  /** Timeline zoom, pixels per second. */
  pps = $state(60);
  snapping = $state(true);
  ripple = $state(false);
  /** Loop the in/out range while playing. */
  loop = $state(false);

  project = $derived<Project | null>(this.view?.project ?? null);
  assets = $derived(new Map<string, Asset>((this.project?.assets ?? []).map((a) => [a.id, a])));
  duration = $derived(this.project ? Math.max(0, ...this.project.tracks.flatMap((t) => t.clips.map((c) => c.start + c.duration))) : 0);
  fps = $derived(this.project?.settings.fps ?? 30);
  selectedClips = $derived(this.selection.map((id) => this.clip(id)).filter((c): c is Clip => !!c));

  #raf = 0;
  #clockStart = 0;
  #playheadStart = 0;

  set(view: ProjectView | null) {
    this.view = view;
    if (!view) {
      this.selection = [];
      this.playhead = 0;
      this.pause();
      return;
    }
    // Drop selections that no longer exist (after undo, deletes…).
    const ids = new Set(view.project.tracks.flatMap((t) => t.clips.map((c) => c.id)));
    if (this.selection.some((id) => !ids.has(id))) this.selection = this.selection.filter((id) => ids.has(id));
    if (this.selectedAsset && !view.project.assets.some((a) => a.id === this.selectedAsset)) this.selectedAsset = null;
  }

  async edit(edit: Edit, coalesce?: string): Promise<EditOutcome | null> {
    try {
      const r = await api.applyEdit(edit, coalesce);
      this.set(r.view);
      return r.outcome;
    } catch (e) {
      ui.error(message(e));
      return null;
    }
  }

  async edits(edits: Edit[]): Promise<EditOutcome | null> {
    if (!edits.length) return null;
    try {
      const r = await api.applyEdits(edits);
      this.set(r.view);
      return r.outcome;
    } catch (e) {
      ui.error(message(e));
      return null;
    }
  }

  async undo() {
    if (!this.view?.can_undo) return;
    this.set(await api.undo());
  }

  async redo() {
    if (!this.view?.can_redo) return;
    this.set(await api.redo());
  }

  // ---- lookups --------------------------------------------------------

  clip(id: string): Clip | undefined {
    for (const t of this.project?.tracks ?? []) {
      const c = t.clips.find((c) => c.id === id);
      if (c) return c;
    }
  }

  trackOf(clipId: string): Track | undefined {
    return this.project?.tracks.find((t) => t.clips.some((c) => c.id === clipId));
  }

  assetOf(clip: Clip | undefined): Asset | undefined {
    return clip?.content.type === "media" ? this.assets.get(clip.content.asset_id) : undefined;
  }

  /** Top-most visual clip under the playhead, if any. */
  clipAt(t: number, kind: "video" | "audio" = "video"): Clip | undefined {
    for (const track of this.project?.tracks ?? []) {
      if (track.kind !== kind || track.hidden) continue;
      const c = track.clips.find((c) => t >= c.start && t < c.start + c.duration);
      if (c) return c;
    }
  }

  // ---- selection ------------------------------------------------------

  select(id: string, additive = false) {
    this.selectedAsset = null;
    if (additive) {
      this.selection = this.selection.includes(id) ? this.selection.filter((x) => x !== id) : [...this.selection, id];
    } else {
      this.selection = [id];
    }
  }

  clearSelection() {
    this.selection = [];
  }

  // ---- playback -------------------------------------------------------

  seek(t: number) {
    const snapped = Math.round(Math.max(0, t) * this.fps) / this.fps;
    this.playhead = snapped;
    if (this.playing) {
      this.#clockStart = performance.now();
      this.#playheadStart = snapped;
    }
  }

  step(frames: number) {
    this.pause();
    this.seek(this.playhead + frames / this.fps);
  }

  toggle() {
    this.playing ? this.pause() : this.play();
  }

  play() {
    if (this.playing) return;
    if (this.playhead >= this.duration - 1 / this.fps) this.playhead = 0;
    this.playing = true;
    this.#clockStart = performance.now();
    this.#playheadStart = this.playhead;
    const tick = (now: number) => {
      if (!this.playing) return;
      const t = this.#playheadStart + (now - this.#clockStart) / 1000;
      if (t >= this.duration) {
        if (this.loop && this.duration > 0) {
          this.#clockStart = now;
          this.#playheadStart = 0;
          this.playhead = 0;
        } else {
          this.playhead = this.duration;
          this.pause();
          return;
        }
      } else {
        this.playhead = t;
      }
      this.#raf = requestAnimationFrame(tick);
    };
    this.#raf = requestAnimationFrame(tick);
  }

  pause() {
    this.playing = false;
    cancelAnimationFrame(this.#raf);
  }

  // ---- common edits ---------------------------------------------------

  async splitAtPlayhead() {
    const ids = this.selection.length ? this.selection : null;
    const out = await this.edit({ op: "split", time: this.playhead, clip_ids: ids });
    if (out?.created_clips.length && ids) this.selection = [...ids, ...out.created_clips];
  }

  async deleteSelection(ripple = this.ripple) {
    if (this.selection.length) {
      await this.edit({ op: "delete_clips", clip_ids: this.selection, ripple });
      this.selection = [];
    } else if (this.selectedAsset) {
      await this.edit({ op: "remove_asset", asset_id: this.selectedAsset });
      this.selectedAsset = null;
    }
  }

  async duplicateSelection() {
    if (!this.selection.length) return;
    const out = await this.edit({ op: "duplicate_clips", clip_ids: this.selection });
    if (out?.created_clips.length) this.selection = out.created_clips;
  }

  async insertAsset(assetId: string, trackId: string | null = null, start = this.playhead) {
    const out = await this.edit({ op: "insert_asset", asset_id: assetId, track_id: trackId, start });
    if (out?.created_clips.length) this.selection = out.created_clips;
  }
}

export const editor = new EditorState();
