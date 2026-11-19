// motion.ts — DESIGN.md §6. One rAF loop; springs are pure functions of dt.
export type SpringCfg = { k: number; c: number; m?: number; eps?: number };

export class Spring {
  v = 0; x: number;
  constructor(public target: number, private cfg: SpringCfg) { this.x = target; }
  step(dt: number) { // semi-implicit Euler, 240 Hz substeps → stable at any frame rate
    const { k, c, m = 1 } = this.cfg;
    let left = Math.min(dt, 1 / 20);
    while (left > 0) {
      const s = Math.min(1 / 240, left);
      this.v += ((-k * (this.x - this.target) - c * this.v) / m) * s;
      this.x += this.v * s; left -= s;
    }
    return this.x;
  }
  get settled() { const e = this.cfg.eps ?? 0.001; return Math.abs(this.v) < e && Math.abs(this.x - this.target) < e; }
}

// One rAF loop for the whole app. Sleeps when nothing is subscribed.
export type TickerFn = (dt: number, t: number) => boolean | void; // return false to unsubscribe
class Ticker {
  private fns = new Set<TickerFn>(); private raf = 0; private last = 0;
  add(fn: TickerFn) {
    this.fns.add(fn);
    if (!this.raf) this.raf = requestAnimationFrame(this.loop);
    return () => { this.fns.delete(fn); };
  }
  private loop = (now: number) => {
    const dt = this.last ? (now - this.last) / 1000 : 0; this.last = now;
    for (const f of [...this.fns]) if (f(dt, now / 1000) === false) this.fns.delete(f);
    if (this.fns.size && !document.hidden) this.raf = requestAnimationFrame(this.loop);
    else { this.raf = 0; this.last = 0; }
  };
}
export const ticker = new Ticker();
if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => { if (!document.hidden) ticker.add(() => false); });
}

export const damp = (a: number, b: number, lambda: number, dt: number) => b + (a - b) * Math.exp(-lambda * dt);

export const reduced = typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;
// reduced motion: springs → 120ms opacity fades, no breathing, no ambient loops, no wave (crossfade only).

export const mulberry32 = (a: number) => () => { // seeded PRNG: the only randomness allowed
  let t = (a += 0x6D2B79F5);
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
