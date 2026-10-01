// Generation: providers, models, running jobs and the prompt composer.

import { api, message } from "../ipc";
import type { GenRequest } from "../bindings/GenRequest";
import type { ImageRole } from "../bindings/ImageRole";
import type { Job } from "../bindings/Job";
import type { ModelInfo } from "../bindings/ModelInfo";
import type { ProviderStatus } from "../bindings/ProviderStatus";
import type { Task } from "../bindings/Task";
import { editor } from "./editor.svelte";
import { ui } from "./ui.svelte";

export type Mode = "image" | "video";

export interface Ref {
  role: ImageRole;
  path: string;
  /** Library asset this came from, for provenance. */
  assetId: string | null;
  label: string;
}

export interface Target {
  trackId: string | null;
  start: number;
  duration: number;
  label: string;
}

export interface Draft {
  mode: Mode;
  prompt: string;
  negative: string;
  model: string | null;
  aspect: string | null;
  duration: number | null;
  resolution: string | null;
  count: number;
  seed: string;
  audio: boolean;
  params: Record<string, unknown>;
  refs: Ref[];
  toTimeline: boolean;
  target: Target | null;
}

export const modelKey = (m: ModelInfo) => `${m.provider}::${m.id}`;

const stored = (k: string) => {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
};
const store = (k: string, v: string) => {
  try {
    localStorage.setItem(k, v);
  } catch {}
};

function freshDraft(mode: Mode = "image"): Draft {
  return {
    mode,
    prompt: "",
    negative: "",
    model: stored(`kimchi.model.${mode}`),
    aspect: null,
    duration: null,
    resolution: null,
    count: 1,
    seed: "",
    audio: true,
    params: {},
    refs: [],
    toTimeline: true,
    target: null,
  };
}

class GenState {
  providers = $state<ProviderStatus[]>([]);
  models = $state<ModelInfo[]>([]);
  loadingModels = $state(false);
  jobs = $state<Job[]>([]);
  draft = $state<Draft>(freshDraft());

  ready = $derived(this.providers.filter((p) => p.ready));
  active = $derived(this.jobs.filter((j) => j.status === "queued" || j.status === "running"));
  task = $derived<Task>(taskFor(this.draft));
  available = $derived(this.models.filter((m) => m.tasks.includes(this.task)));
  model = $derived(this.available.find((m) => modelKey(m) === this.draft.model) ?? this.available.find((m) => m.featured) ?? this.available[0] ?? null);

  async load() {
    try {
      const [providers, jobs] = await Promise.all([api.genProviders(), api.genJobs()]);
      this.providers = providers;
      this.jobs = jobs;
      await this.loadModels();
    } catch (e) {
      ui.error(message(e));
    }
  }

  async loadModels(refresh = false) {
    this.loadingModels = true;
    try {
      if (refresh) {
        const lists = await Promise.allSettled(this.ready.map((p) => api.genModels(p.info.id, true)));
        this.models = lists.flatMap((r) => (r.status === "fulfilled" ? r.value : []));
      } else {
        this.models = await api.genModels(null);
      }
    } finally {
      this.loadingModels = false;
    }
  }

  setProviders(p: ProviderStatus[]) {
    this.providers = p;
    void this.loadModels();
  }

  upsertJob(job: Job) {
    const i = this.jobs.findIndex((j) => j.id === job.id);
    if (i === -1) this.jobs = [job, ...this.jobs];
    else this.jobs[i] = job;
    if (job.status === "failed" && job.error) ui.error(job.error);
  }

  providerName(id: string) {
    return this.providers.find((p) => p.info.id === id)?.info.name ?? id;
  }

  setMode(mode: Mode) {
    if (this.draft.mode === mode) return;
    this.draft.mode = mode;
    this.draft.model = stored(`kimchi.model.${mode}`);
    this.draft.duration = null;
    this.draft.resolution = null;
    this.draft.params = {};
    // Video only uses start/end frames; images only references.
    this.draft.refs = this.draft.refs
      .map((r) => ({ ...r, role: (mode === "video" ? (r.role === "reference" ? "start_frame" : r.role) : "reference") as ImageRole }))
      .slice(0, mode === "video" ? 2 : 8);
  }

  pickModel(m: ModelInfo) {
    this.draft.model = modelKey(m);
    this.draft.params = Object.fromEntries(m.params.map((p) => [p.key, p.default]));
    store(`kimchi.model.${this.draft.mode}`, modelKey(m));
  }

  addRef(ref: Ref) {
    const refs = this.draft.refs.filter((r) => !(ref.role !== "reference" && r.role === ref.role));
    this.draft.refs = [...refs, ref];
  }

  removeRef(i: number) {
    this.draft.refs = this.draft.refs.filter((_, j) => j !== i);
  }

  /** Opens the Generate panel pre-filled, e.g. from a timeline action. */
  compose(patch: Partial<Draft> & { mode?: Mode }) {
    if (patch.mode) this.setMode(patch.mode);
    Object.assign(this.draft, patch);
    ui.leftTab = "generate";
    queueMicrotask(() => document.querySelector<HTMLTextAreaElement>("#prompt")?.focus());
  }

  request(): GenRequest | null {
    const m = this.model;
    const p = editor.project;
    if (!m || !p) return null;
    const d = this.draft;
    const seed = d.seed.trim() === "" ? null : Number.parseInt(d.seed, 10);
    return {
      model: m.id,
      task: this.task,
      prompt: d.prompt.trim(),
      negative_prompt: m.negative_prompt && d.negative.trim() ? d.negative.trim() : null,
      images: d.refs.slice(0, Math.max(m.max_images, m.end_frame ? 2 : 1)).map((r) => ({ role: r.role, path: r.path, mime: "" })),
      aspect_ratio: d.aspect ?? aspectOf(p.settings.width, p.settings.height),
      width: p.settings.width,
      height: p.settings.height,
      duration: d.mode === "video" ? (d.duration ?? m.durations[0] ?? 5) : null,
      resolution: d.resolution ?? m.resolutions[0] ?? null,
      seed: Number.isFinite(seed) ? seed : null,
      count: Math.min(d.count, Math.max(1, m.max_outputs)),
      audio: m.audio ? d.audio : null,
      params: d.params,
    };
  }

  async submit() {
    const m = this.model;
    const req = this.request();
    if (!m || !req) {
      ui.openSettings();
      return;
    }
    const d = this.draft;
    const length = d.target?.duration ?? (d.mode === "video" ? (req.duration ?? 5) : 5);
    try {
      const job = await api.genSubmit({
        provider: m.provider,
        request: req,
        placement: d.toTimeline
          ? { type: "timeline", track_id: d.target?.trackId ?? null, start: d.target?.start ?? editor.playhead, duration: length }
          : { type: "library" },
        input_assets: d.refs.map((r) => r.assetId).filter((x): x is string => !!x),
      });
      this.upsertJob(job);
      d.target = null;
    } catch (e) {
      ui.error(message(e));
    }
  }

  async cancel(id: string) {
    await api.genCancel(id);
  }

  async clearFinished() {
    this.jobs = await api.genClear();
  }
}

export function taskFor(d: Draft): Task {
  const hasImage = d.refs.length > 0;
  if (d.mode === "video") return hasImage ? "image_to_video" : "text_to_video";
  return hasImage ? "image_to_image" : "text_to_image";
}

export function aspectOf(w: number, h: number): string {
  const gcd = (a: number, b: number): number => (b ? gcd(b, a % b) : a);
  const g = gcd(w, h) || 1;
  return `${w / g}:${h / g}`;
}

export const gen = new GenState();
