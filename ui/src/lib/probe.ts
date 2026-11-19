// probe.ts — dev-only probes (?probe=1). Zero cost when disabled.
// Frame probe: rAF deltas, flags >16.7 ms frames.
// Interaction probe: keydown(Enter)/Cmd+K → acknowledged state paint, plus JS work per
// interaction (paint/layout excluded — measured from a microtask after the event's sync work).
export interface ProbeStats { avg: number; p95: number; max: number; jank: number; frames: number }
export type InteractionKind = 'submit' | 'focus';
export interface InteractionSample {
  kind: InteractionKind;
  keydownToPaintMs: number;
  jsWorkMs: number;
  at: number;
}

const JS_BUDGET_MS = 16.7;
const ACK_SELECTOR = '[data-optimistic-task], .composer-ok';

function isComposerTarget(t: EventTarget | null): boolean {
  return t instanceof Element && !!t.closest('.composer');
}

/** Install both probes. No-op unless ?probe=1. */
export function installProbe(): () => void {
  if (typeof window === 'undefined') return () => {};
  if (new URLSearchParams(window.location.search).get('probe') !== '1') return () => {};

  // ---------- frame probe ----------
  const CAP = 240;
  const ring = new Float64Array(CAP);
  let idx = 0, filled = 0, jank = 0;
  let last = performance.now();
  let raf = 0;

  const el = document.createElement('div');
  el.id = 'frame-probe';
  el.style.cssText = [
    'position:fixed', 'right:10px', 'bottom:10px', 'z-index:9999',
    'padding:6px 10px', 'border-radius:8px', 'border:1px solid rgba(255,255,255,.14)',
    'background:rgba(10,11,30,.82)', 'color:#eee',
    'font:400 10.5px/1.5 ui-monospace,Menlo,monospace',
    'letter-spacing:.08em', 'pointer-events:none', 'white-space:pre',
  ].join(';');
  el.textContent = 'probe: warming up…';
  document.body.append(el);

  // ---------- interaction probe ----------
  const samples: InteractionSample[] = [];
  type Pending = { t0: number; jsWorkMs: number; waited: boolean };
  const pending = new Map<InteractionKind, Pending>();

  const readout = (frameText: string) => {
    const line = (kind: InteractionKind) => {
      const s = samples.filter((x) => x.kind === kind);
      if (!s.length) return `${kind} —`;
      const lastS = s[s.length - 1]!;
      const worstJs = Math.max(...s.map((x) => x.jsWorkMs));
      return `${kind} ${lastS.keydownToPaintMs.toFixed(1)}ms paint / ${lastS.jsWorkMs.toFixed(1)}ms js (worst ${worstJs.toFixed(1)})`;
    };
    const js = samples.map((s) => s.jsWorkMs);
    const over = js.filter((v) => v > JS_BUDGET_MS).length;
    const verdict = !js.length ? 'warming' : over === 0 ? 'PASS' : `FAIL ${over}/${js.length}`;
    el.textContent = `${frameText}\ninteraction  ${line('submit')} · ${line('focus')}\njs work < ${JS_BUDGET_MS}ms  ${verdict}  (n=${js.length})`;
  };

  const finish = (kind: InteractionKind, p: Pending) => {
    if (p.waited) return;
    p.waited = true;
    // two rAFs: the first runs before the frame's paint, the second after it was committed
    requestAnimationFrame(() => requestAnimationFrame(() => {
      samples.push({ kind, keydownToPaintMs: performance.now() - p.t0, jsWorkMs: p.jsWorkMs, at: Date.now() });
      if (samples.length > 40) samples.shift();
      pending.delete(kind);
    }));
  };

  const ackNow = () => {
    const focus = pending.get('focus');
    if (focus && document.activeElement?.classList.contains('composer-input')) finish('focus', focus);
    const submit = pending.get('submit');
    if (submit && document.querySelector(ACK_SELECTOR)) finish('submit', submit);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const kind: InteractionKind | null =
      (e.key === 'Enter' && !e.shiftKey && isComposerTarget(e.target)) ? 'submit'
      : ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') ? 'focus'
      : null;
    if (!kind) return;
    const t0 = performance.now();
    pending.set(kind, { t0, jsWorkMs: 0, waited: false });
    // A macrotask runs after the event's full listener chain (root + window handlers).
    // React's discrete render is JS work and is included; browser paint/layout is not.
    window.setTimeout(() => {
      const p = pending.get(kind);
      if (p && !p.waited) p.jsWorkMs = performance.now() - t0;
    }, 0);
  };
  window.addEventListener('keydown', onKeyDown, true);
  const mo = new MutationObserver(ackNow);
  mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-status', 'class'] });
  window.addEventListener('focusin', ackNow, true);

  // Harness-facing readout. Dev only; absent in Tauri because installProbe() is gated.
  (window as unknown as { __interactionProbe?: unknown }).__interactionProbe = {
    samples: () => samples.map((s) => ({ ...s })),
    clear: () => { samples.length = 0; },
    budgetMs: JS_BUDGET_MS,
  };

  const renderFrame = () => {
    if (filled === 0) { readout('frame ms  warming up…'); return; }
    const slice = Array.from(ring.subarray(0, filled)).sort((a, b) => a - b);
    const avg = slice.reduce((a, b) => a + b, 0) / filled;
    const p95 = slice[Math.min(filled - 1, Math.floor(filled * 0.95))] ?? slice[filled - 1] ?? 0;
    const max = slice[filled - 1] ?? 0;
    readout(`frame ms  avg ${avg.toFixed(1)}  p95 ${p95.toFixed(1)}  max ${max.toFixed(1)}  jank ${jank}/${filled}`);
  };
  let lastRender = 0;
  const loop = (now: number) => {
    const dt = now - last; last = now;
    ring[idx % CAP] = dt; idx += 1; filled = Math.min(filled + 1, CAP);
    if (dt > 16.7) jank += 1;
    if (now - lastRender > 500) { renderFrame(); lastRender = now; }
    raf = requestAnimationFrame(loop);
  };
  raf = requestAnimationFrame(loop);

  return () => {
    cancelAnimationFrame(raf);
    window.removeEventListener('keydown', onKeyDown, true);
    window.removeEventListener('focusin', ackNow, true);
    mo.disconnect();
    el.remove();
  };
}
