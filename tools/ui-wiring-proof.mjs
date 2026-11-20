#!/usr/bin/env node
// ui-wiring-proof.mjs — headless proof for the notch fleet-stream wiring.
// One job: serve ui/dist, drive the real dashboard in Playwright, capture deterministic
// before/after screenshots, and probe row-click → focus + WS `fleet` frames. JSON out,
// nonzero exit on failure. Never opens a window, never touches the user's browser.
//
// Usage: node tools/ui-wiring-proof.mjs baseline|red|verify
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
// Playwright lives in ui/node_modules (the UI owns the browser dependency).
const { chromium } = await import(new URL('../ui/node_modules/playwright/index.mjs', import.meta.url).href);

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const DIST = process.env.UI_DIST ? join(process.env.UI_DIST) : join(ROOT, 'ui', 'dist');
const OUT = join(ROOT, '.proof', 'agents', 'ui-wiring');
const MODE = process.argv[2] ?? 'verify';
const FIXED_NOW = 1763520000000; // 2026-01-08T… deterministic for every capture

const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.woff2': 'font/woff2', '.svg': 'image/svg+xml',
  '.json': 'application/json', '.png': 'image/png',
};

function serveDist() {
  if (!existsSync(join(DIST, 'index.html'))) throw new Error('ui/dist missing — run npm run build');
  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? '/', 'http://127.0.0.1');
      let p = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '');
      if (!p) p = 'index.html';
      let file = join(DIST, p);
      if (!file.startsWith(DIST)) { res.writeHead(403).end(); return; }
      if (!existsSync(file) || !readFileSync(file).length) file = join(DIST, 'index.html');
      res.writeHead(200, { 'content-type': MIME[extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
      res.end(readFileSync(file));
    } catch (err) { res.writeHead(500).end(String(err)); }
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server)));
}

// Minimal text-frame WebSocket server — only what the browser fleet client needs.
function startMockWs(token) {
  const sockets = new Set();
  const server = createServer();
  let connections = 0;
  server.on('upgrade', (req, socket) => {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    if (url.searchParams.get('token') !== token) { socket.destroy(); return; }
    const key = String(req.headers['sec-websocket-key'] ?? '');
    const accept = createHash('sha1').update(key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
    socket.write('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + '\r\n\r\n');
    sockets.add(socket);
    connections += 1;
    socket.on('close', () => sockets.delete(socket));
    socket.on('error', () => sockets.delete(socket));
    socket.resume();
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({
    port: server.address().port,
    connections: () => connections,
    send(obj) {
      const payload = Buffer.from(JSON.stringify(obj), 'utf8');
      const n = payload.length;
      let head;
      if (n < 126) head = Buffer.from([0x81, n]);
      else if (n < 65536) { head = Buffer.alloc(4); head[0] = 0x81; head[1] = 126; head.writeUInt16BE(n, 2); }
      else { head = Buffer.alloc(10); head[0] = 0x81; head[1] = 127; head.writeBigUInt64BE(BigInt(n), 2); }
      for (const s of sockets) s.write(Buffer.concat([head, payload]));
    },
    close: () => { for (const s of sockets) s.destroy(); server.close(); },
  })));
}

// Freeze wall-clock and the 240 ms demo tick so two independent page loads are pixel-equal.
function freezeInitScript() {
  // literal, not a module constant — this function is serialized into the page
  Date.now = () => 1763520000000;
  // Kill every CSS animation/transition so two independent loads land on the same frame.
  const style = document.createElement('style');
  style.textContent = '*,*::before,*::after{animation:none!important;animation-delay:0s!important}';
  const host = document.head ?? document.documentElement;
  if (host) host.appendChild(style);
  else document.addEventListener('DOMContentLoaded', () => document.head.appendChild(style), { once: true });
  const intervals = [];
  window.__intervals = intervals;
  window.setInterval = (fn, ms, ...args) => {
    const id = intervals.length;
    intervals.push({ fn, ms, args, id, cancelled: false });
    return id;
  };
  window.clearInterval = (id) => { if (intervals[id]) intervals[id].cancelled = true; };
  window.__fire = (ms, times = 1) => {
    for (let t = 0; t < times; t += 1)
      for (const it of intervals) if (!it.cancelled && it.ms === ms) it.fn(...it.args);
  };
}

function probeInitScript() {
  window.__notchActions = [];
  window.__fleetEvents = [];
  window.addEventListener('fleet', (e) => window.__fleetEvents.push(e.detail));
}

async function settle(page) {
  await page.evaluate(async () => { await document.fonts.ready; });
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  await page.waitForTimeout(500);
}

async function openDashboard(browser, port, { demo = 'mixed', wsPort, token, probe = false, frozen = false } = {}) {
  const ctx = await browser.newContext({
    viewport: { width: 1120, height: 720 },
    deviceScaleFactor: 1,
    reducedMotion: 'reduce',
  });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push('pageerror: ' + String(e)));
  page.on('console', (m) => { if (m.type() === 'error') errors.push('console: ' + m.text()); });
  if (frozen) await page.addInitScript(freezeInitScript);
  if (probe) await page.addInitScript(probeInitScript);
  let url = `http://127.0.0.1:${port}/?surface=dashboard`;
  if (demo) url += `&demo=${demo}`;
  if (wsPort) url += `&wsPort=${wsPort}`;
  if (token) url += `&token=${token}`;
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => window.__ready === true, { timeout: 10000 });
  return { ctx, page, errors, url };
}

async function capture(browser, port, tab, file) {
  const { ctx, page, errors, url } = await openDashboard(browser, port, { frozen: true });
  if (tab === 'mux') {
    await page.getByRole('tab', { name: 'Mux' }).click();
    await page.waitForSelector('.mux-card');
  }
  await page.evaluate(() => window.__fire(240, 2));
  await settle(page);
  await page.screenshot({ path: file });
  await ctx.close();
  return errors;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const actions = (page) => page.evaluate(() => (window.__notchActions ?? []).slice());
const fleetEvents = (page) => page.evaluate(() => (window.__fleetEvents ?? []).length);
const focusActions = async (page) => (await actions(page)).filter((a) => a.cmd === 'focus').map((a) => a.id);

function assert(cond, msg, results, label) {
  results.push({ check: label, pass: !!cond, detail: msg });
  if (!cond) console.error('FAIL', label, '—', msg);
  else console.log('PASS', label, '—', msg);
}

async function runFocusProbe(browser, port, results, { requireFocus = true } = {}) {
  const { ctx, page, errors } = await openDashboard(browser, port, { probe: true });
  try {
    await settle(page);
    assert(await page.locator('.sup-row').count() > 0, 'dashboard shows supervisors', results, 'render');

    // 1. supervisor row primary click → select + focus its first working session
    await page.locator('[data-sv="SV-1"]').first().click();
    await settle(page);
    let ids = await focusActions(page);
    assert(ids.includes('S1-OC01'), `supervisor row focus ids=${JSON.stringify(ids)}`, results, 'supervisor-row focus');

    // 2. inspector session row primary click → focus that session
    const row = page.locator('.inspector .inspector-session-row').first();
    await row.click();
    await settle(page);
    ids = await focusActions(page);
    assert(ids.length >= 2, `inspector row added a focus (ids=${JSON.stringify(ids)})`, results, 'inspector-row focus');

    // 3. explicit "Herdr" button still works (regression)
    const before = ids.length;
    await page.locator('button[title="Open this session in herdr"]').click();
    await settle(page);
    ids = await focusActions(page);
    assert(ids.length === before + 1, `Herdr button still sends focus (${before} → ${ids.length})`, results, 'explicit Herdr button');

    // 4. multiplexer card primary click → select (jump to tree) + focus
    await page.getByRole('tab', { name: 'Mux' }).click();
    await page.waitForSelector('.mux-card');
    await page.locator('.mux-card').first().click();
    await settle(page);
    ids = await focusActions(page);
    assert(ids.length >= before + 2, `mux card added a focus (total ${ids.length})`, results, 'mux-card focus');
    assert(await page.locator('.tree-grid').count() === 1, 'mux click still jumps to Tree', results, 'mux selection preserved');

    assert(errors.length === 0, `page errors: ${JSON.stringify(errors)}`, results, 'focus probe page errors');
  } finally { await ctx.close(); }
  return { expected: requireFocus };
}

async function runFleetProbe(browser, port, results) {
  const ws = await startMockWs('probe-token');
  const { ctx, page, errors, url } = await openDashboard(browser, port, {
    demo: '', wsPort: ws.port, token: 'probe-token', probe: true,
  });
  try {
    await page.waitForFunction(() => window.__intervals !== undefined || window.__fleetEvents !== undefined, { timeout: 5000 });
    for (let i = 0; i < 50 && ws.connections() === 0; i += 1) await sleep(20);
    assert(ws.connections() > 0, `mock WS accepted a connection (${ws.connections()})`, results, 'ws connect');

    const snap = {
      version: 1, generatedAt: FIXED_NOW,
      orchestrator: { id: 'ORCH', status: 'working' },
      supervisors: [{ id: 'SV-1', status: 'working', working: 1, idle: 0, failed: 0, done: 0 }],
      sessions: [{ id: 'S1-OC01', sup: 'SV-1', status: 'working', title: 'Fleet streamed session', task: 'Fleet streamed session', cwd: '/tmp', paneId: 'w80:p1', startedAt: FIXED_NOW - 60000, lastAction: 'live', lastRevision: 1, latencyMs: 100, failureReason: null }],
      eps: 4, conflicts: [], attribution: [], warnings: [],
    };
    ws.send({ event: 'hello', version: 1, pid: 1, snapshot: snap });
    await page.waitForFunction(() => document.body.innerText.includes('Fleet streamed session'), { timeout: 5000 });
    assert(await fleetEvents(page) >= 1, 'hello frame emits a fleet event', results, 'fleet hello');

    const next = JSON.parse(JSON.stringify(snap));
    next.sessions[0].title = 'Fleet streamed session updated';
    next.generatedAt += 100;
    ws.send({ event: 'snapshot', snapshot: next, lag: 0 });
    await page.waitForFunction(() => document.body.innerText.includes('Fleet streamed session updated'), { timeout: 5000 });
    assert(await fleetEvents(page) >= 2, 'snapshot frame emits a second fleet event', results, 'fleet snapshot');

    // The daemon's additive `fleet` frame carries a units list, not a §1.3 snapshot.
    // The transport adapts it onto the last snapshot so the existing UI just updates.
    ws.send({
      event: 'fleet',
      fleet: {
        topStatus: 'failed',
        units: [
          { id: 'ORCH', kind: 'orchestrator', status: 'failed', topStatus: 'failed' },
          { id: 'SV-1', kind: 'supervisor', status: 'failed', topStatus: 'failed' },
          { id: 'S1-OC01', kind: 'session', status: 'failed', topStatus: 'failed', lastAction: 'fleet delta', revision: 99, paneId: 'w80:p9' },
        ],
      },
    });
    await page.waitForFunction(() => document.querySelector('.rail-live')?.textContent === 'failed', { timeout: 5000 });
    assert(await fleetEvents(page) >= 3, 'units fleet frame emits a fleet event', results, 'fleet units frame');
    assert((await page.locator('.inspector-head').innerText()).includes('failed'), 'unit status reached the inspector', results, 'fleet status adapted');
    assert(await page.locator('.tree-grid').count() === 1, 'fleet frames do not change the layout', results, 'fleet layout unchanged');
    assert(errors.length === 0, `page errors: ${JSON.stringify(errors)}`, results, 'fleet probe page errors');
  } finally { await ctx.close(); ws.close(); }
}

function pixelDiff(a, b) {
  const py = `
from PIL import Image, ImageChops
import json,sys
a=Image.open(sys.argv[1]).convert('RGB'); b=Image.open(sys.argv[2]).convert('RGB')
print(json.dumps({'size_a':list(a.size),'size_b':list(b.size),'identical':a.size==b.size and ImageChops.difference(a,b).getbbox() is None}))
`;
  return JSON.parse(execFileSync('python3', ['-c', py, a, b], { encoding: 'utf8' }).trim());
}

async function main() {
  mkdirSync(OUT, { recursive: true });
  const server = await serveDist();
  const port = server.address().port;
  const browser = await chromium.launch({
    headless: true,
    args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--disable-gpu-sandbox'],
  });
  const results = [];
  try {
    if (MODE === 'baseline') {
      const e1 = await capture(browser, port, 'tree', join(OUT, 'before-tree.png'));
      const e2 = await capture(browser, port, 'mux', join(OUT, 'before-mux.png'));
      const errors = [...e1, ...e2];
      writeFileSync(join(OUT, 'baseline.json'), JSON.stringify({ errors, shots: ['before-tree.png', 'before-mux.png'] }, null, 2));
      if (errors.length) { console.error(errors.join('\n')); process.exitCode = 2; }
      console.log('baseline captured at', port);
      return;
    }

    if (MODE === 'red') {
      await runFocusProbe(browser, port, results, { requireFocus: false });
      const failed = results.filter((r) => !r.pass);
      writeFileSync(join(OUT, 'red.json'), JSON.stringify(results, null, 2));
      if (!failed.length) { console.error('RED TEST PASSED UNEXPECTEDLY — behavior already present'); process.exitCode = 3; }
      else console.error(`RED: ${failed.length}/${results.length} checks failed as expected`);
      return;
    }

    // verify
    const e1 = await capture(browser, port, 'tree', join(OUT, 'after-tree.png'));
    const e2 = await capture(browser, port, 'mux', join(OUT, 'after-mux.png'));
    await runFocusProbe(browser, port, results);
    await runFleetProbe(browser, port, results);
    const diffTree = pixelDiff(join(OUT, 'before-tree.png'), join(OUT, 'after-tree.png'));
    const diffMux = pixelDiff(join(OUT, 'before-mux.png'), join(OUT, 'after-mux.png'));
    assert(diffTree.identical, `tree pixel diff ${JSON.stringify(diffTree)}`, results, 'tree pixel-identical');
    assert(diffMux.identical, `mux pixel diff ${JSON.stringify(diffMux)}`, results, 'mux pixel-identical');
    assert(e1.length === 0 && e2.length === 0, `capture page errors ${JSON.stringify([...e1, ...e2])}`, results, 'capture page errors');
    writeFileSync(join(OUT, 'probe.json'), JSON.stringify({ results, diffTree, diffMux }, null, 2));
    const failed = results.filter((r) => !r.pass);
    if (failed.length) { console.error(`${failed.length}/${results.length} checks FAILED`); process.exitCode = 1; }
    else console.log(`\nALL ${results.length} CHECKS PASSED`);
  } finally {
    await browser.close();
    server.close();
  }
}
main().catch((e) => { console.error(e); process.exit(1); });
