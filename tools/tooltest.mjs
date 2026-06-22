// Drive the analysis tools headlessly: select a tool, click surface points,
// read the #results panel. Verifies picking + measure/section/dip end to end.
import { chromium } from 'playwright-core';
const EXE = process.env.HOME +
  '/Library/Caches/ms-playwright/chromium-1208/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing';

const browser = await chromium.launch({
  executablePath: EXE,
  args: ['--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--use-angle=swiftshader'],
});
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
const logs = [];
page.on('console', m => { if (m.type() === 'error') logs.push(m.text()); });
page.on('pageerror', e => logs.push('ERR ' + e.message));

await page.goto('http://localhost:8848/index.html?mesh=test.objv', { waitUntil: 'load' });
await page.waitForTimeout(3500); // wasm init + decode + first frames

const results = async () => (await page.locator('#results').textContent()).replace(/\n/g, ' | ');
const pick = async (key, pts, name) => {
  await page.locator(`#tools button[data-key="${key}"]`).click();
  for (const [x, y] of pts) { await page.mouse.click(x, y); await page.waitForTimeout(120); }
  await page.waitForTimeout(200);
  console.log(`${name}:`, await results());
};

await pick('m', [[420, 430], [760, 360], [900, 520]], 'MEASURE');
await page.screenshot({ path: '/tmp/objv_measure.png' });
await pick('s', [[380, 300], [950, 520]], 'SECTION');
await page.screenshot({ path: '/tmp/objv_section.png' });
await pick('d', [[500, 380], [700, 350], [620, 520], [560, 300]], 'DIP');
await page.screenshot({ path: '/tmp/objv_dip.png' });

console.log('tool indicator:', await page.locator('#m-tool').textContent());
if (logs.length) console.log('PAGE ERRORS:', logs.join(' ; '));
await browser.close();
