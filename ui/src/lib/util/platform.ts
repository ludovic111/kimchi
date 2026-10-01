// Small platform differences in the UI: window chrome and shortcut labels.
export const platform: "mac" | "windows" | "linux" = /Mac|iPhone|iPad/.test(navigator.userAgent)
  ? "mac"
  : /Windows/.test(navigator.userAgent)
    ? "windows"
    : "linux";

/** Shortcut labels are written Mac-style (⌘Z); spell them out elsewhere (Ctrl+Z). */
export function keys(label: string): string {
  if (platform === "mac") return label;
  return label.replace(/⇧/g, "Shift+").replace(/⌥/g, "Alt+").replace(/⌘/g, "Ctrl+").replace(/⌫/g, "Backspace").replace(/↵/g, "Enter");
}

export const SPONSOR_URL = "https://github.com/sponsors/ludovic111";
