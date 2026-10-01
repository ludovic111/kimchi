import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  root,
  plugins: [svelte()],
  clearScreen: false,
  resolve: { alias: { $lib: `${root}src/lib` } },
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/target/**"] } },
  build: { outDir: `${root}dist`, emptyOutDir: true, target: "safari16", chunkSizeWarningLimit: 1500 },
});
