// tokens.ts — DESIGN.md §3, single source of truth. CSS variables in base.css mirror this.
// v2 brand: fuchsia-purple, square pixels, hard edges. No blur. No shadow. No rounding.
export const C = {
  void: '#000000', // the only ground
  ink: '#000000', // text on a hot fill
  panel: '#0B0210', // card body: near-black violet, never pure black on black
  ground: '#12041A', // raised row / input well
  line: '#2A0A33', // the 1px rule family
  line2: '#4A1157', // stronger rule / hover
  f1: '#2A0A33', // deepest fuchsia
  f2: '#701A75', // dark fuchsia
  f3: '#A21CAF', // mid fuchsia
  f4: '#D946EF', // hot fuchsia — the action colour
  f5: '#F5D0FE', // pale lilac — the reading colour
  white: '#FFFFFF',
  // aliases kept for call sites that predate v2
  lilac: '#F5D0FE',
  peri: '#D946EF',
  blue: '#A21CAF',
  red: '#D946EF',
  pink: '#F5D0FE',
  core: '#2A0A33',
  navy: '#12041A',
  indigo: '#701A75',
  violet: '#A21CAF',
  orchid: '#D946EF',
  mist: '#F5D0FE',
} as const;

/** Radii are zero. The scale exists only so nothing can smuggle a curve back in. */
export const R = { s: 0, m: 0, l: 0, xl: 0, pill: 0 } as const;
export const S = (n: number) => n * 4;

export type Status = 'working' | 'idle' | 'failed' | 'done';

/** Status → ramp stop. Working is the hot one; idle is the dark one. */
export const STATUS_COLOR: Record<Status, string> = {
  working: C.f4,
  idle: C.f2,
  failed: C.f5,
  done: C.f3,
};

export interface GlowPreset { rim: string; mid: string; core: string; width: number; level: number }

// Legacy shape consumed by glow-orb; values are the v2 ramp.
export const GLOW: Record<Status, GlowPreset> = {
  working: { rim: C.f5, mid: C.f4, core: C.f2, width: 34, level: 1.0 },
  idle: { rim: C.f2, mid: C.f1, core: C.void, width: 16, level: 0.5 },
  failed: { rim: C.white, mid: C.f5, core: C.f2, width: 30, level: 1.0 },
  done: { rim: C.f5, mid: C.f3, core: C.f2, width: 42, level: 1.0 },
};

export const PAL: Record<Exclude<Status, 'idle'>, string[]> = {
  working: [C.f2, C.f3, C.f4, C.f5, C.white, C.f4],
  failed: [C.f5, C.white, C.f5, C.f4, C.white, C.f4],
  done: [C.f3, C.f4, C.f5, C.f3, C.f4, C.f5],
};

export const EASE = {
  out: 'cubic-bezier(.16, 1, .3, 1)', // expo-out: default for anything that arrives
  inOut: 'cubic-bezier(.65, 0, .35, 1)',
  snap: 'cubic-bezier(.2, .9, .1, 1)',
  step: 'steps(4, end)', // pixel-quantised motion: no interpolation
} as const;

export const SPRING_REACT = {
  open: { stiffness: 380, damping: 34 },
  hover: { stiffness: 520, damping: 38 },
  press: { stiffness: 700, damping: 42 },
  digit: { stiffness: 260, damping: 26 },
} as const;

export const MS = { press: 80, hover: 140, wipe: 220, state: 320, open: 420 } as const;
export const WAVE = { lean: 0.30, cross: 0.62, settle: 1.15 } as const;

export const SPRING = {
  open: { k: 380, c: 34 },
  hover: { k: 520, c: 38 },
  press: { k: 700, c: 42 },
  digit: { k: 260, c: 26 },
} as const;

export const hex3 = (h: string): [number, number, number] => {
  const n = parseInt(h.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
};
