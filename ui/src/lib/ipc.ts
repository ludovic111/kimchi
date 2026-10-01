// Typed bridge to the Rust side. Every type here is generated from Rust (see bindings/).
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppInfo } from "./bindings/AppInfo";
import type { Asset } from "./bindings/Asset";
import type { Edit } from "./bindings/Edit";
import type { EditResponse } from "./bindings/EditResponse";
import type { ExportEvent } from "./bindings/ExportEvent";
import type { ExportSettings } from "./bindings/ExportSettings";
import type { Job } from "./bindings/Job";
import type { ModelInfo } from "./bindings/ModelInfo";
import type { ProjectSettings } from "./bindings/ProjectSettings";
import type { ProjectSummary } from "./bindings/ProjectSummary";
import type { ProjectView } from "./bindings/ProjectView";
import type { ProviderSettings } from "./bindings/ProviderSettings";
import type { ProviderStatus } from "./bindings/ProviderStatus";
import type { SubmitArgs } from "./bindings/SubmitArgs";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** `invoke`, or demo data when running in a plain browser (`npm run ui:dev`). */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri) return invoke<T>(cmd, args);
  const { mockInvoke } = await import("./mock");
  return mockInvoke(cmd, args);
}

function on<T>(event: string, f: (payload: T) => void): Promise<UnlistenFn> {
  if (!isTauri) return Promise.resolve(() => {});
  return listen<T>(event, (e) => f(e.payload));
}

export const api = {
  appInfo: () => call<AppInfo>("app_info"),

  listProjects: () => call<ProjectSummary[]>("list_projects"),
  createProject: (name: string, settings?: ProjectSettings) => call<ProjectView>("create_project", { name, settings }),
  openProject: (id: string) => call<ProjectView>("open_project", { id }),
  closeProject: () => call<void>("close_project"),
  deleteProject: (id: string) => call<void>("delete_project", { id }),
  duplicateProject: (id: string) => call<ProjectSummary>("duplicate_project", { id }),
  currentProject: () => call<ProjectView | null>("current_project"),
  applyEdit: (edit: Edit, coalesce?: string) => call<EditResponse>("apply_edit", { edit, coalesce }),
  applyEdits: (edits: Edit[]) => call<EditResponse>("apply_edits", { edits }),
  undo: () => call<ProjectView>("undo"),
  redo: () => call<ProjectView>("redo"),

  importMedia: (paths: string[]) => call<Asset[]>("import_media", { paths }),
  clipFrame: (clipId: string, time: number) => call<string>("clip_frame", { clipId, time }),
  writePng: (name: string, dataUrl: string) => call<string>("write_png", { name, dataUrl }),
  readPeaks: (path: string) => call<ArrayBuffer>("read_peaks", { path }),

  genProviders: () => call<ProviderStatus[]>("gen_providers"),
  genSetKey: (provider: string, key: string | null) => call<ProviderStatus[]>("gen_set_key", { provider, key }),
  genSetSettings: (provider: string, settings: ProviderSettings) =>
    invoke<ProviderStatus[]>("gen_set_settings", { provider, settings }),
  genCheck: (provider: string) => call<string>("gen_check", { provider }),
  genModels: (provider: string | null, refresh = false) => call<ModelInfo[]>("gen_models", { provider, refresh }),
  genJobs: () => call<Job[]>("gen_jobs"),
  genCancel: (jobId: string) => call<void>("gen_cancel", { jobId }),
  genClear: () => call<Job[]>("gen_clear"),
  genSubmit: (args: SubmitArgs) => call<Job>("gen_submit", { args }),

  exportStart: (settings: ExportSettings, overlays: Record<string, string>) =>
    invoke<string>("export_start", { settings, overlays }),
  exportCancel: (id: string) => call<void>("export_cancel", { id }),
};

export const events = {
  projectChanged: (f: (v: ProjectView) => void) => on<ProjectView>("project-changed", f),
  job: (f: (j: Job) => void) => on<Job>("gen-job", f),
  exportProgress: (f: (e: ExportEvent) => void) => on<ExportEvent>("export-progress", f),
  toast: (f: (msg: string) => void) => on<string>("toast", f),
};

export type { UnlistenFn };

/** URL the webview can load for a file on disk. */
export function fileUrl(path: string | null | undefined): string {
  if (!path) return "";
  return isTauri ? convertFileSrc(path) : path.replace(/^\/?(.*)$/, "/$1");
}

/** Normalises errors thrown by `invoke` (plain strings) and JS errors. */
export function message(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
