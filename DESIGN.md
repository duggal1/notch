# NOTCH — design.md

Agent control plane that lives in the MacBook notch. This file is the contract. If an implementation disagrees with it, the implementation is wrong.

Status: v1. Snippets are written to compile, not run. Expect typos, not design errors.

---

## 0. Read first

**What this is.** The visual spec for the notch + menu-bar surface of the orchestration layer (Orchestrator → Supervisors → OpenCode sessions). The four reference images are the visual source of truth:

1. Glass squircle app icon, floor glow, a wave bleeding off one edge.
2. The "Excellent" orb: dark core, lobed lavender rim bloom, white pill, huge numeral, mono caption.
3. The Stability card: bottom-anchored floor glow, A/B range ticks, floating mono tags, film grain.
4. The frosted metric board: dot-matrix time series, dot grid with hollow "loss" dots, radar tile.

**Assumed stack.** UI in TypeScript inside a WKWebView in a borderless NSPanel. Swift owns the window, notch geometry, haptics and SF Symbols. Rust owns state and pushes a `Snapshot` over the bridge. One WebGL2 context per effect surface, vanilla TS, no framework. If the surface ends up SwiftUI, §11 has that path.

**The contradiction, resolved.** The strict brief bans soft gradients. The references are made of them. Resolution: **geometry is hard, light is soft.** Every edge, radius, grid cell and baseline is exact. The only soft thing allowed is glow, and glow is a data channel: color = status, level = load, motion = liveness. Glow that carries no state gets deleted.

**What does not transfer from the references:**

- Ref 4's light lavender backdrop. The notch hardware is pure black. A non-black top edge exposes the cutout and the whole illusion dies. Default surface is `night` (§3). `glass` is optional for a detached panel only.
- Typos. Refs 2 and 4 say "MPBS". Copy the style, not the typo.
- Ref 2 is an iPhone Dynamic Island, a floating pill. The Mac notch is a fixed cutout, so the shell must merge into it with concave ears (§4).

---

## 1. Laws

1. **Hover and selection = solid `#FFFFFF`.** Ink flips to `#0A0B1E`. Never black, never gray, never translucent. Keyboard focus is the same state.
2. **Glow is state.** Color = status, level = load, breathing = liveness. No decorative glow.
3. **One accent family** (indigo → lilac) plus exactly two signal hues: red for failed, white for done and hot. Nothing else.
4. **Radii from the scale only:** 8 · 16 · 28 · 36 · 999.
5. **4pt grid.** Hairlines are 1px. Nothing is off-grid by "optical" excuse.
6. **Type:** Inter for UI, system mono for labels. Numerals tabular. Weights 400 / 500 / 600 only.
7. **Labels** are mono, uppercase, 10.5px, tracking `.18em`, 56% white. Always.
8. **Motion:** springs for size and position, exponential ease for opacity and color. Nothing linear. Overshoot ≤ 2%. Nothing over 420ms except the wave (0.62s cross + 1.15s settle) and ambient loops.
9. **Pure functions of time.** Render paths use springs or `f(t)`. Randomness is a seeded PRNG only.
10. **Ambient loops exist only in `open`.** `rest` is static except on status change.
11. **No drop shadows.** Light only: colored glow or white bloom.
12. **No emoji, no illustration, no mixed icon weights.** Outline at rest, `.fill` variant when hot.

---

## 2. Reference → component map

| Ref | Steal this | Notch component | Data |
|---|---|---|---|
| 1 | Glass squircle, floor glow, wave bleeding off one edge | App icon, state-change wave | none |
| 2 | Dark core, lobed rim bloom, pill, big numeral, mono caption | `GlowOrb` + wings in `rest` / `peek` | working count |
| 3 | Floor glow, A/B ticks, drifting mono tags, grain | `Drift` card, `FloatingIds` | step latency per session |
| 4 | Frosted tiles, dot-matrix series, dot grid with hollow dots, radar | `Heartbeat`, `SessionGrid`, `Radar` | events/s, session states, supervisors |

---

## 3. Tokens — `tokens.ts`

Single source of truth. CSS variables are generated from this file, never hand-edited.

```ts
// tokens.ts
export const C = {
  void:   '#04051A', // behind everything
  ink:    '#0A0B1E', // text on white (hot state)
  panel:  '#1E1F2E', // dark card body (ref 3, top of card)
  core:   '#14153A', // orb dark core (ref 2, centre)
  navy:   '#0B0E3A',
  indigo: '#3A33C8',
  blue:   '#4455EE', // electric mid (ref 2 ring, ref 4 radar)
  peri:   '#8F93F5',
  lilac:  '#DCD5FA', // rim light
  mist:   '#EEEAFD',
  violet: '#8B5CF6',
  orchid: '#C58CF0', // warm bloom (ref 2 lower, ref 4 hero)
  white:  '#FFFFFF',
  red:    '#FF4D6D',
  pink:   '#FFB3C1',
} as const;

export const R = { s: 8, m: 16, l: 28, xl: 36, pill: 999 } as const;
export const S = (n: number) => n * 4;

export type Status = 'working' | 'idle' | 'failed' | 'done';

export interface GlowPreset { rim: string; mid: string; core: string; width: number; level: number }

// width in CSS px, level 0..1. working = the reference look.
export const GLOW: Record<Status, GlowPreset> = {
  working: { rim: C.lilac,  mid: C.blue,   core: C.core,    width: 34, level: 1.0 },
  idle:    { rim: '#3B3D66', mid: '#1C1D3A', core: '#101126', width: 16, level: 0.5 },
  failed:  { rim: C.pink,   mid: C.red,    core: '#1F0F1A', width: 30, level: 1.0 },
  done:    { rim: C.white,  mid: C.lilac,  core: '#23234A', width: 42, level: 1.0 },
};

// wave palettes: six stops, read round a loop
export const PAL: Record<Exclude<Status, 'idle'>, string[]> = {
  working: [C.violet, C.blue, C.peri, C.lilac, C.orchid, C.indigo],
  failed:  [C.red, C.pink, '#FF7A8A', C.red, '#FFD0D8', '#B3203F'],
  done:    [C.white, C.mist, C.lilac, C.white, '#C9C2F5', '#F6F4FF'],
};

export const EASE = {
  out:   'cubic-bezier(.16, 1, .3, 1)',  // expo-out: default for anything that arrives
  inOut: 'cubic-bezier(.65, 0, .35, 1)',
  snap:  'cubic-bezier(.2, .9, .1, 1)',
} as const;

export const MS = { press: 80, hover: 140, wipe: 220, state: 320, open: 420 } as const;
export const WAVE = { lean: 0.30, cross: 0.62, settle: 1.15 } as const; // seconds

// zeta = c / (2 sqrt(k)) ≈ 0.79–0.87 → overshoot 0.4–1.7%
export const SPRING = {
  open:  { k: 380, c: 34 },
  hover: { k: 520, c: 38 },
  press: { k: 700, c: 42 },
  digit: { k: 260, c: 26 },
} as const;

export const hex3 = (h: string): [number, number, number] => {
  const n = parseInt(h.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
};
export const cssVars = () => Object.entries(C).map(([k, v]) => `--c-${k}:${v}`).join(';');
```

---

## 4. Type and surfaces — `base.css`

```css
@font-face {
  font-family: 'Inter';
  src: url('./fonts/InterVariable.woff2') format('woff2');
  font-weight: 100 900; font-display: block;   /* bundle it. Same type on every machine. */
}

html, body { margin: 0; background: transparent; overflow: hidden; -webkit-font-smoothing: antialiased; }
.ui   { font: 500 13px/1.25 Inter, system-ui, sans-serif; letter-spacing: -.01em; color: #fff; }

/* the label: ref 2 "MBPS DOWNLOAD", ref 3 "B 9.21%" */
.label {
  font: 500 10.5px/1 ui-monospace, 'SF Mono', monospace;
  letter-spacing: .18em; text-transform: uppercase;
  color: rgba(255,255,255,.56);
  font-variant-numeric: slashed-zero tabular-nums;
}

/* numerals: ref 2 "912.3" */
.num      { font-variant-numeric: tabular-nums; font-feature-settings: 'tnum', 'zero'; }
.num-xl   { font: 500 88px/.9 Inter; letter-spacing: -.045em; }
.num-l    { font: 500 40px/1  Inter; letter-spacing: -.035em; }
.title    { font: 500 20px/1.2 Inter; letter-spacing: -.02em; }

/* night: default. The shell must be pure #000 to fuse with the hardware notch. */
.shell { position: absolute; top: 0; left: 50%; transform: translateX(-50%); background: #000; overflow: hidden; }

.card  { position: relative; isolation: isolate; overflow: hidden; border-radius: 36px; background: #1E1F2E; }
.tile  { border-radius: 28px; }

/* 1px gradient hairline: bright top-left, near-zero middle (ref 2, 3, 4 edges) */
.card::after, .tile::after {
  content: ''; position: absolute; inset: 0; border-radius: inherit; padding: 1px; pointer-events: none;
  background: linear-gradient(160deg, rgba(255,255,255,.38), rgba(255,255,255,.06) 40%, rgba(255,255,255,.14));
  -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0);
  -webkit-mask-composite: xor; mask-composite: exclude;
}

/* film grain: ref 3 background. Static, cheap, kills banding in long ramps. */
.grain::before {
  content: ''; position: absolute; inset: 0; z-index: 3; pointer-events: none; opacity: .07; mix-blend-mode: overlay;
  background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='160' height='160'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='.9' numOctaves='2' stitchTiles='stitch'/><feColorMatrix values='0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0 0 0 .5 0'/></filter><rect width='100%' height='100%' filter='url(%23n)'/></svg>");
}

/* glass: optional, detached panel only. Frost needs a backdrop; the black shell has none. */
.glass { background: rgba(255,255,255,.10); -webkit-backdrop-filter: blur(28px) saturate(1.4); box-shadow: inset 0 1px 0 rgba(255,255,255,.28); }

/* the pill: ref 2 "Excellent" */
.pill { display: inline-flex; align-items: center; height: 22px; padding: 0 10px; border-radius: 999px;
        background: #E9E5FB; color: #0A0B1E; font: 500 12px/1 Inter; letter-spacing: -.01em; }
```

Type scale: `88 / 40 / 20 / 13 / 10.5mono`. Nothing between. Copy rules: sentences are plain and carry a number ("Six sessions. Step latency never drifted more than 9.21%."). Units always present. IDs are `S1-OC01`. No exclamation marks, no "Oops".

---

## 5. Notch geometry

### 5.1 Host window (Swift)

The notch is hardware. Read it, never hardcode it. Fallback (200×32) is for notchless Macs and external displays, so the design stays identical there.

```swift
// NotchHost.swift
import AppKit
import WebKit

extension NSScreen {
    var notch: CGSize? {
        guard safeAreaInsets.top > 0,
              let l = auxiliaryTopLeftArea, let r = auxiliaryTopRightArea else { return nil }
        return CGSize(width: frame.width - l.width - r.width, height: safeAreaInsets.top)
    }
}

final class NotchPanel: NSPanel {
    init(screen: NSScreen, size: CGSize) {
        let f = NSRect(x: screen.frame.midX - size.width / 2, y: screen.frame.maxY - size.height,
                       width: size.width, height: size.height)
        super.init(contentRect: f, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 3)
        collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary, .ignoresCycle]
        isOpaque = false; backgroundColor = .clear; hasShadow = false
        isMovable = false; hidesOnDeactivate = false
    }
    override var canBecomeKey: Bool { true }   // keyboard nav on rows
}

func makeWebView(notch n: CGSize) -> WKWebView {
    let cfg = WKWebViewConfiguration()
    cfg.setURLSchemeHandler(SymbolScheme(), forURLScheme: "symbol")             // §11
    let js = "window.__NOTCH__={w:\(Int(n.width)),h:\(Int(n.height))}"
    cfg.userContentController.addUserScript(WKUserScript(source: js, injectionTime: .atDocumentStart, forMainFrameOnly: true))
    let v = WKWebView(frame: .zero, configuration: cfg)
    v.setValue(false, forKey: "drawsBackground"); v.underPageBackgroundColor = .clear
    return v
}

// haptics: one tick on open, one on failure. Nothing else.
func tick(_ p: NSHapticFeedbackManager.FeedbackPattern = .alignment) {
    NSHapticFeedbackManager.defaultPerformer.perform(p, performanceTime: .now)
}
```

Window strategy: resize the NSPanel **once** per mode change, never per frame. Grow the window to the target size *before* the shell animates open; shrink it *after* the shell finishes closing. The shell morph happens inside the window with a JS-driven `clip-path`.

### 5.2 Shell shape (TS)

The shell hangs from the menu bar. Concave "ears" at the top corners blend it into the bar and the cutout. Bottom corners use a continuous-corner approximation (`K = 0.62`; a circle is 0.5523).

```ts
// shell.ts
import { Spring, ticker } from './motion';
import { SPRING } from './tokens';

export type Mode = 'rest' | 'peek' | 'open';
export const notch: { w: number; h: number } = (window as any).__NOTCH__ ?? { w: 200, h: 32 };

export function frameFor(mode: Mode, contentH = 640, n = notch) {
  const wing = 60;                                  // one metric per wing
  switch (mode) {
    case 'rest': return { w: n.w + wing * 2,       h: n.h,      r: 12, e: 10 };
    case 'peek': return { w: n.w + wing * 2 + 120, h: n.h + 44, r: 20, e: 10 };
    case 'open': return { w: 560,                  h: contentH, r: 36, e: 12 };
  }
}

const K = 0.62;
export function notchPath(w: number, h: number, r: number, e: number): string {
  return [
    `M0 0`, `A${e} ${e} 0 0 1 ${e} ${e}`,                                    // left ear (concave)
    `V${h - r}`,
    `C${e} ${h - r + r * K} ${e + r - r * K} ${h} ${e + r} ${h}`,            // bottom-left
    `H${w - e - r}`,
    `C${w - e - r + r * K} ${h} ${w - e} ${h - r + r * K} ${w - e} ${h - r}`, // bottom-right
    `V${e}`, `A${e} ${e} 0 0 1 ${w} 0`, `Z`,                                 // right ear
  ].join('');
}

export class Shell {
  private f = frameFor('rest');
  private w = new Spring(this.f.w, SPRING.open);
  private h = new Spring(this.f.h, SPRING.open);
  private r = new Spring(this.f.r, SPRING.open);
  private e = this.f.e;
  mode: Mode = 'rest';

  constructor(private el: HTMLElement) { this.paint(); }

  to(mode: Mode, contentH?: number) {
    this.mode = mode;
    const f = frameFor(mode, contentH);
    this.w.target = f.w; this.h.target = f.h; this.r.target = f.r; this.e = f.e;
    ticker.add(this.tick);
  }

  private tick = (dt: number) => {
    const w = this.w.step(dt), h = this.h.step(dt), r = this.r.step(dt);
    this.paint(w, h, r);
    return !(this.w.settled && this.h.settled && this.r.settled);   // false = unsubscribe
  };

  private paint(w = this.w.x, h = this.h.x, r = this.r.x) {
    const s = this.el.style;
    s.width = `${w}px`; s.height = `${h}px`;
    s.clipPath = `path('${notchPath(w, h, Math.max(0, Math.min(r, h - this.e)), this.e)}')`;
  }
}
```

Intent model: pointer enters shell → `peek` after 60ms. Click → `open`. Pointer leaves → `rest` after 240ms (not while `open`). `Esc`, click-outside or loss of key → `rest`. Every `open` fires `tick(.alignment)`; every failure fires `tick(.levelChange)`.

```ts
// intent.ts
export function wireIntent(shell: Shell, el: HTMLElement, post: (m: object) => void) {
  let t = 0;
  const go = (m: Mode, ms: number) => { clearTimeout(t); t = window.setTimeout(() => { post({ type: 'setFrame', mode: m }); shell.to(m); }, ms); };
  el.addEventListener('pointerenter', () => shell.mode === 'rest' && go('peek', 60));
  el.addEventListener('pointerleave', () => shell.mode !== 'open' && go('rest', 240));
  el.addEventListener('click',        () => go('open', 0));
  addEventListener('keydown', e => e.key === 'Escape' && go('rest', 0));
}
```

---

## 6. Motion engine — `motion.ts`

```ts
// motion.ts
export type SpringCfg = { k: number; c: number; m?: number; eps?: number };

export class Spring {
  v = 0; x: number;
  constructor(public target: number, private cfg: SpringCfg) { this.x = target; }
  step(dt: number) {                         // semi-implicit Euler, 240 Hz substeps → stable at any frame rate
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
type Fn = (dt: number, t: number) => boolean | void;   // return false to unsubscribe
class Ticker {
  private fns = new Set<Fn>(); private raf = 0; private last = 0;
  add(fn: Fn) { this.fns.add(fn); if (!this.raf) this.raf = requestAnimationFrame(this.loop); return () => this.fns.delete(fn); }
  private loop = (now: number) => {
    const dt = this.last ? (now - this.last) / 1000 : 0; this.last = now;
    for (const f of [...this.fns]) if (f(dt, now / 1000) === false) this.fns.delete(f);
    if (this.fns.size && !document.hidden) this.raf = requestAnimationFrame(this.loop);
    else { this.raf = 0; this.last = 0; }
  };
}
export const ticker = new Ticker();
document.addEventListener('visibilitychange', () => { if (!document.hidden) ticker.add(() => false); }); // kick the loop awake

export const damp = (a: number, b: number, lambda: number, dt: number) => b + (a - b) * Math.exp(-lambda * dt);

export const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
// reduced motion: springs → 120ms opacity fades, no breathing, no ambient loops, no wave (crossfade only).

export const mulberry32 = (a: number) => () => {          // seeded PRNG: the only randomness allowed
  let t = (a += 0x6D2B79F5);
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
```

---

## 7. The glow system

Four layers. Each answers one question about the system.

| Layer | Answers | Source |
|---|---|---|
| Rim bloom (`GlowOrb`) | What state is it in? | Ref 2 |
| Floor glow | How loaded is it? | Ref 3 |
| Breathing | Is it alive? | Ref 2 |
| Pointer specular | Where is my cursor? | Ref 1 |

### 7.1 GL kit — `glkit.ts`

One fullscreen-triangle pass helper. Both the orb and the wave use it.

```ts
// glkit.ts
type U = Record<string, number | number[]>;

const compile = (gl: WebGL2RenderingContext, type: number, src: string) => {
  const s = gl.createShader(type)!;
  gl.shaderSource(s, src); gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader');
  return s;
};

export class Pass {
  gl: WebGL2RenderingContext; dpr = 1;
  private prog: WebGLProgram; private locs = new Map<string, WebGLUniformLocation | null>();

  constructor(public canvas: HTMLCanvasElement, frag: string) {
    const gl = canvas.getContext('webgl2', { alpha: true, premultipliedAlpha: true, antialias: false, powerPreference: 'low-power' });
    if (!gl) throw new Error('webgl2 unavailable');
    this.gl = gl;
    // gl_VertexID triangle: no buffers, no attributes
    const vs = `#version 300 es
void main(){ vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)); gl_Position = vec4(p * 2. - 1., 0., 1.); }`;
    const prog = gl.createProgram()!;
    gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, vs));
    gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, frag.trimStart())); // "#version" must be byte 0 or it won't compile
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? 'link');
    this.prog = prog;
  }

  resize(cssW: number, cssH: number, cap = 2) {
    this.dpr = Math.min(window.devicePixelRatio || 1, cap);
    this.canvas.width = Math.round(cssW * this.dpr);
    this.canvas.height = Math.round(cssH * this.dpr);
  }

  draw(u: U) {
    const { gl } = this;
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.useProgram(this.prog);
    gl.clearColor(0, 0, 0, 0); gl.clear(gl.COLOR_BUFFER_BIT);
    for (const [k, v] of Object.entries(u)) {
      let l = this.locs.get(k);
      if (l === undefined) { l = gl.getUniformLocation(this.prog, k); this.locs.set(k, l); }
      if (!l) continue;
      if (typeof v === 'number') gl.uniform1f(l, v);
      else if (v.length === 2) gl.uniform2fv(l, v);
      else if (v.length === 4) gl.uniform4fv(l, v);
      else gl.uniform3fv(l, v);                 // vec3 and vec3[] (flat)
    }
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}
```

### 7.2 Rim bloom — the reference orb

Anatomy of ref 2: dark core → electric blue mid → pale lavender rim, with the rim **lobed** (brighter blobs at corners and flanks, thinner at the top and bottom). The lobes come from whole angular harmonics, so they close seamlessly round the shape. The glow's **width is fixed** and only its **strength** varies, because varying the width makes hard edges deep inside (`exp(-d/w)` is very sensitive to `w`).

```ts
// glow-orb.ts
import { Pass } from './glkit';
import { GLOW, Status, hex3 } from './tokens';
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
    case 'working': return 1 + 0.08 * Math.sin((t * 2 * Math.PI) / 3.2);              // 3.2 s inhale/exhale
    case 'failed': {                                                                   // double-beat every 2 s
      const p = (t % 2) / 2, g = (c: number) => Math.exp(-(((p - c) / 0.035) ** 2));
      return 1 + 0.18 * (g(0.08) + 0.7 * g(0.22));
    }
    default: return 1;
  }
}

export class GlowOrb {
  private pass: Pass; private off?: () => void;
  private cur = { rim: hex3(GLOW.idle.rim), mid: hex3(GLOW.idle.mid), core: hex3(GLOW.idle.core), width: GLOW.idle.width, level: GLOW.idle.level };
  private tgt = { ...this.cur };
  private status: Status = 'idle'; private t = 0; private acc = 0;

  constructor(private canvas: HTMLCanvasElement, private o: { radius: number; seed?: number; fps?: number }) {
    this.pass = new Pass(canvas, FRAG);
  }

  resize(w: number, h: number) { this.pass.resize(w, h); }

  setStatus(s: Status) {
    this.status = s; const g = GLOW[s];
    this.tgt = { rim: hex3(g.rim), mid: hex3(g.mid), core: hex3(g.core), width: g.width, level: g.level };
  }

  /** fps cap: 60 in open, 30 in peek, call stop() in rest */
  setFps(fps: number) { this.o.fps = fps; }

  start() { this.off ??= ticker.add(this.frame); }
  stop()  { this.off?.(); this.off = undefined; }

  private frame = (dt: number) => {
    this.t += dt; this.acc += dt;
    if (this.acc < 1 / (this.o.fps ?? 30)) return;
    const step = this.acc; this.acc = 0;
    const λ = 7;                                      // ≈ 99% of a colour change in 0.65 s
    for (const k of ['rim', 'mid', 'core'] as const)
      this.cur[k] = this.cur[k].map((v, i) => damp(v, this.tgt[k][i], λ, step)) as [number, number, number];
    this.cur.width = damp(this.cur.width, this.tgt.width, λ, step);
    this.cur.level = damp(this.cur.level, this.tgt.level, λ, step);

    const { width: W, height: H } = this.canvas, dpr = this.pass.dpr;
    this.pass.draw({
      uRes: [W, H], uBox: [W / 2, H / 2], uR: this.o.radius * dpr, uT: this.t,
      uW: this.cur.width * dpr, uLevel: this.cur.level * breathe(this.status, this.t),
      uSeed: this.o.seed ?? 1.3, uRim: this.cur.rim, uMid: this.cur.mid, uCore: this.cur.core,
    });
  };
}
```

Mount it behind content, clipped by the card's own squircle:

```css
.orb { position: relative; border-radius: 36px; overflow: hidden; }
.orb > canvas { position: absolute; inset: 0; width: 100%; height: 100%; z-index: 0; }
.orb > .content { position: relative; z-index: 1; }
```

### 7.3 Floor glow (ref 3)

Anchored to the bottom edge, lavender → periwinkle → indigo → the dark card body, with two faint blue blobs near the top edge. `--lvl` (0..1) is the load. It animates as a real number through `@property`, so the glow rises like liquid.

```css
@property --lvl { syntax: '<number>'; inherits: true; initial-value: 0; }

.floor {
  position: absolute; inset: 0; z-index: 0; pointer-events: none;
  transition: --lvl 900ms cubic-bezier(.16, 1, .3, 1);
  background:
    radial-gradient(120% calc(34% + 46% * var(--lvl)) at 50% 108%,
      #E4DCFB 0%, rgba(228,220,251,.92) 16%, rgba(98,100,232,.88) 38%,
      rgba(58,51,200,.55) 58%, transparent 80%),
    radial-gradient(60% 40% at 18% 6%,  rgba(100,100,255,.28), transparent 70%),
    radial-gradient(40% 30% at 82% 10%, rgba(120,110,255,.22), transparent 70%);
}
.floor[data-s='failed'] {
  background:
    radial-gradient(120% calc(34% + 46% * var(--lvl)) at 50% 108%,
      #FFD0D8 0%, rgba(255,179,193,.9) 16%, rgba(255,77,109,.8) 38%, rgba(120,20,50,.5) 58%, transparent 80%),
    radial-gradient(60% 40% at 18% 6%, rgba(255,77,109,.18), transparent 70%);
}
```

```ts
// floor.ts — load is events/s normalised 0..1 against a rolling max
export const setLoad = (el: HTMLElement, load: number) => el.style.setProperty('--lvl', Math.min(1, Math.max(0, load)).toFixed(3));
```

### 7.4 Pointer specular

A soft white light that follows the cursor with spring lag. It is the "glass" in the glass.

```ts
// specular.ts
import { Spring, ticker } from './motion';
import { SPRING } from './tokens';

export function specular(el: HTMLElement) {
  const x = new Spring(0.5, SPRING.hover), y = new Spring(0.0, SPRING.hover);
  const tick = (dt: number) => {
    el.style.setProperty('--sx', (x.step(dt) * 100).toFixed(2) + '%');
    el.style.setProperty('--sy', (y.step(dt) * 100).toFixed(2) + '%');
    return !(x.settled && y.settled);
  };
  el.addEventListener('pointermove', e => {
    const r = el.getBoundingClientRect();
    x.target = (e.clientX - r.left) / r.width; y.target = (e.clientY - r.top) / r.height;
    ticker.add(tick);
  });
  el.addEventListener('pointerenter', () => el.style.setProperty('--spec', '1'));
  el.addEventListener('pointerleave', () => el.style.setProperty('--spec', '0'));
}
```

```css
.card .spec {
  position: absolute; inset: 0; z-index: 2; pointer-events: none; border-radius: inherit;
  background: radial-gradient(260px circle at var(--sx, 50%) var(--sy, 0%), rgba(255,255,255,.10), transparent 60%);
  opacity: var(--spec, 0); transition: opacity 200ms cubic-bezier(.16, 1, .3, 1);
}
```

---

## 8. Hover and selection = all white

The most important rule in this file. A hot item becomes **solid `#FFFFFF`**. Ink flips to `#0A0B1E`. Icons flip through `currentColor`. A white bloom is the only decoration.

The fill is not a fade. It is a **wipe from where the pointer entered**, and it retracts toward where the pointer left. That directionality is what makes it feel physical.

```css
.item {
  position: relative; isolation: isolate; border-radius: 16px; color: #fff;
  --hx: 50%; --hy: 50%;
  transition: color 140ms cubic-bezier(.16, 1, .3, 1), box-shadow 220ms cubic-bezier(.16, 1, .3, 1), transform 80ms cubic-bezier(.2, .9, .1, 1);
}
.item::before {                                   /* the white */
  content: ''; position: absolute; inset: 0; z-index: -1; border-radius: inherit; background: #FFFFFF;
  clip-path: circle(0% at var(--hx) var(--hy));
  transition: clip-path 220ms cubic-bezier(.16, 1, .3, 1);
}
.item[data-hot] { color: #0A0B1E; box-shadow: 0 0 0 .5px #fff, 0 0 28px 2px rgba(255,255,255,.42), 0 0 80px 8px rgba(220,213,250,.32); }
.item[data-hot]::before { clip-path: circle(150% at var(--hx) var(--hy)); }   /* ≥141.5% reaches the far corner from any point */
.item[data-hot] .label { color: rgba(10,11,30,.58); }
.item:active { transform: scale(.985); }                                       /* still white while pressed */

/* actions: hidden at rest, slide in 6px, 40ms stagger */
.item .act { opacity: 0; transform: translateX(6px); transition: opacity 140ms, transform 220ms cubic-bezier(.16, 1, .3, 1); }
.item[data-hot] .act { opacity: 1; transform: none; }
.item[data-hot] .act:nth-child(2) { transition-delay: 40ms; }
.item[data-hot] .act:nth-child(3) { transition-delay: 80ms; }
```

```ts
// hot.ts
export function whiteHover(root: HTMLElement, sel = '.item') {
  const origin = (el: HTMLElement, e: PointerEvent) => {
    const r = el.getBoundingClientRect();
    el.style.setProperty('--hx', ((e.clientX - r.left) / r.width * 100).toFixed(1) + '%');
    el.style.setProperty('--hy', ((e.clientY - r.top) / r.height * 100).toFixed(1) + '%');
  };
  root.addEventListener('pointerover', e => {
    const el = (e.target as HTMLElement).closest<HTMLElement>(sel);
    if (!el || el.contains(e.relatedTarget as Node)) return;
    origin(el, e); el.toggleAttribute('data-hot', true);
  });
  root.addEventListener('pointerout', e => {
    const el = (e.target as HTMLElement).closest<HTMLElement>(sel);
    if (!el || el.contains(e.relatedTarget as Node)) return;
    origin(el, e); el.toggleAttribute('data-hot', false);   // retracts toward the exit point
  });
  root.addEventListener('focusin',  e => (e.target as HTMLElement).closest(sel)?.toggleAttribute('data-hot', true));
  root.addEventListener('focusout', e => (e.target as HTMLElement).closest(sel)?.toggleAttribute('data-hot', false));
}
```

Row markup:

```html
<div class="item row" tabindex="0" data-id="S1-OC02">
  <i class="dot" data-s="working"></i>
  <div class="grow"><div class="title-s">Refactor auth middleware</div><div class="label">S1-OC02 · SV-1</div></div>
  <span class="label num">02:14</span>
  <button class="act" aria-label="Logs">…</button><button class="act" aria-label="Re-prompt">…</button><button class="act" aria-label="Kill">…</button>
</div>
```

---

## 9. Components

### 9.1 `NumberRoll` — the "912.3" numeral

Each digit is a column of 0–9. A spring moves the column; velocity drives a small vertical blur (motion blur), so fast changes read as fast. Counters scroll *through* intermediate digits (9 → 0 rolls back through 8…1). That is intended: direction equals value.

```ts
// number-roll.ts
import { Spring, ticker } from './motion';
import { SPRING } from './tokens';

type Col = { digit: boolean; node: HTMLElement; strip?: HTMLElement; sp: Spring };

export class NumberRoll {
  private cols: Col[] = [];
  constructor(private root: HTMLElement) { root.style.cssText += ';display:inline-flex;height:1em;line-height:1;overflow:hidden'; }

  set(v: number, fmt: (n: number) => string = n => n.toFixed(1)) {
    const s = fmt(v);
    if (s.length !== this.cols.length) this.build(s);
    [...s].forEach((ch, i) => { const c = this.cols[i]; if (c.digit) c.sp.target = +ch; else c.node.textContent = ch; });
    ticker.add(this.tick);
  }

  private build(s: string) {
    this.root.textContent = ''; this.cols = [];
    for (const ch of s) {
      const node = document.createElement('span'); const digit = /\d/.test(ch);
      if (!digit) { node.textContent = ch; this.cols.push({ digit, node, sp: new Spring(0, SPRING.digit) }); this.root.append(node); continue; }
      node.style.cssText = 'display:block;height:1em;overflow:hidden';
      const strip = document.createElement('span'); strip.style.cssText = 'display:block;will-change:transform';
      strip.innerHTML = Array.from({ length: 10 }, (_, d) => `<i style="display:block;height:1em;font-style:normal">${d}</i>`).join('');
      node.append(strip); this.root.append(node);
      this.cols.push({ digit, node, strip, sp: new Spring(+ch, SPRING.digit) });
    }
  }

  private tick = (dt: number) => {
    let busy = false;
    for (const c of this.cols) {
      if (!c.digit) continue;
      const x = c.sp.step(dt);
      c.strip!.style.transform = `translateY(${-x}em)`;
      c.strip!.style.filter = `blur(${Math.min(1.4, Math.abs(c.sp.v) * 0.09).toFixed(2)}px)`;
      busy ||= !c.sp.settled;
    }
    return busy;
  };
}
```

### 9.2 `Heartbeat` — the dot-matrix series (ref 4, "0S … 10S")

Dots on a square pitch. Lit dots are white at 96%, unlit at 22%. The leftmost columns fade out. The whole field scrolls left one column per sample on a spring. The newest column grows from zero and carries one bright head dot (the dot at the top right of the reference).

Data: aggregate events/s across all sessions, sampled every 240ms (≈ 10 s window). Axis labels: `−10S` and `NOW`.

```ts
// heartbeat.ts
import { Spring, ticker } from './motion';
import { SPRING } from './tokens';

export class Heartbeat {
  private ctx: CanvasRenderingContext2D; private cols: number; private rows: number;
  private vals: number[]; private off = new Spring(0, SPRING.open); private head = new Spring(0, SPRING.hover);
  constructor(private cv: HTMLCanvasElement, private pitch = 11, rows = 9) {
    const w = cv.clientWidth, h = rows * pitch, dpr = Math.min(devicePixelRatio, 2);
    cv.width = w * dpr; cv.height = h * dpr; cv.style.height = h + 'px';
    this.ctx = cv.getContext('2d')!; this.ctx.scale(dpr, dpr);
    this.cols = Math.floor(w / pitch); this.rows = rows; this.vals = new Array(this.cols).fill(0);
  }
  /** v: 0..1 */
  push(v: number) {
    this.vals.shift(); this.vals.push(Math.round(v * this.rows));
    this.off.x = 1; this.off.v = 0; this.off.target = 0;          // slide one column left
    this.head.x = 0; this.head.v = 0; this.head.target = this.vals[this.cols - 1];
    ticker.add(this.draw);
  }
  private draw = (dt: number) => {
    const { ctx, pitch, rows, cols } = this, o = this.off.step(dt), hd = this.head.step(dt);
    ctx.clearRect(0, 0, cols * pitch, rows * pitch);
    for (let c = 0; c < cols; c++) {
      const x = (c + o + 0.5) * pitch, fade = Math.min(1, (c + o) / 6);
      const lit = c === cols - 1 ? hd : this.vals[c];
      for (let r = 0; r < rows; r++) {
        const on = Math.max(0, Math.min(1, lit - r));                // fractional last dot = partial alpha
        ctx.fillStyle = `rgba(255,255,255,${((0.22 + 0.74 * on) * fade).toFixed(3)})`;
        ctx.beginPath(); ctx.arc(x, (rows - 1 - r + 0.5) * pitch, 1.5, 0, 7); ctx.fill();
      }
      if (c === cols - 1) {                                          // head dot
        ctx.fillStyle = '#fff'; ctx.beginPath(); ctx.arc(x, (rows - 1 - lit + 0.5) * pitch - pitch, 2.6, 0, 7); ctx.fill();
      }
    }
    return !(this.off.settled && this.head.settled);
  };
}
```

### 9.3 `SessionGrid` — the packet-loss tile (ref 4)

Rows = supervisors. Columns = OpenCode session slots. One dot per slot.

| State | Dot |
|---|---|
| working | filled white, a diagonal highlight sweeps through (scan) |
| idle | filled white at 30% |
| failed | **hollow ring**, pink, pulsing on the failed heartbeat |
| done | filled lilac |
| empty | nothing drawn |

```ts
// session-grid.ts
import { C } from './tokens';
import { breathe } from './glow-orb';
import type { Status } from './tokens';

export type Cell = Status | null;

export function drawSessionGrid(ctx: CanvasRenderingContext2D, grid: Cell[][], t: number, pitch = 20, r = 4.2) {
  ctx.clearRect(0, 0, 4096, 4096);                                   // ctx is pre-scaled by dpr
  grid.forEach((row, j) => row.forEach((s, i) => {
    if (!s) return;
    const x = pitch * (i + 0.5), y = pitch * (j + 0.5), edge = Math.min(1, (i + 1) / 3);   // left columns fade in
    if (s === 'working') {
      const b = Math.pow(Math.max(0, Math.sin((i + j) * 0.35 - t * 2.2)), 8);              // travelling diagonal bump
      ctx.fillStyle = `rgba(255,255,255,${((0.78 + 0.22 * b) * edge).toFixed(3)})`;
      ctx.beginPath(); ctx.arc(x, y, r, 0, 7); ctx.fill();
    } else if (s === 'idle') {
      ctx.fillStyle = `rgba(255,255,255,${(0.3 * edge).toFixed(3)})`;
      ctx.beginPath(); ctx.arc(x, y, r, 0, 7); ctx.fill();
    } else if (s === 'done') {
      ctx.fillStyle = C.lilac; ctx.globalAlpha = 0.95 * edge;
      ctx.beginPath(); ctx.arc(x, y, r, 0, 7); ctx.fill(); ctx.globalAlpha = 1;
    } else {                                                          // failed: hollow
      ctx.strokeStyle = C.pink; ctx.lineWidth = 1.5;
      ctx.beginPath(); ctx.arc(x, y, r * (0.86 + 0.14 * (breathe('failed', t) - 1) / 0.18), 0, 7); ctx.stroke();
    }
  }));
}
```

### 9.4 `Radar` — the "Nearest server" tile (ref 4)

Center dot = Orchestrator. Satellite dots = Supervisors, placed on the middle ring. A dashed tether runs to the hottest one. Rings ping outward every 2.4 s. The tile is the one blue gradient in the layout.

```ts
// radar.ts
import { C } from './tokens';
import type { Status } from './tokens';

export function radarSVG(sups: { id: string; status: Status }[], hot = 0): string {
  const pts = sups.map((s, i) => {
    const a = (i / sups.length) * Math.PI * 2 - Math.PI / 4, R = 52;
    return { ...s, x: +(Math.cos(a) * R).toFixed(1), y: +(Math.sin(a) * R).toFixed(1) };
  });
  const h = pts[hot];
  const fill = (s: Status) => (s === 'failed' ? C.pink : s === 'idle' ? 'rgba(255,255,255,.4)' : '#fff');
  return `
<svg viewBox="-100 -100 200 200" class="radar" aria-hidden="true">
  <g fill="none" stroke="rgba(255,255,255,.32)" stroke-width=".75">
    <circle r="30"/><circle r="52"/><circle r="82" opacity=".5"/>
    <circle r="52" class="ping"/>
  </g>
  ${h ? `<line x1="0" y1="0" x2="${h.x}" y2="${h.y}" stroke="#fff" stroke-width="1" stroke-dasharray="2 3" class="tether"/>` : ''}
  <circle r="3.4" fill="#fff"/>
  ${pts.map(p => `<circle cx="${p.x}" cy="${p.y}" r="9" fill="rgba(255,255,255,.14)"/><circle cx="${p.x}" cy="${p.y}" r="4.6" fill="${fill(p.status)}"/>`).join('')}
</svg>`;
}
```

```css
.tile--blue { background: linear-gradient(180deg, #3B49E0 0%, #2A2EB0 68%, #6D7DEE 100%); }
.radar .ping   { transform-box: fill-box; transform-origin: center; animation: ping 2.4s cubic-bezier(.16, 1, .3, 1) infinite; }
.radar .tether { animation: dash 1.2s linear infinite; }      /* the one allowed linear: constant-speed marching ants */
@keyframes ping { from { transform: scale(.55); opacity: .7; } to { transform: scale(1.5); opacity: 0; } }
@keyframes dash { to { stroke-dashoffset: -10; } }
```

### 9.5 `Drift` — the Stability card (ref 3)

Vertical range bars: min–max step latency per session, a dot at each end, the latest session in full white and the rest at 45%. Two dashed reference lines: `A` (best) and `B` (worst), labeled in mono. A single plain-language sentence sits above.

```ts
// drift.ts
export function driftSVG(runs: { lo: number; hi: number }[], W = 340, H = 140): string {
  const lo = Math.min(...runs.map(r => r.lo)), hi = Math.max(...runs.map(r => r.hi));
  const y = (v: number) => +(H - 12 - ((v - lo) / (hi - lo || 1)) * (H - 24)).toFixed(1);
  const step = (W - 90) / runs.length;
  const bars = runs.map((r, i) => {
    const x = 24 + step * (i + 0.5), last = i === runs.length - 1, o = last ? 1 : 0.45;
    return `<line x1="${x}" x2="${x}" y1="${y(r.hi)}" y2="${y(r.lo)}" stroke="rgba(255,255,255,${o})" stroke-width="${last ? 2 : 1.25}"/>
            <circle cx="${x}" cy="${y(r.hi)}" r="${last ? 5 : 4}" fill="rgba(255,255,255,${o})"/>
            <circle cx="${x}" cy="${y(r.lo)}" r="${last ? 5 : 4}" fill="rgba(255,255,255,${o * 0.8})"/>`;
  }).join('');
  const pct = ((hi - lo) / lo * 100).toFixed(2);
  return `<svg viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" class="drift">
    <g stroke="rgba(255,255,255,.28)" stroke-dasharray="2 3"><line x1="0" x2="${W - 70}" y1="${y(hi)}" y2="${y(hi)}"/><line x1="0" x2="${W - 70}" y1="${y(lo)}" y2="${y(lo)}"/></g>
    <g class="label" fill="rgba(255,255,255,.8)" font-family="ui-monospace" font-size="10.5" letter-spacing="1.9">
      <text x="${W - 62}" y="${y(hi) + 4}">B ${pct}%</text><text x="${W - 62}" y="${y(lo) + 4}">A 0.00%</text></g>
    ${bars}</svg>`;
}
```

### 9.6 `FloatingIds` — the drifting "1059" tags (ref 3)

In the reference they are texture. Here they must earn their place: they are real session IDs, at 7–19% white, drifting slowly. A "live" one carries a white dot with a bloom. Click one to focus that session. Deterministic: same IDs, same positions.

```ts
// floating-ids.ts
import { mulberry32 } from './motion';

export function floatingIds(host: HTMLElement, ids: { id: string; live: boolean }[], seed = 7) {
  host.querySelectorAll('.fid').forEach(n => n.remove());
  const rnd = mulberry32(seed);
  for (const { id, live } of ids) {
    const el = document.createElement('button');
    el.className = 'fid'; el.textContent = id; el.dataset.id = id; if (live) el.dataset.live = '';
    el.style.cssText = `left:${(rnd() * 88).toFixed(1)}%;top:${(rnd() * 86).toFixed(1)}%;--o:${(0.07 + rnd() * 0.12).toFixed(2)};--d:${(18 + rnd() * 14).toFixed(1)}s;--dx:${(rnd() * 12 - 6).toFixed(1)}px`;
    host.append(el);
  }
}
```

```css
.fid { position: absolute; z-index: 1; background: none; border: 0; padding: 0; cursor: default;
       font: 400 11px ui-monospace, 'SF Mono', monospace; letter-spacing: .3em; text-align: center;
       color: rgba(255,255,255,var(--o, .12)); animation: drift var(--d, 24s) ease-in-out infinite alternate; }
.fid[data-live]::before { content: ''; display: block; width: 6px; height: 6px; margin: 0 auto 6px; border-radius: 50%;
                          background: #fff; box-shadow: 0 0 10px rgba(255,255,255,.8); }
.fid:hover { color: #fff; }
@keyframes drift { to { transform: translate(var(--dx, 4px), -10px); } }
```

### 9.7 Wiring — `mount.ts`

```ts
// mount.ts
import { Shell, wireIntent, frameFor } from './shell';
import { GlowOrb } from './glow-orb';
import { Heartbeat } from './heartbeat';
import { NumberRoll } from './number-roll';
import { whiteHover } from './hot';
import { specular } from './specular';
import { setLoad } from './floor';
import type { Status } from './tokens';

export interface Session    { id: string; sup: string; status: Status; title: string; startedAt: number }
export interface Supervisor { id: string; status: Status }
export interface Snapshot   { sessions: Session[]; supervisors: Supervisor[]; eps: number }

// Swift → web: window.__push(snapshot)      web → Swift: webkit.messageHandlers.notch.postMessage(...)
const post = (m: object) => (window as any).webkit?.messageHandlers?.notch?.postMessage(m);

export function mount() {
  const shellEl = document.querySelector<HTMLElement>('.shell')!;
  const shell = new Shell(shellEl);
  wireIntent(shell, shellEl, post);

  const orb = new GlowOrb(document.querySelector('.orb canvas')!, { radius: 36, seed: 1.3 });
  orb.resize(shellEl.clientWidth, shellEl.clientHeight);
  const count = new NumberRoll(document.querySelector('#working')!);
  const beat  = new Heartbeat(document.querySelector('#beat')!);
  whiteHover(document.body); specular(document.querySelector('.orb')!);

  let top: Status = 'idle', prev: Status = 'idle';
  (window as any).__push = (s: Snapshot) => {
    const w = s.sessions.filter(x => x.status === 'working').length;
    top = s.sessions.some(x => x.status === 'failed') ? 'failed' : w ? 'working' : s.sessions.length && s.sessions.every(x => x.status === 'done') ? 'done' : 'idle';
    orb.setStatus(top); count.set(w, n => String(n).padStart(2, '0'));
    setLoad(document.querySelector('.floor')!, Math.min(1, s.eps / 40));
    beat.push(Math.min(1, s.eps / 40));
    if (top !== prev) { post({ type: 'haptic', kind: top === 'failed' ? 'levelChange' : 'alignment' }); prev = top; }
    shell.mode === 'rest' ? orb.stop() : orb.start();
  };
}
```

---

## 10. State-change wave

A press sends a wave out of the button. The old state leans toward the source and blurs, a rim sweeps across, the new state resolves behind it out of a dark veil with sparkles left in the wake, and the palette lingers on the edges. This is the ref 1 / ref 2 "wave bleeding off the edge", turned into a transition.

Timing: lean 0.30s → cross 0.62s → settle 1.15s. Total 2.07s, deliberately longer than anything else. It is reserved for **status changes of a whole card** (a supervisor going working → failed, a session finishing). Rows use a 320ms halo crossfade instead.

**How it works in a live DOM.** The old and new states are two stacked layers. The new one is revealed by a `clip-path: circle()` whose radius follows `easeOutCubic`. A WebGL2 overlay draws the rim, the veil and the sparkles from the same radius. Per-pixel *focus* (each pixel sharpening a second after the front passes) is approximated by the veil's alpha. True per-pixel blur needs both states rasterized to textures. That is a v2 if the veil reads as dim instead of blurry.

```ts
// wave.ts
import { Pass } from './glkit';
import { WAVE, C, hex3 } from './tokens';
import { ticker } from './motion';

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
  lens?: number;                           // rim half-width. Long thin cards: ~130 → use 40–60 and gain .8
  gain?: number; veil?: string;
}

export function playWave(o: WaveOpts): Promise<void> {
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

  return new Promise(done => {
    let t = 0;
    ticker.add(dt => {
      t += dt;
      const x = Math.min(Math.max((t - WAVE.lean) / WAVE.cross, 0), 1);
      const r = rMax * easeOutCubic(x);
      const lean = Math.min(t / WAVE.lean, 1);                          // old state leans toward the source…
      oldEl.style.transform = `translate(${(sx - W / 2) * 0.012 * lean}px,${(sy - H / 2) * 0.012 * lean}px) scale(${1 - 0.012 * lean})`;
      oldEl.style.filter = `blur(${(3 * x).toFixed(2)}px)`;             // …and blurs as the front nears
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
    });
  });
}
```

Usage. The source is the control that caused the change:

```ts
import { playWave } from './wave';
import { PAL } from './tokens';

button.addEventListener('click', async () => {
  const b = button.getBoundingClientRect(), h = card.getBoundingClientRect();
  await playWave({
    host: card, oldEl: stateBefore, newEl: stateAfter,
    src: [b.left + b.width / 2 - h.left, b.top + b.height / 2 - h.top],
    pal: PAL.failed, seed: 4.1,
  });
  commit();                                // swap data model only after the wave resolves
});
```

Rules, learned the hard way:

- `smoothstep(a, b, x)` with `a > b` is undefined in GLSL. Write `1. - smoothstep(low, high, x)`.
- Anything read from an angle uses whole harmonics (`sin(2a)`, `sin(3a)`), never `a * 0.3`, or a hard seam shows where the angle wraps.
- A glow whose *width* varies with noise creates hard edges deep inside the card. Vary strength, keep width fixed.
- Hold-then-clear reads as a snap. Every pixel focuses on its own clock from when the front passed it.
- Long thin shapes (a pill) need a narrower, weaker rim.
- The whole effect is a pure function of `t`.

---

## 11. Icons — SF Symbols

Apple's own. One family, one weight (`semibold` ≈ 1.75px stroke at 16pt). Outline at rest, `.fill` variant when hot (hot = white surface, so the filled glyph in `#0A0B1E` is crisp). Never mix.

| Meaning | Symbol |
|---|---|
| Orchestrator | `point.3.connected.trianglepath.dotted` |
| Session (OpenCode) | `terminal` |
| Working | `circle.dotted` |
| Idle | `pause` |
| Failed | `exclamationmark.triangle` |
| Done | `checkmark` |
| Kill | `xmark` |
| Re-prompt | `arrow.clockwise` |
| Logs | `text.alignleft` |
| Cleanup | `trash` |
| Focus herdr pane | `arrow.up.forward.square` |

### 11.1 Web path: Swift serves symbols as tint masks

The web layer cannot render SF Symbols natively. Swift rasterizes the symbol to a black-on-transparent PNG at 3× and serves it through a custom scheme. CSS paints it with `currentColor`, which is what makes the **white-hover flip work for free**.

```swift
// SymbolScheme.swift
import AppKit
import WebKit

final class SymbolScheme: NSObject, WKURLSchemeHandler {
    static let weights: [String: NSFont.Weight] = ["regular": .regular, "medium": .medium, "semibold": .semibold, "bold": .bold]

    func webView(_ w: WKWebView, start task: WKURLSchemeTask) {
        guard let url = task.request.url,
              let c = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let name = c.host?.removingPercentEncoding else { task.didFailWithError(URLError(.badURL)); return }
        let q = { (k: String) in c.queryItems?.first { $0.name == k }?.value }
        let pt = CGFloat(Double(q("pt") ?? "") ?? 16)
        let weight = Self.weights[q("w") ?? ""] ?? .semibold
        let cfg = NSImage.SymbolConfiguration(pointSize: pt, weight: weight)
        guard let img = NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(cfg),
              let png = img.maskPNG(scale: 3) else { task.didFailWithError(URLError(.fileDoesNotExist)); return }
        let res = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: [
            "Content-Type": "image/png", "Access-Control-Allow-Origin": "*", "Cache-Control": "max-age=31536000"])!
        task.didReceive(res); task.didReceive(png); task.didFinish()
    }
    func webView(_ w: WKWebView, stop task: WKURLSchemeTask) {}
}

extension NSImage {
    /// Alpha is all that matters; the web side tints it.
    func maskPNG(scale: CGFloat) -> Data? {
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale),
                                         bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                         colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        rep.size = size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        draw(in: NSRect(origin: .zero, size: size))
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])
    }
}
```

```css
.sym { display: inline-block; width: var(--pt, 16px); height: var(--pt, 16px); background: currentColor;
       -webkit-mask: var(--sym) center / contain no-repeat; mask: var(--sym) center / contain no-repeat; }
```

```ts
// sym.ts
export const sym = (name: string, pt = 16, w: 'regular' | 'medium' | 'semibold' | 'bold' = 'semibold') =>
  `<i class="sym" style="--pt:${pt}px;--sym:url('symbol://${name}?pt=${pt}&w=${w}')"></i>`;
```

If WebKit refuses the cross-scheme `mask-image` despite the CORS header, hand the bytes over the bridge as `data:` URLs instead. The CSS does not change.

### 11.2 Animated glyphs

Web cannot run `symbolEffect`. The three glyphs that animate (check, x, spinner) are hand-authored 24×24 strokes at `stroke-width: 1.75` so they match the symbol weight. They draw on, instead of fading in.

```ts
// draw.ts
import { EASE } from './tokens';

export const DRAW = { check: 'M5 12.5l4.5 4.5L19 7.5', x: 'M6 6l12 12M18 6L6 18' } as const;

export function drawOn(path: SVGPathElement, ms = 260) {
  const L = path.getTotalLength();
  path.style.strokeDasharray = `${L}`; path.style.strokeDashoffset = `${L}`;
  path.animate([{ strokeDashoffset: L }, { strokeDashoffset: 0 }], { duration: ms, easing: EASE.out, fill: 'forwards' });
}
```

### 11.3 SwiftUI path

If any surface goes native, this is the same contract: outline at rest, `.fill` and ink-dark when hot, a symbol effect on change.

```swift
struct ActionGlyph: View {
    let name: String
    let hot: Bool
    var body: some View {
        Image(systemName: name)
            .symbolVariant(hot ? .fill : .none)
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(hot ? Color(red: 0.039, green: 0.043, blue: 0.118) : Color.white.opacity(0.9))
            .contentTransition(.symbolEffect(.replace))
            .symbolEffect(.bounce, value: hot)
    }
}
```

SF Symbols 7 (macOS 26) added Draw On / Draw Off and variable draw. Use them natively for `checkmark` and `xmark` completion. I wrote the modifier names from memory, so confirm the exact signatures in Xcode before leaning on them.

License: SF Symbols are for apps on Apple platforms. Rendering them on-device through the scheme handler is fine. Do **not** export symbol paths to SVG and ship them in web content, a marketing site or a non-Apple build.

---

## 12. Motion catalogue

| Event | Motion | Time | Curve |
|---|---|---|---|
| Pointer enters shell | `rest → peek`, shell spring | ~320ms, 60ms intent delay | spring `open` |
| Click | `peek → open`, shell spring; cards rise 8px and fade in, 40ms stagger; glow level 0.5 → 1; haptic `.alignment` | 420ms | spring `open` + `EASE.out` |
| Pointer leaves | `→ rest` | 240ms delay, then spring | spring `open` |
| Row hover | white wipe from entry point; ink flips; actions slide 6px, 40ms stagger | 220 / 140ms | `EASE.out` |
| Row press | scale .985, stays white | 80ms | spring `press` |
| Count change | `NumberRoll`, per-digit spring plus velocity blur | ~400ms | spring `digit` |
| Heartbeat sample | field slides one column left, head column grows | ~320ms | spring `open` / `hover` |
| Load change | floor glow rises as `--lvl` | 900ms | `EASE.out` |
| Status → failed | orb colors shift to red, double-beat starts, dot goes hollow, haptic `.levelChange` | ~650ms | `damp` λ=7 |
| Status → done | rim → white, one bloom flash, then settles to idle halo | 480ms | `EASE.out` |
| Whole-card state change | wave (§10) | 2.07s | closed-form |
| Kill | hold white 80ms, row height collapses | 280ms | spring `open` |
| Working (ambient) | orb breathing | 3.2s loop | sine |
| Radar (ambient) | ring ping | 2.4s loop | `EASE.out` |
| IDs (ambient) | drift | 18–32s | ease-in-out alternate |
| Pointer move | specular follows with spring lag | continuous | spring `hover` |

`rest` has no ambient row. Nothing loops while the panel is closed.

---

## 13. Performance budget

This is always-on software in the menu bar. Battery is a design constraint, not an afterthought.

| Rule | Value |
|---|---|
| WebGL contexts | ≤ 2 (orb, wave). The wave context exists only while a wave plays. |
| DPR cap | 2 |
| `powerPreference` | `'low-power'` |
| Orb canvas | ≤ 600×700 CSS px |
| Orb fps | `open` 60 · `peek` 30 · `rest` 0 (one static render per status change) |
| `backdrop-filter` | ≤ 1 per surface, never on the black shell |
| Window resize | once per mode change, never per frame |
| Hidden or occluded | `ticker` sleeps on `document.hidden`; Swift also posts occlusion state |
| Allocation in render paths | none beyond the three tiny color arrays in `GlowOrb.frame` |
| Context loss | listen for `webglcontextlost`, `preventDefault()`, rebuild `Pass` on `webglcontextrestored` |

---

## 14. Rejection checklist

Review every change against this list. Any hit is a rewrite, not a patch.

1. Hover is gray, black, translucent, or a fade. It must be a pure `#FFFFFF` wipe with `#0A0B1E` ink.
2. Glow that encodes nothing. If you cannot say what it means, delete it.
3. A hue outside indigo → lilac, red, white.
4. A radius off the scale (8 · 16 · 28 · 36 · 999).
5. A drop shadow. Light only.
6. A third font, a weight outside 400 / 500 / 600, an uppercase label without mono and tracking.
7. Numerals that are not tabular, or a metric that jumps instead of rolling.
8. Linear easing anywhere except the marching-ants tether.
9. Overshoot above 2%, a "bouncy" spring, a shake on failure.
10. Any ambient loop running while closed.
11. `Math.random()` or `Date.now()` in a render path.
12. A mixed-weight icon set, a filled icon at rest, an emoji.
13. Copy that is friendly instead of informative: "Oops", "All good!", "Hang tight".
14. A new window, panel, or competing multiplexer. We request a herdr pane; we never open a window.
15. A wave used for something that is not a whole-card status change.
16. A light backdrop behind the shell. The top edge must be pure black.
17. A grid that is off the 4pt rhythm "because it looks better". It does not.
18. An effect that survives only at 120Hz on a plugged-in machine.

If the result could have come from a mediocre model in one pass, it fails. The standard is a UI that feels inevitable, controlled and sharp under scrutiny, with exactly one soft thing in it: the light, and the light is telling the truth.