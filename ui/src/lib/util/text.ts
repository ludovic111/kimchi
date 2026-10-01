// Text layers are drawn with Canvas 2D, both in the preview and for export
// (the PNG handed to ffmpeg), so what you see is exactly what renders.
import type { TextStyle } from "../bindings/TextStyle";
import type { Transform } from "../bindings/Transform";

export const FONTS = [
  "Instrument Sans",
  "Instrument Serif",
  "Geist Mono",
  "Helvetica Neue",
  "Avenir Next",
  "Futura",
  "Georgia",
  "Didot",
  "Menlo",
  "Marker Felt",
];

const fontString = (s: TextStyle) => `${s.italic ? "italic " : ""}${s.font_weight} ${s.font_size}px "${s.font_family}"`;

export async function ensureFont(s: TextStyle) {
  try {
    await document.fonts.load(fontString(s), s.content || "A");
  } catch {}
}

/** Measured size of the text block in project pixels (before transform.scale). */
export function measure(ctx: CanvasRenderingContext2D, s: TextStyle): { w: number; h: number; lines: string[]; lineH: number } {
  ctx.font = fontString(s);
  if ("letterSpacing" in ctx) (ctx as any).letterSpacing = `${s.letter_spacing}px`;
  const lines = (s.content || " ").split("\n");
  const lineH = s.font_size * s.line_height;
  const w = Math.max(...lines.map((l) => ctx.measureText(l).width), 1);
  return { w, h: lineH * lines.length, lines, lineH };
}

/** Draws a text layer onto a canvas the size of the project. */
export function drawText(ctx: CanvasRenderingContext2D, s: TextStyle, t: Transform, W: number, H: number) {
  ctx.clearRect(0, 0, W, H);
  const m = measure(ctx, s);
  ctx.save();
  ctx.translate(W / 2 + t.x, H / 2 + t.y);
  ctx.rotate((t.rotation * Math.PI) / 180);
  ctx.scale(t.scale, t.scale);

  if (s.background) {
    const padX = s.font_size * 0.35;
    const padY = s.font_size * 0.18;
    ctx.fillStyle = s.background;
    const r = s.font_size * 0.18;
    roundRect(ctx, -m.w / 2 - padX, -m.h / 2 - padY, m.w + padX * 2, m.h + padY * 2, r);
    ctx.fill();
  }

  ctx.font = fontString(s);
  if ("letterSpacing" in ctx) (ctx as any).letterSpacing = `${s.letter_spacing}px`;
  ctx.textBaseline = "middle";
  ctx.fillStyle = s.color;
  if (s.shadow) {
    ctx.shadowColor = "rgba(0,0,0,0.45)";
    ctx.shadowBlur = s.font_size * 0.18;
    ctx.shadowOffsetY = s.font_size * 0.04;
  }
  const x = s.align === "left" ? -m.w / 2 : s.align === "right" ? m.w / 2 : 0;
  ctx.textAlign = s.align;
  m.lines.forEach((line, i) => {
    ctx.fillText(line, x, -m.h / 2 + m.lineH * (i + 0.5));
  });
  ctx.restore();
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

/** Full-canvas transparent PNG of a text layer, as a data URL. */
export async function textPng(s: TextStyle, t: Transform, W: number, H: number): Promise<string> {
  await ensureFont(s);
  const c = document.createElement("canvas");
  c.width = W;
  c.height = H;
  drawText(c.getContext("2d")!, s, t, W, H);
  return c.toDataURL("image/png");
}
