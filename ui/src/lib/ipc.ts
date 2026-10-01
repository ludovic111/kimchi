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

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),

  listProjects: () => invoke<ProjectSummary[]>("list_projects"),
  createProject: (name: string, settings?: ProjectSettings) => invoke<ProjectView>("create_project", { name, settings }),
  openProject: (id: string) => invoke<ProjectView>("open_project", { id }),
  closeProject: () => invoke<void>("close_project"),
  deleteProject: (id: string) => invoke<void>("delete_project", { id }),
  duplicateProject: (id: string) => invoke<ProjectSummary>("duplicate_project", { id }),
  currentProject: () => invoke<ProjectView | null>("current_project"),
  applyEdit: (edit: Edit, coalesce?: string) => invoke<EditResponse>("apply_edit", { edit, coalesce }),
  applyEdits: (edits: Edit[]) => invoke<EditResponse>("apply_edits", { edits }),
  undo: () => invoke<ProjectView>("undo"),
  redo: () => invoke<ProjectView>("redo"),

  importMedia: (paths: string[]) => invoke<Asset[]>("import_media", { paths }),
  clipFrame: (clipId: string, time: number) => invoke<string>("clip_frame", { clipId, time }),
  writePng: (name: string, dataUrl: string) => invoke<string>("write_png", { name, dataUrl }),
  readPeaks: (path: string) => invoke<ArrayBuffer>("read_peaks", { path }),

  genProviders: () => invoke<ProviderStatus[]>("gen_providers"),
  genSetKey: (provider: string, key: string | null) => invoke<ProviderStatus[]>("gen_set_key", { provider, key }),
  genSetSettings: (provider: string, settings: ProviderSettings) =>
    invoke<ProviderStatus[]>("gen_set_settings", { provider, settings }),
  genCheck: (provider: string) => invoke<string>("gen_check", { provider }),
  genModels: (provider: string | null, refresh = false) => invoke<ModelInfo[]>("gen_models", { provider, refresh }),
  genJobs: () => invoke<Job[]>("gen_jobs"),
  genCancel: (jobId: string) => invoke<void>("gen_cancel", { jobId }),
  genClear: () => invoke<Job[]>("gen_clear"),
  genSubmit: (args: SubmitArgs) => invoke<Job>("gen_submit", { args }),

  exportStart: (settings: ExportSettings, overlays: Record<string, string>) =>
    invoke<string>("export_start", { settings, overlays }),
  exportCancel: (id: string) => invoke<void>("export_cancel", { id }),
};

export const events = {
  projectChanged: (f: (v: ProjectView) => void) => listen<ProjectView>("project-changed", (e) => f(e.payload)),
  job: (f: (j: Job) => void) => listen<Job>("gen-job", (e) => f(e.payload)),
  exportProgress: (f: (e: ExportEvent) => void) => listen<ExportEvent>("export-progress", (e) => f(e.payload)),
  toast: (f: (msg: string) => void) => listen<string>("toast", (e) => f(e.payload)),
};

export type { UnlistenFn };

/** URL the webview can load for a file on disk. */
export function fileUrl(path: string | null | undefined): string {
  if (!path) return "";
  return isTauri ? convertFileSrc(path) : path;
}

/** Normalises errors thrown by `invoke` (plain strings) and JS errors. */
export function message(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
