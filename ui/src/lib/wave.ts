// wave.ts — DESIGN.md §10. Reserved for whole-card status changes only (e.g. top status flips).
import { Pass } from './glkit';
import { WAVE, C, hex3 } from './tokens';
import { ticker, reduced } from './motion';

const FRAG = `#version 300 es
precision highp float;
uniform vec2  uRes, uSrc;            // device px, origin bottom-left
uniform float uR, uRMax, uT;         // front radius, covering radius, seconds since trigger
uniform float uLean, uCross, uSettle;
uniform float uLens, uGain, uSeed;
uniform vec3  uPal[6];
uniform vec3  uVeil;
out vec4 o;

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
vec3 pal(float x){
  float f = clamp(x, 0., .9999) * 5.;
  int i = int(floor(f));
  return mix(uPal[i], uPal[i + 1], fract(f));
}

void main(){
  vec2 q = gl_FragCoord.xy - uSrc;
  float dist = length(q);
  float d = dist - uR;                                   // < 0 behind the front
  float a = atan(q.y, q.x + 1e-4);

  // when did the front reach this pixel? closed-form inverse of easeOutCubic
  float x = 1. - pow(max(1. - dist / uRMax, 0.), 1. / 3.);
  float since = uT - (uLean + uCross * x);               // > 0 once passed
  float behind = step(d, 0.);
  float focus = smoothstep(0., uSettle, since);          // edges in the right order: low, high

  // rim colour: whole harmonics only, so it closes round the circle
  vec3 rim = pal(.5 + .5 * sin(2. * a + uSeed + d * .01));
  float core = exp(-d * d / (2. * uLens * uLens * .08));
  float halo = exp(-max(d, 0.) / uLens) * .55;
  float tail = exp(-max(-d, 0.) / (uLens * 1.8)) * .5 * behind;
  vec3 light = (rim * (halo + tail) + vec3(.92, .9, 1.) * core) * uGain;

  // veil + sparkles: dark behind the front, resolving outward from the source
  float veil = behind * (1. - focus) * .78;
  float sp = step(.985, hash(floor(gl_FragCoord.xy / 7.))) * behind * (1. - focus) * exp(-max(-d, 0.) / (uLens * 3.));

  vec3 c = clamp(light + uVeil * veil + rim * sp * .9, 0., 1.);
  float al = max(veil, max(c.r, max(c.g, c.b)));         // keeps rgb <= alpha: valid premultiplied
  o = vec4(c, al);
}`;

const easeOutCubic = (x: number) => 1 - Math.pow(1 - x, 3);

export interface WaveOpts {
  host: HTMLElement; oldEl: HTMLElement; newEl: HTMLElement;
  src: [number, number];                   // CSS px, host space, origin top-left
  pal: string[]; seed?: number;
  lens?: number;
  gain?: number; veil?: string;
}

/** Returns false when reduced motion is on (caller should skip and crossfade). */
export function playWave(o: WaveOpts): Promise<void> {
  if (reduced) return Promise.resolve();
  const { host, oldEl, newEl } = o;
  const { width: W, height: H } = host.getBoundingClientRect();
  const cv = document.createElement('canvas');
  cv.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;pointer-events:none;z-index:5';
  host.append(cv);
  const pass = new Pass(cv, FRAG); pass.resize(W, H); const dpr = pass.dpr;

  const [sx, sy] = o.src;
  const rMax = Math.hypot(Math.max(sx, W - sx), Math.max(sy, H - sy)) + 80;
  const pal = o.pal.flatMap(hex3), veil = hex3(o.veil ?? C.panel);
  newEl.hidden = false; newEl.style.clipPath = `circle(0px at ${sx}px ${sy}px)`;

  return new Promise((done) => {
    let t = 0;
    ticker.add((dt) => {
      t += dt;
      const x = Math.min(Math.max((t - WAVE.lean) / WAVE.cross, 0), 1);
      const r = rMax * easeOutCubic(x);
      const lean = Math.min(t / WAVE.lean, 1);
      oldEl.style.transform = `translate(${(sx - W / 2) * 0.012 * lean}px,${(sy - H / 2) * 0.012 * lean}px) scale(${1 - 0.012 * lean})`;
      oldEl.style.filter = `blur(${(3 * x).toFixed(2)}px)`;
      newEl.style.clipPath = `circle(${r.toFixed(1)}px at ${sx}px ${sy}px)`;

      pass.draw({
        uRes: [cv.width, cv.height], uSrc: [sx * dpr, (H - sy) * dpr],
        uR: r * dpr, uRMax: rMax * dpr, uT: t,
        uLean: WAVE.lean, uCross: WAVE.cross, uSettle: WAVE.settle,
        uLens: (o.lens ?? 46) * dpr, uGain: o.gain ?? 1, uSeed: o.seed ?? 1.3, uPal: pal, uVeil: veil,
      });

      if (t > WAVE.lean + WAVE.cross + WAVE.settle) {
        cv.remove(); oldEl.hidden = true;
        oldEl.style.transform = oldEl.style.filter = newEl.style.clipPath = '';
        done(); return false;
      }
      return true;
    });
  });
}
