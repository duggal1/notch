// capture-icon-theme.mjs — headless before/after evidence driver for the pixel-icon theme.
// Usage: node tools/capture-icon-theme.mjs <distDir> <outDir> <tag>
// Serves a static dist on an ephemeral port, drives the same deterministic views,
// and writes screenshots, an icon DOM probe and a full element-geometry census.
// Never opens a window, never touches the user's browser profile.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { extname, join, normalize, resolve } from 'node:path';
import { createRequire } from 'node:module';

// Playwright lives in ui/node_modules; resolve from that package, not this script.
const require = createRequire(new URL('../ui/package.json', import.meta.url));
const { chromium } = require('playwright');

const [distArg, outArg, tag = 'run'] = process.argv.slice(2);
if (!distArg || !outArg) {
  console.error('usage: node tools/capture-icon-theme.mjs <distDir> <outDir> <tag>');
  process.exit(1);
}
const dist = resolve(distArg);
const out = resolve(outArg);
mkdirSync(out, { recursive: true });
if (!existsSync(join(dist, 'index.html'))) { console.error('no index.html in', dist); process.exit(1); }

const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.woff2': 'font/woff2', '.svg': 'image/svg+xml',
  '.png': 'image/png', '.json': 'application/json',
};
const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    let p = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '');
    if (p === '') p = 'index.html';
    let file = join(dist, p);
    if (!file.startsWith(dist)) { res.writeHead(403); res.end(); return; }
    if (!existsSync(file)) file = join(dist, 'index.html');
    res.writeHead(200, {
      'content-type': MIME[extname(file)] ?? 'application/octet-stream',
      'cache-control': 'no-store',
    });
    res.end(await readFile(file));
  } catch (e) { res.writeHead(500); res.end(String(e)); }
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const port = server.address().port;

const browser = await chromium.launch({
  headless: true,
  args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--disable-gpu-sandbox'],
});
const errors = [];
const freeze = async (ctx) => {
  await ctx.addInitScript(() => {
    const FIXED = 1763600000000;
    Date.now = () => FIXED;
    let seed = 0x9e3779b9;
    Math.random = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 4294967296; };
    // Freeze demo data ticks: an un-frozen eps walk changes text between runs
    // and would pollute the glyph-only pixel diff. setTimeout still drives the
    // shell's rest/peek/open intent machine.
    window.setInterval = () => 0;
  });
};
const style = `
  *,*::before,*::after { animation: none !important; transition: none !important;
    caret-color: transparent !important; }
  canvas, .dither, .dither-field, [class*='dither'], .distortion-wave,
  [class*='wave-canvas'], .spec, .grain::before { visibility: hidden !important; }
`;

const geomProbe = () => {
  const pathOf = (el) => {
    const parts = [];
    let n = el;
    while (n && n.nodeType === 1 && parts.length < 12) {
      const p = n.parentElement;
      const idx = p ? [...p.children].indexOf(n) : 0;
      parts.unshift(`${n.tagName.toLowerCase()}${n.className && typeof n.className === 'string' ? '.' + n.className.trim().split(/\s+/).slice(0, 2).join('.') : ''}[${idx}]`);
      n = p;
    }
    return parts.join('>');
  };
  const els = [...document.querySelectorAll('body *')].filter((e) => {
    const cs = getComputedStyle(e);
    return cs.display !== 'none' && cs.visibility !== 'hidden';
  });
  const rects = els.map((e) => {
    const r = e.getBoundingClientRect();
    return { p: pathOf(e), x: +r.x.toFixed(2), y: +r.y.toFixed(2), w: +r.width.toFixed(2), h: +r.height.toFixed(2) };
  });
  const icons = [...document.querySelectorAll('i[data-icon], .sym, svg[aria-hidden="true"]')].map((e) => {
    const r = e.getBoundingClientRect();
    return {
      name: e.getAttribute('data-icon') ?? (typeof e.className === 'string' ? e.className : 'svg'),
      size: e.getAttribute('data-icon-size') ?? r.width,
      x: Math.floor(r.x), y: Math.floor(r.y), w: Math.ceil(r.width), h: Math.ceil(r.height),
    };
  });
  return { rects, icons };
};

const results = { tag, port, views: [], errors };

async function openPage(viewport, query) {
  const ctx = await browser.newContext({
    viewport, deviceScaleFactor: 1, reducedMotion: 'reduce',
  });
  await freeze(ctx);
  const p = await ctx.newPage();
  p.on('pageerror', (e) => errors.push(String(e)));
  p.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  await p.goto(`http://127.0.0.1:${port}/${query}`, { waitUntil: 'networkidle' });
  await p.waitForFunction(() => window.__ready === true, { timeout: 8000 });
  await p.addStyleTag({ content: style });
  await p.waitForTimeout(500);
  return { ctx, p };
}

async function view(name, viewport, query, act) {
  const { ctx, p } = await openPage(viewport, query);
  if (act) await act(p);
  await p.waitForTimeout(250);
  const shot = join(out, `${tag}-${name}.png`);
  await p.screenshot({ path: shot });
  const probe = await p.evaluate(geomProbe);
  writeFileSync(join(out, `${tag}-${name}.json`), JSON.stringify(probe, null, 2));
  results.views.push({ name, shot, probe: `${tag}-${name}.json`, viewport, icons: probe.icons.length });
  console.log(`shot ${name}: ${probe.icons.length} icons`);
  await ctx.close();
}

// Popover at 1120x720 — the spec viewport. Pointer drives rest → peek → open.
await view('popover-rest', { width: 1120, height: 720 }, '?demo=mixed', async (p) => {
  await p.mouse.move(560, 700);
});
await view('popover-peek', { width: 1120, height: 720 }, '?demo=mixed', async (p) => {
  await p.mouse.move(560, 16);
  await p.waitForTimeout(120);
  await p.mouse.move(560, 16);
  await p.waitForTimeout(700);
});
await view('popover-open', { width: 1120, height: 720 }, '?demo=mixed', async (p) => {
  await p.locator('.shell').focus();
  await p.keyboard.press('Enter');
  await p.waitForTimeout(100);
  if (await p.locator('.shell').getAttribute('data-mode') !== 'open') {
    await p.locator('.shell').dispatchEvent('pointerdown');
    await p.waitForTimeout(100);
  }
  await p.waitForTimeout(800);
  const mode = await p.locator('.shell').getAttribute('data-mode');
  if (mode !== 'open') throw new Error(`popover-open: shell did not open (mode=${mode})`);
});

// Dashboard tabs, 1120x720.
for (const [name, tab] of [
  ['dashboard-tree', null],
  ['dashboard-mux', 'Mux'],
  ['dashboard-comms', 'Comms'],
  ['dashboard-escalations', 'Alarms'],
]) {
  await view(name, { width: 1120, height: 720 }, '?surface=dashboard&demo=mixed', async (p) => {
    if (tab) { await p.getByRole('tab', { name: tab }).click(); await p.waitForTimeout(350); }
    await p.mouse.move(10, 700);
  });
}

await browser.close();
server.close();
writeFileSync(join(out, `${tag}-index.json`), JSON.stringify(results, null, 2));
if (errors.length) {
  console.error('PAGE ERRORS:\n' + errors.join('\n'));
  process.exit(2);
}
console.log('no page errors');
