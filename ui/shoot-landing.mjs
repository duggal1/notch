import { chromium } from 'playwright';
import { mkdirSync } from 'fs';

const OUT = process.argv[2] || '.proof/landing';
mkdirSync(OUT, { recursive: true });

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });

const errors = [];
page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
page.on('pageerror', e => errors.push(String(e)));

await page.goto('http://localhost:8899/index.html', { waitUntil: 'networkidle' });
await page.waitForTimeout(900);

// full page desktop
await page.screenshot({ path: `${OUT}/01-full.png`, fullPage: true });

// hero crop
await page.screenshot({ path: `${OUT}/02-hero.png`, clip: { x: 0, y: 0, width: 1440, height: 980 } });

// each demo view
for (const v of ['tree','mux','comms','alarms']) {
  await page.click(`.seg button[data-view="${v}"]`);
  await page.waitForTimeout(500);
  const el = await page.$('#demo');
  await el.screenshot({ path: `${OUT}/03-view-${v}.png` });
}

// download menu open
await page.click('#dlBtn');
await page.waitForTimeout(400);
await page.screenshot({ path: `${OUT}/04-menu.png`, clip: { x: 900, y: 0, width: 540, height: 320 } });
await page.keyboard.press('Escape');

// narrow
await page.setViewportSize({ width: 390, height: 844 });
await page.waitForTimeout(500);
await page.screenshot({ path: `${OUT}/05-mobile.png`, fullPage: true });

// overflow check
const overflow = await page.evaluate(() => {
  const w = document.documentElement.scrollWidth;
  return { scrollW: w, clientW: document.documentElement.clientWidth, over: w > document.documentElement.clientWidth + 1 };
});

console.log(JSON.stringify({ errors, overflow }, null, 2));
await browser.close();