// Browser-only demo data so the UI can be developed and reviewed without the
// desktop shell (`npm run ui:dev`). Media comes from ui/demo (see scripts/demo-media.sh).
import type { Asset } from "./bindings/Asset";
import type { Job } from "./bindings/Job";
import type { ModelInfo } from "./bindings/ModelInfo";
import type { Project } from "./bindings/Project";
import type { ProjectSummary } from "./bindings/ProjectSummary";
import type { ProjectView } from "./bindings/ProjectView";
import type { ProviderStatus } from "./bindings/ProviderStatus";
import type { Task } from "./bindings/Task";

const now = new Date().toISOString();
const T = { x: 0, y: 0, scale: 1, rotation: 0, opacity: 1, fit: "contain" as const };

function video(id: string, name: string, file: string, duration: number, generated?: string): Asset {
  return {
    id,
    name,
    kind: "video",
    path: `/${file}.mp4`,
    meta: { duration, width: 960, height: 540, fps: 30, has_video: true, has_audio: false, video_codec: "h264", audio_codec: null, size_bytes: 1_400_000 },
    origin: generated
      ? { type: "generated", job_id: "j", provider: "fal", model: "veo3.1", model_name: "Veo 3.1", task: "text_to_video", prompt: generated, negative_prompt: null, seed: 41213, params: {}, inputs: [], elapsed_ms: 74000, cost_usd: 0.8 }
      : { type: "imported" },
    created_at: now,
    thumbnail: `/${file}-thumb.jpg`,
    filmstrip: { path: `/${file}-strip.jpg`, frames: 12, frame_width: 128, frame_height: 72, interval: 0.5 },
    waveform: null,
    proxy: null,
  };
}

const assets: Asset[] = [
  video("a1", "dusk.mp4", "dusk", 6),
  video("a2", "Neon market at night", "fractal", 6, "A slow dolly through a neon-lit night market, rain on the lens, shallow depth of field"),
  video("a3", "cells.mp4", "cells", 6),
  {
    id: "a4",
    name: "Ember backdrop",
    kind: "image",
    path: "/ember.jpg",
    meta: { duration: null, width: 1280, height: 720, fps: null, has_video: true, has_audio: false, video_codec: "mjpeg", audio_codec: null, size_bytes: 18000 },
    origin: { type: "generated", job_id: "j2", provider: "openrouter", model: "google/gemini-3-pro-image", model_name: "Nano Banana Pro", task: "text_to_image", prompt: "Warm abstract gradient, ember and cream, film grain", negative_prompt: null, seed: 7, params: {}, inputs: [], elapsed_ms: 9000, cost_usd: 0.04 },
    created_at: now,
    thumbnail: "/ember.jpg",
    filmstrip: null,
    waveform: null,
    proxy: null,
  },
  {
    id: "a5",
    name: "meadow.jpg",
    kind: "image",
    path: "/meadow.jpg",
    meta: { duration: null, width: 1280, height: 720, fps: null, has_video: true, has_audio: false, video_codec: "mjpeg", audio_codec: null, size_bytes: 31000 },
    origin: { type: "imported" },
    created_at: now,
    thumbnail: "/meadow.jpg",
    filmstrip: null,
    waveform: null,
    proxy: null,
  },
  {
    id: "a6",
    name: "bed.m4a",
    kind: "audio",
    path: "/bed.m4a",
    meta: { duration: 12, width: null, height: null, fps: null, has_video: false, has_audio: true, video_codec: null, audio_codec: "aac", size_bytes: 106000 },
    origin: { type: "imported" },
    created_at: now,
    thumbnail: null,
    filmstrip: null,
    waveform: { path: "demo-peaks", peaks_per_second: 100 },
    proxy: null,
  },
];

const clip = (id: string, name: string, start: number, duration: number, content: any, extra: object = {}) => ({
  id,
  name,
  start,
  duration,
  in_point: 0,
  speed: 1,
  content,
  transform: { ...T },
  volume: 1,
  fade_in: 0,
  fade_out: 0,
  ...extra,
});

export const demoProject: Project = {
  id: "demo",
  name: "Night Market",
  created_at: now,
  updated_at: now,
  settings: { width: 1920, height: 1080, fps: 30, background: "#000000", sample_rate: 48000 },
  assets,
  tracks: [
    {
      id: "t0",
      kind: "video",
      name: "Video 2",
      muted: false,
      hidden: false,
      locked: false,
      clips: [
        clip("c5", "NIGHT MARKET", 0.6, 3.6, { type: "text", style: { content: "Night Market", font_family: "Instrument Serif", font_size: 150, font_weight: 400, italic: true, color: "#ffffff", background: null, align: "center", line_height: 1.1, letter_spacing: -2, shadow: true } }, { fade_in: 0.4, fade_out: 0.4 }),
        clip("c6", "Rain, steam and neon", 13, 5, { type: "pending", job_id: "job-run", kind: "video", prompt: "Rain, steam and neon reflections on wet asphalt, slow push in", model_name: "Kling 3 Pro" }),
      ],
    },
    {
      id: "t1",
      kind: "video",
      name: "Video 1",
      muted: false,
      hidden: false,
      locked: false,
      clips: [
        clip("c1", "dusk.mp4", 0, 6, { type: "media", asset_id: "a1" }, { fade_in: 0.6 }),
        clip("c2", "Neon market at night", 6, 6, { type: "media", asset_id: "a2" }),
        clip("c3", "Ember backdrop", 12, 4, { type: "media", asset_id: "a4" }),
        clip("c4", "cells.mp4", 16, 6, { type: "media", asset_id: "a3" }, { speed: 1.5, duration: 4 }),
      ],
    },
    {
      id: "t2",
      kind: "audio",
      name: "Audio 1",
      muted: false,
      hidden: false,
      locked: false,
      clips: [clip("c7", "bed.m4a", 0, 12, { type: "media", asset_id: "a6" }, { fade_in: 1, fade_out: 2 }), clip("c8", "bed.m4a", 12, 8, { type: "media", asset_id: "a6" }, { in_point: 2, fade_out: 2 })],
    },
  ],
  markers: [{ id: "m1", time: 12, label: "", color: "#ff5a36" }],
};


export const demoSummaries: ProjectSummary[] = [
  { id: "demo", name: "Night Market", updated_at: now, duration: 22, width: 1920, height: 1080, cover: "/fractal-thumb.jpg", generated_count: 2 },
  { id: "p2", name: "Product teaser — v3", updated_at: new Date(Date.now() - 3600e3 * 5).toISOString(), duration: 31, width: 1080, height: 1920, cover: "/dusk-thumb.jpg", generated_count: 6 },
  { id: "p3", name: "Kimchi recipe short", updated_at: new Date(Date.now() - 86400e3 * 2).toISOString(), duration: 58, width: 1080, height: 1350, cover: "/meadow.jpg", generated_count: 0 },
  { id: "p4", name: "Moodboard loops", updated_at: new Date(Date.now() - 86400e3 * 9).toISOString(), duration: 12, width: 1080, height: 1080, cover: "/cells-thumb.jpg", generated_count: 11 },
];

const provider = (id: string, name: string, kind: "cloud" | "local", ready: boolean, tagline: string, tasks: Task[]): ProviderStatus => ({
  info: { id, name, kind, tagline, website: `https://${id}.example`, needs_key: kind === "cloud", key_env: [`${id.toUpperCase()}_API_KEY`], key_url: "https://example.com", key_hint: "sk-…", default_base_url: "", base_url_editable: kind === "local", tasks },
  settings: { enabled: true, base_url: null, options: {} },
  key_source: ready && kind === "cloud" ? "keychain" : "none",
  key_preview: ready && kind === "cloud" ? "…9f3a" : null,
  ready,
});

const all: Task[] = ["text_to_image", "image_to_image", "text_to_video", "image_to_video"];
export const demoProviders: ProviderStatus[] = [
  provider("openrouter", "OpenRouter", "cloud", true, "One key for image and video models from every lab", all),
  provider("fal", "fal", "cloud", true, "Hundreds of image and video models, fast queues", all),
  provider("replicate", "Replicate", "cloud", false, "Run open and proprietary models by the second", all),
  provider("openai", "OpenAI", "cloud", false, "GPT Image", ["text_to_image", "image_to_image"]),
  provider("google", "Google Gemini", "cloud", false, "Gemini image, Omni and Veo video", all),
  provider("comfyui", "ComfyUI", "local", true, "Your own nodes and checkpoints, on your GPU", all),
];

const model = (provider: string, id: string, name: string, tasks: Task[], extra: Partial<ModelInfo> = {}): ModelInfo => ({
  id,
  name,
  provider,
  tasks,
  description: null,
  aspect_ratios: ["16:9", "9:16", "1:1", "4:3", "3:4"],
  durations: [],
  resolutions: [],
  max_outputs: 1,
  negative_prompt: false,
  seed: true,
  end_frame: false,
  max_images: 1,
  audio: false,
  params: [],
  price: null,
  featured: false,
  ...extra,
});

export const demoModels: ModelInfo[] = [
  model("fal", "fal-ai/veo3.1", "Veo 3.1", ["text_to_video", "image_to_video"], { durations: [4, 6, 8], resolutions: ["720p", "1080p"], audio: true, end_frame: true, featured: true, price: "$0.40/s" }),
  model("fal", "fal-ai/kling-video/v3/pro", "Kling 3 Pro", ["text_to_video", "image_to_video"], { durations: [5, 10], end_frame: true, featured: true, price: "$0.11/s", negative_prompt: true }),
  model("openrouter", "bytedance/seedance-2.0", "Seedance 2.0", ["text_to_video", "image_to_video"], { durations: [5, 10], resolutions: ["480p", "720p", "1080p"], featured: true }),
  model("openrouter", "google/gemini-3-pro-image", "Nano Banana Pro", ["text_to_image", "image_to_image"], { max_images: 14, featured: true, price: "$0.13/img", resolutions: ["1K", "2K", "4K"] }),
  model("fal", "fal-ai/flux-2-pro", "FLUX.2 pro", ["text_to_image", "image_to_image"], { max_images: 8, featured: true, max_outputs: 4, price: "$0.03/MP" }),
  model("comfyui", "ckpt:sdxl_base_1.0.safetensors", "sdxl_base_1.0", ["text_to_image", "image_to_image"], {
    negative_prompt: true,
    max_outputs: 4,
    params: [
      { key: "steps", label: "Steps", kind: { type: "int", min: 1, max: 100, step: 1 }, default: 28, help: null },
      { key: "cfg", label: "CFG", kind: { type: "float", min: 1, max: 20, step: 0.5 }, default: 6.5, help: null },
      { key: "sampler", label: "Sampler", kind: { type: "select", options: [{ value: "dpmpp_2m", label: "DPM++ 2M" }, { value: "euler", label: "Euler" }] }, default: "dpmpp_2m", help: null },
    ],
  }),
];

export const demoJobs: Job[] = [
  {
    id: "job-run",
    provider: "fal",
    model: "fal-ai/kling-video/v3/pro",
    model_name: "Kling 3 Pro",
    request: { model: "x", task: "text_to_video", prompt: "Rain, steam and neon reflections on wet asphalt, slow push in", negative_prompt: null, images: [], aspect_ratio: "16:9", width: 1920, height: 1080, duration: 5, resolution: null, seed: null, count: 1, audio: null, params: {} },
    status: "running",
    progress: { fraction: 0.42, message: "Rendering · 42%" },
    created_at: now,
    finished_at: null,
    elapsed_ms: 0,
    outputs: [],
    error: null,
    seed: null,
    cost_usd: null,
    tag: null,
  },
  {
    id: "job-done",
    provider: "openrouter",
    model: "google/gemini-3-pro-image",
    model_name: "Nano Banana Pro",
    request: { model: "x", task: "text_to_image", prompt: "Warm abstract gradient, ember and cream, film grain", negative_prompt: null, images: [], aspect_ratio: "16:9", width: 1920, height: 1080, duration: null, resolution: null, seed: null, count: 1, audio: null, params: {} },
    status: "succeeded",
    progress: { fraction: 1, message: "Done" },
    created_at: now,
    finished_at: now,
    elapsed_ms: 9000,
    outputs: [{ path: "/ember.jpg", kind: "image", mime: "image/jpeg" }],
    error: null,
    seed: 7,
    cost_usd: 0.04,
    tag: null,
  },
];

/** Answers `invoke` calls in the browser. Edits are accepted but not applied. */
export async function mockInvoke(cmd: string, args: any): Promise<any> {
  const view = (): ProjectView => ({ project: demoProject, can_undo: true, can_redo: false });
  switch (cmd) {
    case "app_info":
      return { version: "0.1.0", ffmpeg: "/opt/homebrew/bin/ffmpeg", library: "~/Library/Application Support/kimchi" };
    case "list_projects":
      return demoSummaries;
    case "current_project":
      return new URLSearchParams(location.search).has("editor") ? view() : null;
    case "open_project":
    case "create_project":
    case "undo":
    case "redo":
      return view();
    case "apply_edit":
    case "apply_edits":
      return { view: view(), outcome: { created_clips: [], created_tracks: [] } };
    case "gen_providers":
    case "gen_set_key":
    case "gen_set_settings":
      return demoProviders;
    case "gen_models":
      return args?.provider ? demoModels.filter((m) => m.provider === args.provider) : demoModels;
    case "gen_jobs":
    case "gen_clear":
      return demoJobs;
    case "gen_check":
      return "Connected · $12.40 credit left";
    case "read_peaks": {
      const n = 2000;
      const a = new Float32Array(n);
      for (let i = 0; i < n; i++) a[i] = 0.25 + 0.6 * Math.abs(Math.sin(i / 23) * Math.sin(i / 7.3)) * (0.6 + 0.4 * Math.random());
      return a.buffer;
    }
    default:
      return null;
  }
}
