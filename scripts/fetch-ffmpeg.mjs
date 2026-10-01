// Downloads static ffmpeg/ffprobe builds for a Rust target triple and places them where Tauri
// expects sidecars: src-tauri/binaries/kimchi-{ffmpeg,ffprobe}-<triple>[.exe].
//   node scripts/fetch-ffmpeg.mjs [triple]      (defaults to this machine)
// Builds come from https://github.com/eugeneware/ffmpeg-static (GPL; licence copied alongside).
import { execSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { gunzipSync } from "node:zlib";

const RELEASE = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1";
const PLATFORMS = {
  "aarch64-apple-darwin": "darwin-arm64",
  "x86_64-apple-darwin": "darwin-x64",
  "x86_64-unknown-linux-gnu": "linux-x64",
  "aarch64-unknown-linux-gnu": "linux-arm64",
  "x86_64-pc-windows-msvc": "win32-x64",
};

const triple = process.argv[2] ?? execSync("rustc -vV").toString().match(/host: (\S+)/)[1];
const platform = PLATFORMS[triple];
if (!platform) throw new Error(`no ffmpeg build for ${triple}; supported: ${Object.keys(PLATFORMS).join(", ")}`);
const ext = triple.includes("windows") ? ".exe" : "";
const dir = new URL("../src-tauri/binaries/", import.meta.url);
mkdirSync(dir, { recursive: true });

async function get(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: ${r.status}`);
  return Buffer.from(await r.arrayBuffer());
}

for (const tool of ["ffmpeg", "ffprobe"]) {
  const out = new URL(`kimchi-${tool}-${triple}${ext}`, dir);
  if (existsSync(out) && !process.argv.includes("--force")) {
    console.log(`have ${out.pathname}`);
    continue;
  }
  writeFileSync(out, gunzipSync(await get(`${RELEASE}/${tool}-${platform}.gz`)));
  chmodSync(out, 0o755);
  console.log(`fetched ${out.pathname}`);
}
writeFileSync(new URL("FFMPEG-LICENSE.txt", dir), await get(`${RELEASE}/${platform}.LICENSE`));
