// Where a layer sits on the canvas. Mirrors the export filter graph in
// kimchi-media so the preview matches the render.
import type { Clip } from "../bindings/Clip";
import type { Transform } from "../bindings/Transform";

export interface Box {
  /** Centre, in canvas pixels. */
  cx: number;
  cy: number;
  w: number;
  h: number;
  rotation: number;
}

export function fitBox(t: Transform, iw: number, ih: number, W: number, H: number): Box {
  let sx: number;
  let sy: number;
  if (t.fit === "stretch") {
    sx = W / iw;
    sy = H / ih;
  } else {
    const s = t.fit === "cover" ? Math.max(W / iw, H / ih) : Math.min(W / iw, H / ih);
    sx = sy = s;
  }
  return { cx: W / 2 + t.x, cy: H / 2 + t.y, w: iw * sx * t.scale, h: ih * sy * t.scale, rotation: t.rotation };
}

/** Opacity of a clip at timeline time `t`, including fades. */
export function opacityAt(c: Clip, t: number): number {
  const local = t - c.start;
  let a = c.transform.opacity;
  if (c.fade_in > 0 && local < c.fade_in) a *= Math.max(0, local / c.fade_in);
  const tail = c.duration - local;
  if (c.fade_out > 0 && tail < c.fade_out) a *= Math.max(0, tail / c.fade_out);
  return a;
}

export function volumeAt(c: Clip, t: number): number {
  return Math.min(1, opacityAt({ ...c, transform: { ...c.transform, opacity: 1 } }, t) * c.volume);
}
