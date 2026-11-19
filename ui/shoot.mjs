// shoot.mjs — headless screenshot driver. Never opens a window. Headless chromium only.
// Usage: node shoot.mjs [suffix]   → .proof/agents/web-ui/{rest,peek,open-<demo>}[-suffix].png
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
const outDir = join(here, '..', '.proof', 'agents', 'web-ui');
mkdirSync(outDir, { recursive: true });

const suffix = process.argv[2] ? `-${process.argv[2]}` : '';
const DEMOS = ['mixed', 'empty', 'failed', 'alldone', 'idle', 'big'];

const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.woff2': 'font/woff2', '.svg': 'image/svg+xml',
  '.png': 'image/png', '.json': 'application/json', '.map': 'application/json',
};

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    let p = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '');
    if (p === '' ) p = 'index.html';
    let file = join(dist, p);
    if (!file.startsWith(dist)) { res.writeHead(403); res.end(); return; }
    if (!existsSync(file)) {
      // SPA-ish fallback for query-only navigation
      file = join(dist, 'index.html');
    }
    const body = await readFile(file);
    res.writeHead(200, { 'content-type': MIME[extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
    res.end(body);
  } catch (err) {
    res.writeHead(500); res.end(String(err));
  }
});

await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const port = server.address().port;
console.log(`serving dist/ on 127.0.0.1:${port}`);

const browser = await chromium.launch({
  headless: true,
  args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--disable-gpu-sandbox'],
});
const pageErrors = [];
const shot = async (name) => {
  const p = join(outDir, `${name}${suffix}.png`);
  await p0.screenshot({ path: p });
  console.log('wrote', p);
};

const ctx = await browser.newContext({ viewport: { width: 1000, height: 760 }, deviceScaleFactor: 2, reducedMotion: 'no-preference' });
const p0 = await ctx.newPage();
p0.on('pageerror', (err) => pageErrors.push(String(err)));
p0.on('console', (msg) => { if (msg.type() === 'error') pageErrors.push(msg.text()); });

for (const demo of DEMOS) {
  await p0.goto(`http://127.0.0.1:${port}/?demo=${demo}`, { waitUntil: 'networkidle' });
  await p0.waitForFunction(() => window.__ready === true, { timeout: 5000 });
  await p0.waitForTimeout(900); // springs settle, heartbeat fills, eps walk ticks

  await shot(demo === 'mixed' ? 'rest' : `rest-${demo}`);

  // peek: hover the shell (top-center)
  await p0.mouse.move(500, 16);
  await p0.waitForTimeout(50);
  await p0.mouse.move(500, 16); // enter fires
  await p0.waitForTimeout(650);
  await shot(demo === 'mixed' ? 'peek' : `peek-${demo}`);

  // open: click
  await p0.mouse.click(500, 16);
  await p0.waitForTimeout(1100);
  await shot(`open-${demo}`);

  if (demo === 'mixed') {
    // mid-wave frame via the browser-preview wave hook
    await p0.evaluate(() => window.__waveTest?.());
    await p0.waitForTimeout(420);
    await shot('wave-mid-mixed');
    await p0.waitForTimeout(1900); // let the wave finish before further interaction
    // white-hot row: hover + logs drawer
    const row = p0.locator('.item').first();
    await row.hover();
    await p0.waitForTimeout(350);
    await shot('row-hot-mixed');
    await row.locator('button[aria-label="Logs"]').click();
    await p0.waitForTimeout(650);
    await shot('logs-open-mixed');
    await p0.keyboard.press('Escape');
    await p0.waitForTimeout(300);
  }
  if (demo === 'empty') {
    await p0.locator('.cmd-chip').click();
    await p0.waitForTimeout(250);
    await shot('chip-click-empty');
  }

  // park pointer away so the next demo starts clean
  await p0.mouse.move(500, 750);
  await p0.waitForTimeout(150);
}

await browser.close();
server.close();

if (pageErrors.length) {
  console.error('\nPAGE ERRORS:\n' + pageErrors.join('\n'));
  process.exit(2);
}
console.log('\nno page errors');
