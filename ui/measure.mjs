import { chromium } from 'playwright';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });
await page.goto('http://localhost:8899/index.html', { waitUntil: 'networkidle' });
await page.waitForTimeout(700);

const offenders = await page.evaluate(() => {
  const vw = document.documentElement.clientWidth;
  const out = [];
  document.querySelectorAll('body *').forEach(el => {
    const r = el.getBoundingClientRect();
    if (r.width === 0) return;
    if (r.right > vw + 1 || r.left < -1) {
      out.push({ tag: el.tagName, cls: el.className && el.className.toString().slice(0,40), left: Math.round(r.left), right: Math.round(r.right), w: Math.round(r.width) });
    }
  });
  return { vw, scrollW: document.documentElement.scrollWidth, offenders: out.slice(0, 25) };
});
console.log(JSON.stringify(offenders, null, 2));
await browser.close();