import { createServer } from 'node:http';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { chromium } from 'playwright';

const here = dirname(fileURLToPath(import.meta.url));
const fixture = await build({
  entryPoints: [resolve(here, 'test-fixtures/design-snapshots.ts')],
  bundle: true, format: 'esm', platform: 'node', write: false,
});
const fixtureUrl = 'data:text/javascript;base64,' + Buffer.from(fixture.outputFiles[0].text).toString('base64');
const { toSnapshot } = await import(fixtureUrl);
const transport = await build({
  entryPoints: [resolve(here, 'test-fixtures/desktop-transport.ts')],
  bundle: true, format: 'iife', globalName: 'desktopFixture', write: false,
});

const root = resolve(here, 'dist');
const pass = process.argv[2] ?? 'latest';
const folder = resolve(here, '../.proof/design', pass);
await mkdir(folder, { recursive: true });
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.woff2': 'font/woff2' };
const server = createServer(async (req, res) => {
  try {
    const path = resolve(root, '.' + new URL(req.url, 'http://localhost').pathname.replace(/^\/$/, '/index.html'));
    if (!path.startsWith(root + '/')) { res.writeHead(403).end(); return; }
    res.writeHead(200, { 'content-type': mime[extname(path)] ?? 'application/octet-stream' }).end(await readFile(path));
  } catch { res.end(); }
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const browser = await chromium.launch({ headless: true });
const errors = [];
const records = {};
const demos = process.env.NOTCH_SHOT_FAST ? ['mixed', 'empty'] : ['idle', 'mixed', 'failed', 'alldone', 'big', 'empty'];
const dimensions = process.env.NOTCH_SHOT_FAST ? [[900,700]] : [[1280,800],[1024,720],[900,700]];
const base = `http://127.0.0.1:${server.address().port}`;
const audit = async (page, name, idle, surface) => {
  const result = await page.evaluate(({ idle, surface }) => {
    const faults = [];
    if (surface === '.shell') {
      const css = getComputedStyle(document.querySelector(surface));
      if (css.backgroundColor !== 'rgb(0, 0, 0)' || css.backdropFilter !== 'none') faults.push('notch must be opaque black');
      if (css.clipPath !== 'none') faults.push('notch uses a clip path');
    }
    if (document.documentElement.scrollWidth > innerWidth || document.documentElement.scrollHeight > innerHeight) faults.push('viewport overflow');
    if (!document.fonts.check('450 15px "Instrument Sans Variable"') || getComputedStyle(document.body).fontWeight !== '450') faults.push('Instrument Sans 450 missing');
    const text = document.querySelector(surface)?.innerText ?? '';
    if (idle && /\b(?:00|0\.0|None yet|No target|0 in flight|0 supervisors)\b/.test(text)) faults.push('idle noise');
    const visible = [...document.querySelectorAll(surface + ' *')].filter(el => el.getClientRects().length && getComputedStyle(el).visibility === 'visible');
    for (const el of visible) {
      const s = getComputedStyle(el);
      const label = el.tagName + '.' + (typeof el.className === 'string' ? el.className : '');
      if (['Top','Right','Bottom','Left'].some(side => parseFloat(s['border'+side+'Width']) > 0 && s['border'+side+'Style'] !== 'none')) faults.push('border ' + label);
      if (s.outlineStyle !== 'none' && parseFloat(s.outlineWidth) > 0) faults.push('outline ' + label);
      if (el.tagName === 'HR') faults.push('hr ' + label);
      if (el.matches('svg[aria-hidden]') && el.querySelector('rect') && el.getAttribute('viewBox') === '0 0 12 12' && ![12,24,36].includes(Number(el.getAttribute('width')))) faults.push('icon ' + label);
      const directText = [...el.childNodes].some(node => node.nodeType === 3 && node.textContent.trim());
      if (directText && ![88,40,22,15,12].includes(parseFloat(s.fontSize))) faults.push('font size ' + label + ' ' + s.fontSize);
      if (directText && !['450','500'].includes(s.fontWeight)) faults.push('font weight ' + label + ' ' + s.fontWeight);
      if (directText && s.textTransform === 'uppercase') faults.push('uppercase ' + label);
    }
    const ink = [...document.querySelectorAll('button, [role="tab"], .stat, .center-status, .p-status')].filter(el => el.checkVisibility());
    for (const pill of ink) {
      const box = pill.getBoundingClientRect();
      if (box.width < 1 || box.height < 1 || pill.classList.contains('rail-item')) continue;
      const range = document.createRange();
      const nodes = document.createTreeWalker(pill, NodeFilter.SHOW_TEXT);
      while (nodes.nextNode()) {
        const node = nodes.currentNode;
        if (!node.textContent.trim() || !node.parentElement.checkVisibility({ opacityProperty: true })) continue;
        range.selectNodeContents(node);
        for (const rect of range.getClientRects()) {
          if (rect.x < box.x - 2 || rect.right > box.right + 2 || rect.top < box.top - 2 || rect.bottom > box.bottom + 2) {
            if (!node.parentElement.closest('.truncate, .model-btn, .comms-row')) faults.push('pill text overflow ' + pill.className);
          }
        }
      }
    }
    const regions = [...document.querySelectorAll('.tree-start-choices, .composer, .p-head, .p-foot, .empty-form')].filter(el => el.checkVisibility());
    for (const canvas of document.querySelectorAll('canvas.dither-field')) {
      const box = canvas.getBoundingClientRect();
      for (const region of regions) {
        const rect = region.getBoundingClientRect();
        if (Math.min(box.right, rect.right) - Math.max(box.left, rect.left) > 0
          && Math.min(box.bottom, rect.bottom) - Math.max(box.top, rect.top) > 0) faults.push('dither intersects ' + region.className);
      }
    }
    const cards = [...document.querySelectorAll('.mux-card')].map(el => el.offsetHeight);
    if (cards.some(height => height !== 224)) faults.push('mux cards differ from 224px');
    const composer = document.querySelector('.composer')?.getBoundingClientRect();
    for (const selector of ['.tree-start-choices','.center-head','.transcript']) {
      const element = document.querySelector(selector);
      if (composer && element && element.getBoundingClientRect().bottom > composer.top + 1) faults.push('composer overlap ' + selector);
    }
    return { faults: [...new Set(faults)], text: text.slice(0, 100) };
  }, { idle, surface });
  records[name] = result;
};
const capture = async (page, name, selectors = {}) => {
  await page.screenshot({ path: resolve(folder, name + '.png') });
  for (const [region, selector] of Object.entries(selectors)) {
    const locator = page.locator(selector).first();
    if (await locator.count() === 0) continue;
    const rect = await locator.boundingBox();
    if (!rect || rect.width < 2 || rect.height < 2) continue;
    await page.screenshot({ path: resolve(folder, name + '-' + region + '.png'), clip: rect });
  }
};
const prepare = async (page, query, demo) => {
  page.on('pageerror', error => errors.push(String(error)));
  await page.goto(base + query + (query.includes('?') ? '&' : '?') + 'demo=' + demo);
  await page.waitForFunction(() => typeof window.__push === 'function');
  await page.evaluate(snapshot => window.__push(snapshot), toSnapshot(demo, demo === 'mixed' || demo === 'big' ? 6.4 : 0, 1));
  await page.evaluate(() => document.fonts.ready);
};
try {
  for (const [w,h] of dimensions) {
    for (const demo of demos) {
      const page = await browser.newPage({ viewport: { width:w,height:h }, deviceScaleFactor: 3, reducedMotion: 'reduce' });
      await prepare(page, '/?surface=dashboard', demo);
      for (const tab of ['Tree','Mux','Comms','Alarms']) {
        await page.getByRole('tab', { name: tab }).click();
        await page.getByRole('tabpanel', { name: tab + ' view' }).waitFor();
        const name = `${tab.toLowerCase()}-${demo}-${w}x${h}`;
        await audit(page, name, demo === 'empty', '.dash');
        await capture(page, name, tab === 'Tree' && ['empty','mixed'].includes(demo) ? { topbar: '.topbar', masthead: '.dashboard-masthead', stage: '.center-pane', composer: '.composer', spine: '.sidebar' } : {});
      }
      await page.close();
    }
  }
  for (const demo of demos) {
    const page = await browser.newPage({ viewport: { width: 900,height: 700 }, deviceScaleFactor: 3, reducedMotion: 'reduce' });
    await prepare(page, '/', demo);
    await capture(page, 'popover-rest-' + demo);
    await audit(page, 'popover-rest-' + demo, demo === 'empty', '.shell');
    const expected = toSnapshot(demo, 0, 1).sessions.filter(session => session.status === 'working').length;
    if (expected && (await page.locator('.rest-count').textContent()) !== String(expected)) errors.push('wrong collapsed working count for ' + demo);
    await page.mouse.move(450, 25);
    await page.locator('.shell[data-mode=peek]').waitFor();
    await capture(page, 'popover-peek-' + demo);
    await audit(page, 'popover-peek-' + demo, demo === 'empty', '.shell');
    await page.mouse.click(450, 25);
    await page.locator('.shell[data-mode=open] .panel').waitFor();
    await audit(page, 'popover-' + demo, demo === 'empty', '.shell');
    await capture(page, 'popover-open-' + demo, { header: '.p-head', composer: '.empty-form' });
    await page.keyboard.press('Escape');
    await page.locator('.shell[data-mode=rest]').waitFor();
    await page.close();
  }
  {
    const page = await browser.newPage({ viewport: { width: 900, height: 700 }, deviceScaleFactor: 3, reducedMotion: 'reduce' });
    await prepare(page, '/?surface=dashboard', 'empty');
    await page.getByRole('button', { name: 'Fix a test' }).click();
    const input = page.getByRole('textbox', { name: 'Assign work to a session' });
    await page.waitForFunction(() => document.querySelector('.composer-input')?.value.includes('failing test'));
    if (await input.evaluate(el => getComputedStyle(el).boxShadow) !== 'none') errors.push('composer field focus ring');
    await page.getByRole('button', { name: 'Done when…' }).click();
    await page.getByRole('textbox', { name: 'Definition of done' }).fill('Verify from a clean checkout');
    await page.getByRole('textbox', { name: 'Working directory' }).fill('relative');
    await page.getByRole('button', { name: 'Assign', exact: true }).click();
    await page.getByText('absolute path expected', { exact: false }).waitFor();
    if (!(await page.locator('button.killswitch').isDisabled())) errors.push('idle kill enabled');
    if (await page.locator('.press-ripple').count()) errors.push('ripple present under reduced motion');
    await page.keyboard.press('Tab');
    await capture(page, 'tree-keyboard-focus');
    await page.close();
  }
  {
    const page = await browser.newPage({ viewport: { width: 900, height: 700 }, deviceScaleFactor: 3 });
    await page.goto(base + '/?surface=dashboard');
    await page.getByRole('status').getByText('Connecting to notchd…').waitFor();
    await capture(page, 'tree-loading');
    await page.evaluate(snapshot => window.__push(snapshot), toSnapshot('mixed', 6.4, 1));
    const tree = page.getByRole('tab', { name: 'Tree' });
    await tree.focus();
    if (await tree.evaluate(el => getComputedStyle(el).boxShadow) === 'none') errors.push('tab keyboard focus missing');
    await page.keyboard.press('ArrowRight');
    await page.getByRole('tabpanel', { name: 'Mux view' }).waitFor();
    if (await page.evaluate(() => document.activeElement.textContent.trim()) !== 'Mux') errors.push('arrow key failed to move tab focus');
    const comms = page.getByRole('tab', { name: 'Comms' });
    const hit = await comms.boundingBox();
    await page.mouse.move(hit.x + hit.width / 2, hit.y + hit.height / 2);
    await page.mouse.down();
    if (await page.locator('.press-ripple').count()) errors.push('pointer ripple should be absent');
    await page.mouse.up();
    await page.getByRole('tabpanel', { name: 'Comms view' }).waitFor();
    await page.getByRole('button', { name: 'Cross supervisor' }).click();
    if (await page.locator('.comms-row').count() !== 1) errors.push('cross supervisor filter');
    await page.close();
  }
  {
    const page = await browser.newPage({ viewport: { width: 1024, height: 720 }, deviceScaleFactor: 3, reducedMotion: 'reduce' });
    const mixed = toSnapshot('mixed', 6.4, 1);
    await page.addInitScript({ content: transport.outputFiles[0].text + `
      window.testCalls = [];
      window.addEventListener('test-ipc', event => window.testCalls.push(event.detail));
      desktopFixture.install({ snapshot: ${JSON.stringify(mixed)} });
    ` });
    await page.goto(base + '/?surface=dashboard');
    const field = page.getByRole('textbox', { name: 'Assign work to a session' });
    await page.getByRole('log').getByText('Unit tests completed').waitFor();
    await field.fill('Continue the current task');
    await page.locator('.assign-btn').click();
    await page.waitForFunction(() => window.testCalls.some(({ command, args }) =>
      command === 'reprompt' && args.id === 'S1-OC01' && args.text === 'Continue the current task'));
    await page.getByRole('button', { name: 'New supervisor' }).click();
    await field.fill('Assign a second task');
    await page.locator('.assign-btn').click();
    await page.waitForFunction(() => window.testCalls.some(({ command, args }) =>
      command === 'submit_task' && args.task === 'Assign a second task'));
    await page.locator('.dashboard-fleet').click();
    await page.waitForFunction(() => window.testCalls.some(({ command }) => command === 'fleet_state'));
    await capture(page, 'tree-fleet');
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
  await writeFile(resolve(folder, 'audit.json'), JSON.stringify({ errors, records }, null, 2));
}
const failures = Object.entries(records).filter(([,result]) => result.faults.length);
console.log(JSON.stringify({ checked: Object.keys(records).length, errors, failures }, null, 2));
if (errors.length || failures.length) process.exitCode = 1;
