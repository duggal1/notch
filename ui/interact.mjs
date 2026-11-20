import { chromium } from 'playwright';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
await page.goto('http://localhost:8899/index.html', { waitUntil: 'networkidle' });
await page.waitForTimeout(600);

const out = {};

// 1. view switch changes what's visible
out.views = {};
for (const v of ['tree','mux','comms','alarms']) {
  await page.click(`.seg button[data-view="${v}"]`);
  await page.waitForTimeout(350);
  out.views[v] = await page.evaluate(() => {
    const vis = [...document.querySelectorAll('.view')].filter(e => !e.hidden).map(e => e.getAttribute('data-view'));
    return { visible: vis, count: document.getElementById('demoCount').textContent };
  });
}

// 2. segmented indicator moves
out.indicator = await page.evaluate(() => {
  const ind = document.getElementById('segInd');
  return { width: ind.style.width, transform: ind.style.transform };
});

// 3. download menu opens/closes
await page.click('#dlBtn');
await page.waitForTimeout(300);
out.menuOpen = await page.evaluate(() => document.getElementById('navDl').hasAttribute('data-open'));
await page.keyboard.press('Escape');
await page.waitForTimeout(200);
out.menuClosed = await page.evaluate(() => !document.getElementById('navDl').hasAttribute('data-open'));

// 4. node selection moves (tree view must be active first)
await page.click('.seg button[data-view="tree"]');
await page.waitForTimeout(300);
await page.click('.node[data-lv="1"]');
await page.waitForTimeout(200);
out.nodeSelect = await page.evaluate(() => {
  const sel = document.querySelector('.node[data-sel]');
  return sel ? sel.textContent.trim().slice(0,20) : null;
});

// 5. ripple element injected on press
await page.click('.btn-primary');
await page.waitForTimeout(60);
out.rippleInjected = await page.evaluate(() => document.querySelectorAll('.ripple').length > 0);

// 6. dither canvases painted (non-blank)
out.dithers = await page.evaluate(() => {
  const cvs = [...document.querySelectorAll('[data-dither] canvas, [data-rule] canvas')];
  return cvs.map(c => {
    const cx = c.getContext('2d');
    const d = cx.getImageData(0, 0, c.width, c.height).data;
    let lit = 0;
    for (let i = 3; i < d.length; i += 4) if (d[i] > 0) lit++;
    return { w: c.width, h: c.height, litPct: Math.round(lit / (d.length/4) * 100) };
  });
});

// 7. meters + sparks + mux + wire + alarms populated
out.populated = await page.evaluate(() => ({
  meters: document.querySelectorAll('.pixmeter i').length,
  sparks: document.querySelectorAll('[data-spark] i').length,
  estrip: document.querySelectorAll('#estrip i').length,
  muxCards: document.querySelectorAll('.mcard').length,
  wireMsgs: document.querySelectorAll('.wmsg').length,
  alarms: document.querySelectorAll('.acard').length
}));

// 8. reduced motion: no entrance animations
const rm = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
await rm.emulateMedia({ reducedMotion: 'reduce' });
await rm.goto('http://localhost:8899/index.html', { waitUntil: 'networkidle' });
await rm.waitForTimeout(400);
out.reducedMotion = await rm.evaluate(() => {
  const h1 = document.querySelector('h1');
  return { animDuration: getComputedStyle(h1.closest('.hero')).animationDuration };
});

console.log(JSON.stringify(out, null, 2));
await browser.close();