/** `mm:ss.ff` (or `h:mm:ss.ff`) with frames. */
export function timecode(t: number, fps = 30): string {
  const total = Math.max(0, t);
  const frames = Math.floor((total % 1) * fps + 1e-6);
  const s = Math.floor(total) % 60;
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${h ? `${h}:${p(m)}` : p(m)}:${p(s)}.${p(frames)}`;
}

/** Compact human duration: `4.2s`, `1:05`. */
export function short(t: number): string {
  if (t < 60) return `${t < 10 ? t.toFixed(1) : Math.round(t)}s`;
  const m = Math.floor(t / 60);
  return `${m}:${String(Math.round(t % 60)).padStart(2, "0")}`;
}

export function ago(iso: string): string {
  const s = (Date.now() - new Date(iso).getTime()) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  if (s < 86400 * 7) return `${Math.floor(s / 86400)} d ago`;
  return new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const u = ["KB", "MB", "GB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${u[i]}`;
}
