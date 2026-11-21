// dash-shot.mjs — headless dashboard screenshots for the design pass. Never opens a window.
// Usage: node dash-shot.mjs [suffix]
// Writes .proof/design/dashboard-<tab>[-<demo>][-suffix].png
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync, mkdirSync } from 'node:fs';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const here = fileURLToPath(new URL('.', import.meta.url));
const dist = join(here, 'dist');
if (!existsSync(join(dist, 'index.html'))) {
  console.error('dist/index.html missing — run `npm run build` first');
  process.exit(1);
}
const outDir = join(here, '..', '.proof', 'design');
mkdirSync(outDir, { recursive: true });
const suffix = process.argv[2] ? `-${process.argv[2]}` : '';

const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.woff2': 'font/woff2', '.svg': 'image/svg+xml',
  '.png': 'image/png', '.json': 'application/json', '.map': 'application/json',
};
const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    let p = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '');
    if (p === '') p = 'index.html';
    let file = join(dist, p);
    if (!file.startsWith(dist)) { res.writeHead(403); res.end(); return; }
    if (!existsSync(file)) file = join(dist, 'index.html');
    const body = await readFile(file);
    res.writeHead(200, { 'content-type': MIME[extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
    res.end(body);
  } catch (err) { res.writeHead(500); res.end(String(err)); }
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const port = server.address().port;
console.log(`serving dist/ on 127.0.0.1:${port}`);

const browser = await chromium.launch({
  headless: true,
  args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--disable-gpu-sandbox'],
});
const errors = [];
const W = 1280, H = 800;

async function open(demo) {
  const c = await browser.newContext({ viewport: { width: W, height: H }, deviceScaleFactor: 2, reducedMotion: 'no-preference' });
  const p = await c.newPage();
  p.on('pageerror', (e) => errors.push(String(e)));
  p.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  await p.goto(`http://127.0.0.1:${port}/?surface=dashboard&demo=${demo}`, { waitUntil: 'networkidle' });
  await p.waitForFunction(() => window.__ready === true, { timeout: 5000 });
  await p.waitForTimeout(1100);
  return { c, p };
}

const SHOTS = [
  { name: 'tree', demo: 'mixed' },
  { name: 'tree-empty', demo: 'empty' },
  { name: 'tree-big', demo: 'big' },
  { name: 'tree-idle', demo: 'idle' },
  { name: 'mux', demo: 'mixed', tab: 'Mux' },
  { name: 'mux-empty', demo: 'empty', tab: 'Mux' },
  { name: 'comms', demo: 'mixed', tab: 'Comms' },
  { name: 'escalations', demo: 'mixed', tab: 'Alarms' },
  { name: 'escalations-empty', demo: 'empty', tab: 'Alarms' },
];

for (const s of SHOTS) {
  const { c, p } = await open(s.demo);
  if (s.tab) {
    await p.getByRole('tab', { name: s.tab }).click();
    await p.waitForTimeout(700);
  }
  const path = join(outDir, `dashboard-${s.name}${suffix}.png`);
  await p.screenshot({ path });
  console.log('wrote', path);
  await c.close();
}

// state coverage on the tree: supervisor selected, model menu open, composer focus
{
  const { c, p } = await open('mixed');
  await p.locator('[data-sv="SV-2"]').first().click();
  await p.waitForTimeout(600);
  await p.screenshot({ path: join(outDir, `dashboard-tree-supervisor${suffix}.png`) });
  await p.locator('button:has-text("Done when")').click();
  await p.waitForTimeout(300);
  await p.locator('textarea').fill('Ship the pixel pass');
  await p.waitForTimeout(300);
  await p.screenshot({ path: join(outDir, `dashboard-tree-compose${suffix}.png`) });
  await p.locator('.model-btn').click();
  await p.waitForTimeout(300);
  await p.screenshot({ path: join(outDir, `dashboard-tree-model${suffix}.png`) });
  await p.keyboard.press('Escape');
  await p.locator('button.killswitch').click();
  await p.waitForTimeout(250);
  await p.screenshot({ path: join(outDir, `dashboard-tree-armed${suffix}.png`) });
  await c.close();
  console.log('wrote tree states');
}

// narrow widths — 1024 and 900 must not clip or overflow
for (const vp of [{ w: 1024, h: 720 }, { w: 900, h: 700 }]) {
  const c = await browser.newContext({ viewport: { width: vp.w, height: vp.h }, deviceScaleFactor: 2, reducedMotion: 'no-preference' });
  const p = await c.newPage();
  p.on('pageerror', (e) => errors.push(String(e)));
  p.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  await p.goto(`http://127.0.0.1:${port}/?surface=dashboard&demo=mixed`, { waitUntil: 'networkidle' });
  await p.waitForFunction(() => window.__ready === true, { timeout: 5000 });
  await p.waitForTimeout(1000);
  await p.screenshot({ path: join(outDir, `dashboard-${vp.w}-tree${suffix}.png`) });
  await p.getByRole('tab', { name: 'Mux' }).click();
  await p.waitForTimeout(600);
  await p.screenshot({ path: join(outDir, `dashboard-${vp.w}-mux${suffix}.png`) });
  const overflow = await p.evaluate(() => ({
    docW: document.documentElement.scrollWidth,
    winW: window.innerWidth,
    docH: document.documentElement.scrollHeight,
    winH: window.innerHeight,
  }));
  console.log(vp.w, JSON.stringify(overflow));
  await c.close();
}

await browser.close();
server.close();
if (errors.length) { console.error('\nPAGE ERRORS:\n' + errors.join('\n')); process.exit(2); }
console.log('\nno page errors');
