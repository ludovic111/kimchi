// Transient UI state: panels, dialogs, toasts, menus.

export type LeftTab = "media" | "generate" | "text";
export type ToastKind = "info" | "error" | "success";

export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

export interface MenuItem {
  label: string;
  icon?: any;
  shortcut?: string;
  danger?: boolean;
  gen?: boolean;
  disabled?: boolean;
  action?: () => void;
}
export type MenuEntry = MenuItem | "separator";

class UiState {
  leftTab = $state<LeftTab>("media");
  settingsOpen = $state(false);
  settingsProvider = $state<string | null>(null);
  exportOpen = $state(false);
  paletteOpen = $state(false);
  jobsOpen = $state(false);
  toasts = $state<Toast[]>([]);
  menu = $state<{ x: number; y: number; items: MenuEntry[] } | null>(null);
  leftWidth = $state(340);
  rightWidth = $state(300);
  timelineHeight = $state(300);
  #nextId = 1;

  toast(text: string, kind: ToastKind = "info", ms = 4200) {
    const id = this.#nextId++;
    this.toasts = [...this.toasts, { id, kind, text }];
    setTimeout(() => this.dismiss(id), ms);
  }

  error(text: string) {
    this.toast(text, "error", 7000);
  }

  dismiss(id: number) {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  openMenu(e: MouseEvent, items: MenuEntry[]) {
    e.preventDefault();
    e.stopPropagation();
    this.menu = { x: e.clientX, y: e.clientY, items };
  }

  openSettings(provider: string | null = null) {
    this.settingsProvider = provider;
    this.settingsOpen = true;
  }
}

export const ui = new UiState();
