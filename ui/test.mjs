// test.mjs — unit-ish checks for the pure surface. Runs the real modules, not copies.
import { execFileSync } from 'node:child_process';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { mkdirSync } from 'node:fs';
import assert from 'node:assert/strict';

const here = dirname(fileURLToPath(import.meta.url));
mkdirSync(join(here, 'dist'), { recursive: true });

execFileSync(join(here, 'node_modules', '.bin', 'esbuild'), [
  'src/testkit.ts', '--bundle', '--format=esm', '--outfile=dist/testkit.mjs', '--log-level=error',
], { cwd: here, stdio: 'inherit' });

const t = await import(pathToFileURL(join(here, 'dist', 'testkit.mjs')).href + '?run=' + Date.now());
let n = 0;
const ok = (name) => { n += 1; console.log(`ok ${String(n).padStart(2, '0')} — ${name}`); };

// 1. spring open settles with ≤ 2% overshoot (target moved, like a mode change)
{
  const sp = new t.Spring(320, t.SPRING.open);
  sp.target = 560;
  let max = 0, settledAt = -1;
  for (let i = 0; i < 240 * 5 && settledAt < 0; i++) {
    const x = sp.step(1 / 240);
    max = Math.max(max, x);
    if (sp.settled) settledAt = i / 240;
  }
  assert.ok(settledAt > 0 && settledAt < 2, `spring settles in ${settledAt}s`);
  assert.ok((max - 560) / 240 <= 0.02, `overshoot ${(((max - 560) / 240) * 100).toFixed(2)}% of travel ≤ 2%`);
  assert.ok(Math.abs(sp.x - 560) < 0.001);
  ok(`spring open settles in ${settledAt.toFixed(2)}s, overshoot ${(((max - 560) / 240) * 100).toFixed(2)}%`);
}

// 2. notchPath: closed, left ear concave arc, ears + continuous corners
{
  const p = t.notchPath(320, 32, 12, 10);
  assert.ok(p.startsWith('M0 0'), 'starts at origin');
  assert.ok(p.endsWith('Z'), 'closed');
  assert.ok(p.includes('A10 10 0 0 1 10 10'), 'left ear arc present (concave)');
  assert.ok(p.includes('A10 10 0 0 1 320 0'), 'right ear arc present');
  assert.ok((p.match(/C/g) ?? []).length === 2, 'two continuous-corner cubics');
  assert.ok(p.includes('H298'), 'bottom spans to w - e - r');
  const open = t.notchPath(560, 640, 36, 12);
  assert.ok(open.includes('A12 12 0 0 1 12 12'), 'open shell ear');
  assert.ok(open.includes('V604'), 'open shell left edge ends above bottom corners');
  ok('notchPath geometry: closed path, ears, cubics');
}

// 3. frameFor modes
{
  const n = { w: 200, h: 32 };
  assert.deepEqual(t.frameFor('rest', 640, n), { w: 320, h: 32, r: 12, e: 10 });
  assert.deepEqual(t.frameFor('peek', 640, n), { w: 440, h: 76, r: 20, e: 10 });
  assert.deepEqual(t.frameFor('open', 640, n), { w: 560, h: 640, r: 36, e: 12 });
  ok('frameFor rest/peek/open');
}

// 4. floating ids determinism + bounds
{
  const ids = Array.from({ length: 12 }, (_, i) => ({ id: `S${(i % 3) + 1}-OC${String(i + 1).padStart(2, '0')}`, live: i % 3 === 0 }));
  const a = t.computeFloatingIds(ids, 7);
  const b = t.computeFloatingIds(ids, 7);
  const c = t.computeFloatingIds(ids, 8);
  assert.deepEqual(a, b, 'same seed → same layout');
  assert.notDeepEqual(a.map((x) => x.left + x.top), c.map((x) => x.left + x.top), 'different seed → different layout');
  for (const f of a) {
    assert.ok(f.left >= 0 && f.left <= 88 && f.top >= 0 && f.top <= 86);
    assert.ok(f.o >= 0.07 && f.o <= 0.19);
    assert.ok(f.d >= 18 && f.d <= 32);
  }
  ok('floatingIds deterministic, seeded, in-bounds');
}

// 5. topStatus worst-wins
{
  const mk = (st) => ({ latencyMs: null, failureReason: null, id: st, sup: 'SV-1', status: st, title: '', task: '', cwd: '', paneId: '', startedAt: 0, lastAction: '', lastRevision: 0 });
  const sessions = (...sts) => sts.map((s) => mk(s));
  assert.equal(t.topStatus([]), 'idle');
  assert.equal(t.topStatus(sessions('idle')), 'idle');
  assert.equal(t.topStatus(sessions('idle', 'working')), 'working');
  assert.equal(t.topStatus(sessions('working', 'failed')), 'failed');
  assert.equal(t.topStatus(sessions('done', 'done')), 'done');
  assert.equal(t.topStatus(sessions('idle', 'done')), 'idle');
  ok('topStatus worst-wins (failed > working > done > idle)');
}

// 6. drift derivation honesty
{
  const mk = (lat) => ({ id: String(lat), sup: 'SV-1', status: 'working', title: '', task: '', cwd: '', paneId: '', startedAt: 0, lastAction: '', lastRevision: 0, latencyMs: lat, failureReason: null });
  const empty = t.drift([]);
  assert.equal(empty.n, 0); assert.equal(empty.pct, null);
  const one = t.drift([mk(800)]);
  assert.equal(one.n, 1); assert.equal(one.pct, null, 'one sample is not a drift percentage');
  const d = t.drift([mk(800), mk(900)]);
  assert.equal(d.n, 2);
  assert.equal(d.best, 800); assert.equal(d.worst, 900);
  assert.ok(Math.abs(d.pct - 12.5) < 1e-9, `pct = ${d.pct}`);
  ok('drift: insufficient → no invented number; otherwise exact');
}

// 7. fmtElapsed
{
  assert.equal(t.fmtElapsed(1000, 1000), '00:00');
  assert.equal(t.fmtElapsed(0, 134 * 1000), '02:14');
  assert.equal(t.fmtElapsed(0, 3661 * 1000), '1:01:01');
  assert.equal(t.fmtElapsed(100, 0), '00:00', 'never negative');
  ok('fmtElapsed mm:ss / h:mm:ss, clamped');
}

// 8. normalizeEps clamps to 0..1 against 40 ev/s full scale
{
  assert.equal(t.normalizeEps(-5), 0);
  assert.equal(t.normalizeEps(0), 0);
  assert.equal(t.normalizeEps(20), 0.5);
  assert.equal(t.normalizeEps(80), 1);
  ok('normalizeEps clamped');
}

// 9. failureLabel maps bridge reasons to words; never a bare code pretending to be words
{
  assert.equal(t.failureLabel('stuck'), 'STUCK — WATCHDOG');
  assert.equal(t.failureLabel('spawn_failed'), 'SPAWN FAILED');
  assert.equal(t.failureLabel('blocked_waiting'), 'BLOCKED — WAITING ON INPUT');
  assert.equal(t.failureLabel(null), 'FAILED');
  assert.equal(t.failureLabel('some_new_code'), 'SOME NEW CODE');
  ok('failureLabel honest mapping');
}

// 10. mulberry32 seeded determinism
{
  const a = t.mulberry32(7), b = t.mulberry32(7);
  for (let i = 0; i < 100; i++) assert.equal(a(), b());
  ok('mulberry32 deterministic');
}

console.log(`\n${n}/10 checks passed`);
