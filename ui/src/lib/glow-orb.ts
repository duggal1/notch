// glow-orb.ts — DESIGN.md §7.2. Rim bloom orb; React owns mount/unmount via useEffect.
import { Pass } from './glkit';
import { GLOW, type Status, hex3 } from './tokens';
import { damp, ticker } from './motion';

const FRAG = `#version 300 es
precision highp float;
uniform vec2  uRes;       // device px
uniform vec2  uBox;       // half-size, device px
uniform float uR;         // corner radius, device px
uniform float uT;
uniform float uW;         // glow width, device px (fixed per status)
uniform float uLevel;     // strength: preset level * breathing
uniform float uSeed;
uniform vec3  uRim, uMid, uCore;
out vec4 o;

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float sdRBox(vec2 p, vec2 b, float r){ vec2 q = abs(p) - b + r; return length(max(q, 0.)) + min(max(q.x, q.y), 0.) - r; }

void main(){
  vec2 p = gl_FragCoord.xy - .5 * uRes;
  float d = sdRBox(p, uBox, uR);                     // < 0 inside
  float a = atan(p.y, p.x + 1e-4);                   // guard atan(0,0)
  // lobes: whole harmonics only (2, 3, 5), so there is no seam where the angle wraps
  float lobe = 1. + .34 * sin(2. * a + uT * .45 + uSeed)
                  + .22 * sin(3. * a - uT * .31 + uSeed * 1.7)
                  + .12 * sin(5. * a + uT * .20);
  float depth = max(-d, 0.);
  float edge  = exp(-depth / uW) * lobe;                         // tight rim light
  float halo  = exp(-depth / (uW * 2.8)) * (.75 + .25 * lobe);   // wide blue body
  vec3 c = uCore;
  c = mix(c, uMid, clamp(halo * uLevel, 0., 1.));
  c = mix(c, uRim, clamp(edge * uLevel, 0., 1.));
  c += (hash(gl_FragCoord.xy) - .5) / 255.;          // dither: kills banding in the long ramp
  float aa = 1. - smoothstep(-.75, .75, d);
  o = vec4(c * aa, aa);                              // premultiplied
}`;

export function breathe(s: Status, t: number): number {
  switch (s) {
    case 'working': return 1 + 0.08 * Math.sin((t * 2 * Math.PI) / 3.2); // 3.2 s inhale/exhale
    case 'failed': { // double-beat every 2 s
      const p = (t % 2) / 2, g = (c: number) => Math.exp(-(((p - c) / 0.035) ** 2));
      return 1 + 0.18 * (g(0.08) + 0.7 * g(0.22));
    }
    default: return 1;
  }
}

interface OrbOpts { radius: number; seed?: number; fps?: number }

export class GlowOrb {
  private pass: Pass; private off?: () => void;
  private cur = { rim: hex3(GLOW.idle.rim), mid: hex3(GLOW.idle.mid), core: hex3(GLOW.idle.core), width: GLOW.idle.width, level: GLOW.idle.level };
  private tgt = { ...this.cur };
  private status: Status = 'idle'; private t = 0; private acc = 0;

  constructor(private canvas: HTMLCanvasElement, private o: OrbOpts) {
    this.pass = new Pass(canvas, FRAG);
  }

  resize(w: number, h: number) { this.pass.resize(w, h); }

  setStatus(s: Status) {
    this.status = s; const g = GLOW[s];
    this.tgt = { rim: hex3(g.rim), mid: hex3(g.mid), core: hex3(g.core), width: g.width, level: g.level };
  }

  /** fps cap: 60 in open, 30 in peek, 0 in rest (use renderOnce for status changes) */
  setFps(fps: number) { this.o.fps = fps; }

  start() { this.off ??= ticker.add(this.frame); }
  stop() { this.off?.(); this.off = undefined; }

  /** one static render for rest mode (DESIGN.md §13: rest fps 0) — applies target colors immediately */
  renderOnce() {
    this.cur = {
      rim: [...this.tgt.rim] as [number, number, number],
      mid: [...this.tgt.mid] as [number, number, number],
      core: [...this.tgt.core] as [number, number, number],
      width: this.tgt.width, level: this.tgt.level,
    };
    this.t += 0.001;
    this.draw(this.cur.width, 1);
  }

  static supported(): boolean {
    return typeof document !== 'undefined' && !!document.createElement('canvas').getContext('webgl2');
  }

  private frame = (dt: number) => {
    this.t += dt; this.acc += dt;
    if (this.acc < 1 / (this.o.fps ?? 30)) return true;
    const step = this.acc; this.acc = 0;
    const λ = 7; // ≈ 99% of a colour change in 0.65 s
    for (const k of ['rim', 'mid', 'core'] as const)
      this.cur[k] = this.cur[k].map((v, i) => damp(v, this.tgt[k][i] as number, λ, step)) as [number, number, number];
    this.cur.width = damp(this.cur.width, this.tgt.width, λ, step);
    this.cur.level = damp(this.cur.level, this.tgt.level, λ, step);
    this.draw(this.cur.width, breathe(this.status, this.t));
    return true;
  };

  private draw(width: number, breatheLevel: number) {
    const { width: W, height: H } = this.canvas, dpr = this.pass.dpr;
    this.pass.draw({
      uRes: [W, H], uBox: [W / 2, H / 2], uR: this.o.radius * dpr, uT: this.t,
      uW: width * dpr, uLevel: this.cur.level * breatheLevel,
      uSeed: this.o.seed ?? 1.3, uRim: this.cur.rim, uMid: this.cur.mid, uCore: this.cur.core,
    });
  }
}
