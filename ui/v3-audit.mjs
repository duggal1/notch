// v3-audit.mjs — headless design + wiring audit. Never opens a window.
// Usage: node v3-audit.mjs <pass-name>
// Writes .proof/v3/<pass>/*.png and .proof/v3/<pass>-checks.json
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const here = fileURLToPath(new URL('.', import.meta.url));
const root = join(here, '..');
// The machine wipes the global Playwright cache; keep the headless browser inside .proof/.
process.env.PLAYWRIGHT_BROWSERS_PATH ??= join(root, '.proof', 'pw-browsers');
const dist = join(here, 'dist');
if (!existsSync(join(dist, 'index.html'))) { console.error('build first'); process.exit(1); }
const pass = process.argv[2] ?? 'p1';
const out = join(root, '.proof', 'v3', pass);
mkdirSync(out, { recursive: true });

const MIME = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.woff2': 'font/woff2', '.svg': 'image/svg+xml', '.png': 'image/png' };
const server = createServer(async (req, res) => {
  try {
    const u = new URL(req.url ?? '/', 'http://127.0.0.1');
    let p = normalize(decodeURIComponent(u.pathname)).replace(/^([/\\])+/, '');
    if (!p) p = 'index.html';
    let f = join(dist, p);
    if (!f.startsWith(dist)) { res.writeHead(403); res.end(); return; }
    if (!existsSync(f)) f = join(dist, 'index.html');
    const b = await readFile(f);
    res.writeHead(200, { 'content-type': MIME[extname(f)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
    res.end(b);
  } catch (e) { res.writeHead(500); res.end(String(e)); }
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const port = server.address().port;
const executablePath = join(root, '.proof', 'pw-browsers', 'chromium_headless_shell-1255',
  'chrome-headless-shell-mac-arm64', 'chrome-headless-shell');
const launch = () => chromium.launch({ headless: true, executablePath,
  args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--disable-gpu-sandbox'] });
let browser = await launch();

const results = {};
const errors = [];
const problems = (r) => { if (r.problems.length) errors.push(`${r.name}: ${r.problems.join(' | ')}`); };

async function context(vp = { width: 1280, height: 800 }, dsf = 1) {
  const c = await browser.newContext({ viewport: vp, deviceScaleFactor: dsf, reducedMotion: 'no-preference' });
  const p = await c.newPage();
  p.on('pageerror', (e) => errors.push(`pageerror: ${e}`));
  p.on('console', (m) => { if (m.type() === 'error') errors.push(`console: ${m.text()}`); });
  await p.addInitScript(() => { window.__notchActions = []; });
  return { c, p };
}

async function open(p, query) {
  await p.goto(`http://127.0.0.1:${port}/${query}`, { waitUntil: 'networkidle' });
  await p.waitForFunction(() => window.__ready === true, { timeout: 8000 });
  await p.waitForTimeout(900);
}

const AUDIT = `(() => {
  const problems = [];
  const all = [...document.querySelectorAll('*')];
  // 0 borders / rings / outlines / hr
  for (const el of all) {
    const s = getComputedStyle(el);
    for (const side of ['Top','Right','Bottom','Left'])
      if (parseFloat(s['border'+side+'Width']) > 0 && s['border'+side+'Style'] !== 'none')
        problems.push('border: ' + el.tagName + '.' + [...el.classList].join('.'));
    if (s.outlineStyle !== 'none' && parseFloat(s.outlineWidth) > 0 && !el.matches(':focus-visible'))
      problems.push('outline: ' + el.tagName + '.' + [...el.classList].join('.'));
    if (/\\b0px 0px 0px [1-9]\\d*px\\b/.test(s.boxShadow))
      problems.push('ring-shadow: ' + el.tagName + '.' + [...el.classList].join('.'));
    if (el.tagName === 'HR') problems.push('hr');
  }
  // text overlap + clipped text on leaf nodes
  const nodes = all.filter((n) => n.children.length === 0 && n.textContent.trim() && n.getClientRects().length)
    .filter((n) => !n.closest('svg'));
  const rects = nodes.map((n) => ({ n, r: n.getBoundingClientRect() }));
  const clippedApart = (a, b) => {
    let p = a.parentElement;
    while (p) {
      const s = getComputedStyle(p);
      if (/hidden|clip|auto|scroll/.test(s.overflowX + s.overflowY) && !p.contains(b)) {
        const r = p.getBoundingClientRect(), q = b.getBoundingClientRect();
        if (q.left >= r.right || q.right <= r.left || q.top >= r.bottom || q.bottom <= r.top) return true;
      }
      p = p.parentElement;
    }
    return false;
  };
  const inFade = (n) => {
    let a = n.parentElement;
    while (a) {
      const s = getComputedStyle(a);
      if (s.maskImage && s.maskImage !== 'none') {
        const ar = a.getBoundingClientRect();
        if (n.getBoundingClientRect().bottom > ar.bottom - 34) return true;
      }
      a = a.parentElement;
    }
    return false;
  };
  for (let i=0;i<rects.length;i++) for (let j=i+1;j<rects.length;j++) {
    const a=rects[i], b=rects[j];
    if (a.n.contains(b.n) || b.n.contains(a.n)) continue;
    if (inFade(a.n) || inFade(b.n)) continue;
    if (clippedApart(a.n, b.n) || clippedApart(b.n, a.n)) continue;
    // adjacent inline fragments of one ellipsised line share a few tracking pixels
    const ac = a.n.closest('.sup-charter'), bc = b.n.closest('.sup-charter');
    if (ac && ac === bc) continue;
    if (a.n.closest('.num, .num-xl, .num-l') && b.n.closest('.num, .num-xl, .num-l')) continue;
    const x=Math.min(a.r.right,b.r.right)-Math.max(a.r.left,b.r.left);
    const y=Math.min(a.r.bottom,b.r.bottom)-Math.max(a.r.top,b.r.top);
    if (x>2 && y>2) problems.push('OVERLAP: "'+a.n.textContent.trim().slice(0,20)+'" x "'+b.n.textContent.trim().slice(0,20)+'"');
  }
  for (const {n,r} of rects) {
    const s = getComputedStyle(n);
    if (n.scrollWidth > n.clientWidth + 2 && s.overflow !== 'visible' && s.textOverflow !== 'ellipsis')
      problems.push('CLIPPED: "'+n.textContent.trim().slice(0,20)+'"');
  }
  // type law
  const sizes = new Set(), weights = new Set();
  for (const n of nodes) { const s=getComputedStyle(n); sizes.add(Math.round(parseFloat(s.fontSize)));
    weights.add(s.fontWeight); if (s.textTransform === 'uppercase') problems.push('UPPER: '+n.textContent.trim().slice(0,16)); }
  const allowed = [88,40,22,15,12];
  const badS = [...sizes].filter((n) => !allowed.includes(n));
  const badW = [...weights].filter((w) => !['450','500'].includes(w));
  if (badS.length) problems.push('SIZE: '+badS.join(','));
  if (badW.length) problems.push('WEIGHT: '+badW.join(','));
  // idle zero noise
  const bodyText = document.body.innerText;
  if (document.body.dataset.surface === 'popover' && bodyText.includes('Idle') &&
      /(Nothing selected|No target|None yet|0\\.0|\\b00\\b)/.test(bodyText))
    problems.push('ZEROS: idle layout prints empty metrics');
  // cursor law
  for (const el of document.querySelectorAll('button:not(:disabled), [role=button]:not([aria-disabled=true]), [role=tab]')) {
    if (getComputedStyle(el).cursor !== 'pointer') problems.push('CURSOR: '+el.className);
  }
  // overflow
  const sw = document.documentElement.scrollWidth, sh = document.documentElement.scrollHeight;
  if (sw > innerWidth+1 || sh > innerHeight+1) problems.push('OVERFLOW: '+sw+'x'+sh+' vs '+innerWidth+'x'+innerHeight);
  // geometry
  const shell = document.querySelector('.shell');
  const panel = document.querySelector('.panel');
  const rect = (sel) => { const e=document.querySelector(sel); if(!e) return null; const r=e.getBoundingClientRect();
    return {x:Math.round(r.x),y:Math.round(r.y),w:Math.round(r.width),h:Math.round(r.height)}; };
  return { problems, sizes:[...sizes].sort((a,b)=>a-b), weights:[...weights],
    shell: rect('.shell'), panel: rect('.panel'), head: rect('.p-head'), state: rect('.state'),
    sup: rect('.sup-card'), sess: rect('.sess'), drift: rect('.drift'), foot: rect('.p-foot'),
    panelScroll: panel ? {h:panel.scrollHeight, ch:panel.clientHeight} : null,
    shellBg: shell ? getComputedStyle(shell).backgroundColor : null,
    shellBlur: shell ? getComputedStyle(shell).backdropFilter : null,
    fontCheck: document.fonts.check('450 15px "Instrument Sans Variable"') };
})()`;

async function audit(name, p) {
  const r = await p.evaluate(AUDIT);
  results[name] = { name, ...r };
  problems({ ...r, name });
  return r;
}

// ---- popover modes ----
for (const demo of ['mixed', 'empty']) {
  const { c, p } = await context({ width: 1000, height: 760 }, 2);
  await open(p, `?demo=${demo}`);
  await p.screenshot({ path: join(out, `rest-${demo}.png`) });
  await p.mouse.move(500, 16); await p.waitForTimeout(120); await p.mouse.move(500, 16); await p.waitForTimeout(500);
  await p.screenshot({ path: join(out, `peek-${demo}.png`) });
  await p.mouse.click(500, 16); await p.waitForTimeout(800);
  await p.screenshot({ path: join(out, `open-${demo}.png`) });
  await audit(`popover-${demo}`, p);
  await c.close();
}

// ---- dashboard views / viewports ----
for (const vp of [{ width: 1280, height: 800 }, { width: 1024, height: 720 }, { width: 900, height: 700 }]) {
  for (const [name, demo, tab] of [
    ['tree', 'mixed', null], ['tree-empty', 'empty', null], ['mux', 'mixed', 'Mux'],
    ['mux-empty', 'empty', 'Mux'], ['comms', 'mixed', 'Comms'], ['comms-empty', 'empty', 'Comms'],
    ['alarms', 'mixed', 'Alarms'], ['alarms-empty', 'empty', 'Alarms'],
  ]) {
    const { c, p } = await context(vp, 1);
    await open(p, `?surface=dashboard&demo=${demo}`);
    if (tab) { await p.getByRole('tab', { name: tab }).click(); await p.waitForTimeout(700); }
    await p.screenshot({ path: join(out, `dash-${name}-${vp.width}.png`) });
    await audit(`dash-${name}-${vp.width}`, p);
    await c.close();
  }
}

// ---- action dispatch: every button that must reach a backend command ----
// Fresh browser: 30 screenshot contexts leave the first renderer in a bad state.
await browser.close();
browser = await launch();
{
  const { c, p } = await context();
  await open(p, '?surface=dashboard&demo=mixed');
  await p.locator('[data-sv="SV-1"]').first().click();
  await p.waitForTimeout(300);
  const send = p.locator('button:has-text("Message")').first();
  if (await send.count()) { await send.click(); await p.locator('.inspector-message input').first().fill('status');
    await p.locator('.inspector-message button[type=submit]').first().click(); }
  await p.locator('.spine-session').first().click();
  await p.waitForTimeout(250);
  const reprompt = p.locator('button:has-text("Re-prompt")').first();
  if (await reprompt.count()) await reprompt.click();
  await p.getByRole('tab', { name: 'Mux' }).click();
  await p.waitForTimeout(200);
  await p.locator('button.killswitch').click();
  await p.waitForTimeout(200);
  await p.locator('button.killswitch').click();
  await p.waitForTimeout(150);
  const actions = await p.evaluate(() => window.__notchActions);
  results.actions = actions;
  const names = actions.map((a) => a.cmd);
  for (const need of ['message', 'reprompt', 'kill_switch', 'haptic'])
    if (!names.includes(need)) errors.push(`ACTION: ${need} never dispatched from dashboard`);
  await c.close();
}
// fleet direct commands — another fresh browser for a clean renderer
await browser.close();
browser = await launch();
try {
  const { c, p } = await context();
  await p.goto(`http://127.0.0.1:${port}/?surface=dashboard&demo=mixed&fleet=1`, { waitUntil: 'networkidle' });
  await p.waitForTimeout(1200);
  await p.evaluate(() => {
    const set = (el, v) => {
      const s = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      s.call(el, v); el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    set(document.querySelector('input[aria-label^="Prompt for"]'), 'Inspect the ledger');
    set(document.querySelector('input[aria-label="Plan path"]'), '/tmp/plan.md');
    for (const text of ['Spawn', 'Run plan']) [...document.querySelectorAll('button')]
      .find((b) => b.textContent.trim() === text)?.click();
    document.querySelector('[data-storm="kill-all"]')?.click();
  });
  await p.waitForTimeout(200);
  const actions = await p.evaluate(() => window.__notchActions);
  results.fleetActions = actions.map((a) => a.cmd);
  for (const need of ['spawn', 'run_plan', 'fleet_control'])
    if (!results.fleetActions.includes(need)) errors.push(`ACTION: ${need} never dispatched from fleet`);
  await c.close();
} catch (e) {
  errors.push('FLEET ACTION CHECK: ' + String(e).slice(0, 200));
}

await browser.close();
server.close();
writeFileSync(join(root, '.proof', `v3-${pass}-checks.json`), JSON.stringify({ results, errors }, null, 2));
if (errors.length) { console.error(errors.join('\n')); process.exit(2); }
console.log(`PASS ${pass}: ${Object.keys(results).length} checks, no errors`);
