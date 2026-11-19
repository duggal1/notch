// floating-ids.ts — DESIGN.md §9.6. Real session IDs drift at 7–19% white; live ones carry a bloom.
// Positions are a pure function of (ids, seed): same snapshot, same layout.
import { mulberry32 } from './motion';

export interface FidInput { id: string; live: boolean }
export interface FidPlacement extends FidInput {
  left: number; top: number;   // %
  o: number;                   // opacity 0.07..0.19
  d: number;                   // drift duration s
  dx: number;                  // drift distance px
}

export function computeFloatingIds(ids: FidInput[], seed = 7): FidPlacement[] {
  const rnd = mulberry32(seed);
  return ids.map(({ id, live }) => ({
    id, live,
    left: +(rnd() * 88).toFixed(1),
    top: +(rnd() * 86).toFixed(1),
    o: +(0.07 + rnd() * 0.12).toFixed(2),
    d: +(18 + rnd() * 14).toFixed(1),
    dx: +(rnd() * 12 - 6).toFixed(1),
  }));
}
