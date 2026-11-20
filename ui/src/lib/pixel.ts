// pixel.ts — the brand engine. 8×8 Bayer ordered dither, halftone sphere, bitmap glyphs.
// Everything here draws SQUARE pixels with hard edges: no anti-aliasing, no blur, no shadow.
// One palette ramp is shared by every surface so the whole app dithers the same way.

/** The five-stop ramp. Order matters: dark → light. */
export const RAMP = ['#2A0A33', '#701A75', '#A21CAF', '#D946EF', '#F5D0FE'] as const;

/** Canonical 8×8 Bayer matrix, 0..63. */
export const BAYER8: readonly number[] = [
  0, 32, 8, 40, 2, 34, 10, 42,
  48, 16, 56, 24, 50, 18, 58, 26,
  12, 44, 4, 36, 14, 46, 6, 38,
  60, 28, 52, 20, 62, 30, 54, 22,
  3, 35, 11, 43, 1, 33, 9, 41,
  51, 19, 59, 27, 49, 17, 57, 25,
  15, 47, 7, 39, 13, 45, 5, 37,
  63, 31, 55, 23, 61, 29, 53, 21,
];

export const BAYER_NORM: readonly number[] = BAYER8.map((n) => (n + 0.5) / 64);

export type RGB = [number, number, number];

export function hexToRgb(hex: string): RGB {
  const h = hex.replace('#', '');
  const n = parseInt(h.length === 3 ? h.split('').map((c) => c + c).join('') : h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function rgba(hex: string, alpha = 1): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r},${g},${b},${alpha})`;
}

const RAMP_RGB: RGB[] = RAMP.map(hexToRgb);

/** Ordered-dither a 0..1 value into one of the ramp colours. Returns null below the floor. */
function rampColor(v: number, bx: number, by: number, floor: number): string | null {
  if (v <= floor) return null;
  const t = (v - floor) / (1 - floor);
  const span = RAMP_RGB.length - 1;
  const pos = t * span;
  const idx = Math.min(span - 1, Math.floor(pos));
  const frac = pos - idx;
  const threshold = BAYER_NORM[(by & 7) * 8 + (bx & 7)]!;
  const c = frac > threshold ? RAMP_RGB[idx + 1]! : RAMP_RGB[idx]!;
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}

export type DitherMode = 'linear' | 'radial' | 'flat' | 'scan';

export interface DitherOptions {
  /** CSS px per dither cell. 2 or 3 reads as pixel art; 1 reads as noise. */
  cell?: number;
  mode?: DitherMode;
  /** Gradient direction in degrees (linear/scan). 90 = top → bottom. */
  angle?: number;
  /** 0..1 value below which nothing is drawn. */
  floor?: number;
  /** 0..1 ceiling; values above it clamp to the lightest stop. */
  ceil?: number;
  /** Radial: centre as fractions of the canvas. */
  cx?: number;
  cy?: number;
  /** Phase 0..1 used by `scan` to slide the gradient. */
  phase?: number;
  /** Device pixel ratio multiplier; pass window.devicePixelRatio. */
  dpr?: number;
}

function valueAt(x: number, y: number, w: number, h: number, o: DitherOptions): number {
  const mode = o.mode ?? 'linear';
  const floor = o.floor ?? 0;
  const ceil = o.ceil ?? 1;
  let v: number;
  if (mode === 'flat') {
    v = 1;
  } else if (mode === 'radial') {
    const cx = (o.cx ?? 0.5) * w;
    const cy = (o.cy ?? 0.5) * h;
    const r = Math.hypot(w, h) * 0.62;
    v = 1 - Math.hypot(x - cx, y - cy) / r;
  } else {
    const a = ((o.angle ?? 90) * Math.PI) / 180;
    const dx = Math.cos(a);
    const dy = Math.sin(a);
    const span = Math.abs(dx) * w + Math.abs(dy) * h || 1;
    const origin = dx >= 0 ? (dy >= 0 ? 0 : h * Math.abs(dy)) : (dy >= 0 ? w * Math.abs(dx) : 0);
    v = (x * dx + y * dy + (mode === 'scan' ? (o.phase ?? 0) * span : 0) - origin) / span;
  }
  const t = Math.max(0, Math.min(1, v));
  return floor + t * (ceil - floor);
}

/** Paint an ordered-dithered ramp into the canvas (logical size w×h). */
export function paintDither(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  opts: DitherOptions = {},
): void {
  const cell = Math.max(1, Math.round(opts.cell ?? 2));
  const dpr = opts.dpr ?? 1;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  const cols = Math.ceil(w / cell);
  const rows = Math.ceil(h / cell);
  for (let gy = 0; gy < rows; gy++) {
    for (let gx = 0; gx < cols; gx++) {
      const x = gx * cell;
      const y = gy * cell;
      const v = valueAt(x + cell / 2, y + cell / 2, w, h, opts);
      const color = rampColor(v, gx, gy, opts.floor ?? 0);
      if (!color) continue;
      ctx.fillStyle = color;
      ctx.fillRect(x, y, Math.min(cell, w - x), Math.min(cell, h - y));
    }
  }
  ctx.setTransform(1, 0, 0, 1, 0, 0);
}

/**
 * Halftone sphere: a shaded ball of square pixels, dithered through the ramp.
 * `phase` (seconds) slowly rotates the light so the core reads as alive.
 */
export function paintSphere(
  ctx: CanvasRenderingContext2D,
  size: number,
  phase: number,
  opts: { cell?: number; dpr?: number } = {},
): void {
  const cell = Math.max(2, Math.round(opts.cell ?? 3));
  const dpr = opts.dpr ?? 1;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, size, size);
  const c = size / 2;
  const r = size * 0.46;
  const lightA = -2.2 + Math.sin(phase * 0.5) * 0.22;
  const lx = Math.cos(lightA);
  const ly = Math.sin(lightA);
  const cols = Math.ceil(size / cell);
  for (let gy = 0; gy < cols; gy++) {
    for (let gx = 0; gx < cols; gx++) {
      const x = (gx + 0.5) * cell;
      const y = (gy + 0.5) * cell;
      const nx = (x - c) / r;
      const ny = (y - c) / r;
      const d2 = nx * nx + ny * ny;
      if (d2 > 1) continue;
      const nz = Math.sqrt(1 - d2);
      // half-Lambert with a rim: dark core → hot edge, like the reference sphere
      const diff = Math.max(0, nx * lx + ny * ly + nz * 0.45);
      const rim = Math.pow(d2, 2.1) * 0.55;
      const v = Math.min(1, diff * 0.78 + rim + 0.06);
      const color = rampColor(v, gx, gy, 0.04);
      if (!color) continue;
      ctx.fillStyle = color;
      ctx.fillRect(gx * cell, gy * cell, cell, cell);
    }
  }
  ctx.setTransform(1, 0, 0, 1, 0, 0);
}

/** 5×7 bitmap glyphs — the wordmark and eyebrows. Only the letters the app needs. */
const GLYPHS: Record<string, string[]> = {
  N: ['10001', '11001', '10101', '10011', '10001', '10001', '10001'],
  O: ['01110', '10001', '10001', '10001', '10001', '10001', '01110'],
  T: ['11111', '00100', '00100', '00100', '00100', '00100', '00100'],
  C: ['01110', '10001', '10000', '10000', '10000', '10001', '01110'],
  H: ['10001', '10001', '10001', '11111', '10001', '10001', '10001'],
};

export const WORDMARK: { ch: string; bits: string[] }[] = 'NOTCH'
  .split('')
  .map((ch) => ({ ch, bits: GLYPHS[ch] ?? [] }));

/** Rendered glyph rectangles for an SVG wordmark: [x, y] cells at unit scale. */
export function wordmarkCells(): { x: number; y: number }[] {
  const cells: { x: number; y: number }[] = [];
  WORDMARK.forEach((g, i) => {
    const ox = i * 6;
    g.bits.forEach((row, y) => {
      for (let x = 0; x < row.length; x++) if (row[x] === '1') cells.push({ x: ox + x, y });
    });
  });
  return cells;
}
