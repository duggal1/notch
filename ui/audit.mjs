import { chromium } from 'playwright';

const browser = await chromium.launch();
const results = {};

for (const w of [1440, 1024, 900, 720, 390]) {
  const page = await browser.newPage({ viewport: { width: w, height: 900 } });
  await page.goto('http://localhost:8899/index.html', { waitUntil: 'networkidle' });
  await page.waitForTimeout(500);
  results['w' + w] = await page.evaluate(() => ({
    scrollW: document.documentElement.scrollWidth,
    clientW: document.documentElement.clientWidth,
    over: document.documentElement.scrollWidth > document.documentElement.clientWidth + 1
  }));
  await page.close();
}

// compliance on desktop
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
await page.goto('http://localhost:8899/index.html', { waitUntil: 'networkidle' });
await page.waitForTimeout(600);

results.compliance = await page.evaluate(() => {
  const cs = el => getComputedStyle(el);
  const bad = [];

  // type sizes actually used
  const sizes = {};
  document.querySelectorAll('body *').forEach(el => {
    if (!el.textContent.trim()) return;
    const px = cs(el).fontSize;
    sizes[px] = (sizes[px] || 0) + 1;
  });

  // weights
  const weights = {};
  document.querySelectorAll('body *').forEach(el => {
    if (!el.textContent.trim()) return;
    const w = cs(el).fontWeight;
    weights[w] = (weights[w] || 0) + 1;
  });

  // uppercase
  const uppers = [];
  document.querySelectorAll('body *').forEach(el => {
    if (!el.textContent.trim() || el.children.length) return;
    if (cs(el).textTransform === 'uppercase') uppers.push(el.tagName + '.' + el.className.toString().slice(0,30));
  });

  // interactive radius (must be pill / >= rounded)
  const sq = [];
  document.querySelectorAll('button, a.btn, input, .seg button, .node').forEach(el => {
    const r = cs(el).borderRadius;
    const v = parseFloat(r);
    if (r === '0px' || (v < 12 && !r.includes('999'))) sq.push(el.tagName + '.' + el.className.toString().slice(0,30) + ' => ' + r);
  });

  // box-shadow outside focus
  const shadows = [];
  document.querySelectorAll('body *').forEach(el => {
    const s = cs(el).boxShadow;
    if (s && s !== 'none') shadows.push(el.className.toString().slice(0,30) + ' => ' + s.slice(0,60));
  });

  return {
    fontSizes: sizes,
    fontWeights: weights,
    uppercaseCount: uppers.length,
    uppercaseSample: uppers.slice(0, 6),
    nonPillControls: sq.slice(0, 10),
    nonPillCount: sq.length,
    boxShadowCount: shadows.length,
    boxShadowSample: shadows.slice(0, 5)
  };
});

console.log(JSON.stringify(results, null, 2));
await browser.close();