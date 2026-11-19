// probe-run.mjs — headless interaction-probe harness. Serves dist/, drives the real dashboard,
// and writes { submit, focus } samples to the JSON path passed as argv[2].
import { createServer } from 'node:http';
import { readFile, writeFile } from 'node:fs/promises';
import { existsSync, mkdirSync } from 'node:fs';
import { extname, join, normalize, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const here = fileURLToPath(new URL('.', import.meta.url));
const dist = join(here, 'dist');
if (!existsSync(join(dist, 'index.html'))) { console.error('build first'); process.exit(1); }
const out = process.argv[2] ?? join(here, '..', '.proof', 'agents', 'ui-realtime', 'interaction-before.json');
mkdirSync(dirname(out), { recursive: true });

const MIME = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.woff2': 'font/woff2', '.svg': 'image/svg+xml' };
const server = createServer(async (req, res) => {
  try {
    const u = new URL(req.url ?? '/', 'http://127.0.0.1');
    let p = normalize(decodeURIComponent(u.pathname)).replace(/^[/\\]+/, '') || 'index.html';
    let f = join(dist, p);
    if (!f.startsWith(dist)) { res.writeHead(403); res.end(); return; }
    if (!existsSync(f)) f = join(dist, 'index.html');
    res.writeHead(200, { 'content-type': MIME[extname(f)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
    res.end(await readFile(f));
  } catch (e) { res.writeHead(500); res.end(String(e)); }
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const port = server.address().port;

const browser = await chromium.launch({ headless: true, args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--disable-gpu-sandbox'] });
const ctx = await browser.newContext({ viewport: { width: 1120, height: 760 }, deviceScaleFactor: 1, reducedMotion: 'no-preference' });
const page = await ctx.newPage();
const pageErrors = [];
page.on('pageerror', (e) => pageErrors.push(String(e) + '\n' + (e.stack ?? '')));
page.on('console', (m) => { if (m.type() === 'error') pageErrors.push(m.text()); });
await page.goto(`http://127.0.0.1:${port}/?surface=dashboard&probe=1&demo=mixed`, { waitUntil: 'networkidle' });
await page.waitForFunction(() => window.__ready === true && !!window.__interactionProbe, { timeout: 5000 });
await page.waitForTimeout(600);

// Cmd+K → acknowledged state paint (focus lands on the composer input)
await page.locator('.dash-brand').click();
await page.evaluate(() => window.__interactionProbe.clear());
await page.keyboard.press('Meta+k');
await page.waitForFunction(() => window.__interactionProbe.samples().some((s) => s.kind === 'focus'), { timeout: 3000 });

// Enter → acknowledged state paint (optimistic row / legacy assigned state)
const input = page.locator('.composer-input');
await input.fill('Probe submitted task');
await page.evaluate(() => window.__interactionProbe.clear());
await page.keyboard.press('Enter');
await page.waitForFunction(() => window.__interactionProbe.samples().some((s) => s.kind === 'submit'), { timeout: 3000 });
await page.waitForTimeout(250);

const result = await page.evaluate(() => ({
  samples: window.__interactionProbe.samples(),
  budgetMs: window.__interactionProbe.budgetMs,
  readout: document.querySelector('#frame-probe')?.textContent ?? '',
}));
result.pageErrors = pageErrors;
await writeFile(out, JSON.stringify(result, null, 2) + '\n');
console.log(JSON.stringify(result, null, 2));
await browser.close();
server.close();
if (pageErrors.length) process.exitCode = 2;
