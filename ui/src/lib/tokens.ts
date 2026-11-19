// tokens.ts — DESIGN.md §3, single source of truth. CSS variables in tokens.css are generated from this.
export const C = {
  void: '#04051A', // behind everything
  ink: '#0A0B1E', // text on white (hot state)
  panel: '#1E1F2E', // dark card body
  core: '#14153A', // orb dark core
  navy: '#0B0E3A',
  indigo: '#3A33C8',
  blue: '#4455EE', // electric mid
  peri: '#8F93F5',
  lilac: '#DCD5FA', // rim light
  mist: '#EEEAFD',
  violet: '#8B5CF6',
  orchid: '#C58CF0',
  white: '#FFFFFF',
  red: '#FF4D6D',
  pink: '#FFB3C1',
} as const;

export const R = { s: 8, m: 16, l: 28, xl: 36, pill: 999 } as const;
export const S = (n: number) => n * 4;

export type Status = 'working' | 'idle' | 'failed' | 'done';

export interface GlowPreset { rim: string; mid: string; core: string; width: number; level: number }

// width in CSS px, level 0..1. working = the reference look.
export const GLOW: Record<Status, GlowPreset> = {
  working: { rim: C.lilac, mid: C.blue, core: C.core, width: 34, level: 1.0 },
  idle: { rim: '#3B3D66', mid: '#1C1D3A', core: '#101126', width: 16, level: 0.5 },
  failed: { rim: C.pink, mid: C.red, core: '#1F0F1A', width: 30, level: 1.0 },
  done: { rim: C.white, mid: C.lilac, core: '#23234A', width: 42, level: 1.0 },
};

// wave palettes: six stops, read round a loop
export const PAL: Record<Exclude<Status, 'idle'>, string[]> = {
  working: [C.violet, C.blue, C.peri, C.lilac, C.orchid, C.indigo],
  failed: [C.red, C.pink, '#FF7A8A', C.red, '#FFD0D8', '#B3203F'],
  done: [C.white, C.mist, C.lilac, C.white, '#C9C2F5', '#F6F4FF'],
};

export const EASE = {
  out: 'cubic-bezier(.16, 1, .3, 1)', // expo-out: default for anything that arrives
  inOut: 'cubic-bezier(.65, 0, .35, 1)',
  snap: 'cubic-bezier(.2, .9, .1, 1)',
} as const;

// motion/react spring equivalents (DESIGN.md SPRING: k / c)
export const SPRING_REACT = {
  open: { stiffness: 380, damping: 34 },
  hover: { stiffness: 520, damping: 38 },
  press: { stiffness: 700, damping: 42 },
  digit: { stiffness: 260, damping: 26 },
} as const;

export const MS = { press: 80, hover: 140, wipe: 220, state: 320, open: 420 } as const;
export const WAVE = { lean: 0.30, cross: 0.62, settle: 1.15 } as const; // seconds

// zeta = c / (2 sqrt(k)) ≈ 0.79–0.87 → overshoot 0.4–1.7%
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
