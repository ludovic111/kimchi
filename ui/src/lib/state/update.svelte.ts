// Self-updates from GitHub Releases (signed; see tauri.conf.json → plugins.updater).
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { isTauri, message } from "../ipc";
import { ui } from "./ui.svelte";

const EVERY = 6 * 3600 * 1000;

class UpdateState {
  available = $state<Update | null>(null);
  /** 0–1 while downloading, null when idle. */
  progress = $state<number | null>(null);
  dismissed = $state(false);

  start() {
    if (!isTauri) return;
    setTimeout(() => this.check(), 4000);
    setInterval(() => this.check(), EVERY);
  }

  async check(manual = false) {
    if (!isTauri) return;
    try {
      const u = await check();
      if (u) {
        this.available = u;
        this.dismissed = false;
      } else if (manual) {
        ui.toast("kimchi is up to date.", "success");
      }
    } catch (e) {
      // No network or no release yet: stay quiet unless asked.
      if (manual) ui.error(`Couldn't check for updates: ${message(e)}`);
    }
  }

  async install() {
    const u = this.available;
    if (!u || this.progress !== null) return;
    let total = 0;
    let got = 0;
    this.progress = 0;
    try {
      await u.downloadAndInstall((ev) => {
        if (ev.event === "Started") total = ev.data.contentLength ?? 0;
        else if (ev.event === "Progress") {
          got += ev.data.chunkLength;
          this.progress = total ? got / total : 0;
        } else this.progress = 1;
      });
      await relaunch();
    } catch (e) {
      this.progress = null;
      ui.error(`Update failed: ${message(e)}`);
    }
  }
}

export const update = new UpdateState();
