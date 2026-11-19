// shell-geometry.ts — DESIGN.md §5.2. Pure functions; unit-tested in test.mjs.
export type Mode = 'rest' | 'peek' | 'open';
export interface Notch { w: number; h: number }

// Injected by the host at document start in native mode (window.__NOTCH__). Fallback = notchless Macs.
export const notch: Notch =
  (typeof window !== 'undefined' && (window as unknown as { __NOTCH__?: Notch }).__NOTCH__) || { w: 200, h: 32 };

export function frameFor(mode: Mode, contentH = 640, n: Notch = notch) {
  const wing = 60; // one metric per wing
  switch (mode) {
    case 'rest': return { w: n.w + wing * 2, h: n.h, r: 12, e: 10 };
    case 'peek': return { w: n.w + wing * 2 + 120, h: n.h + 44, r: 20, e: 10 };
    case 'open': return { w: 560, h: contentH, r: 36, e: 12 };
  }
}

const K = 0.62; // continuous-corner approximation; a circle is 0.5523
export function notchPath(w: number, h: number, r: number, e: number, x0 = 0): string {
  const X = (n: number) => +(n + x0).toFixed(2);
  return [
    `M${X(0)} 0`, `A${e} ${e} 0 0 1 ${X(e)} ${e}`, // left ear (concave)
    `V${h - r}`,
    `C${X(e)} ${h - r + r * K} ${X(e + r - r * K)} ${h} ${X(e + r)} ${h}`, // bottom-left
    `H${X(w - e - r)}`,
    `C${X(w - e - r + r * K)} ${h} ${X(w - e)} ${h - r + r * K} ${X(w - e)} ${h - r}`, // bottom-right
    `V${e}`, `A${e} ${e} 0 0 1 ${X(w)} 0`, `Z`, // right ear
  ].join('');
}
