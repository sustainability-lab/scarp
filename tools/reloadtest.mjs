// Verify the fixes: (1) load sample, (2) upload a .obj — it must convert in a
// worker (progress bar appears), then REPLACE the mesh (not revert to sample),
// and expose a download.
import { chromium } from 'playwright-core';
const EXE = process.env.HOME +
  '/Library/Caches/ms-playwright/chromium-1208/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing';

const browser = await chromium.launch({
  executablePath: EXE,
  args: ['--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--use-angle=swiftshader'],
});
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
const errs = [];
page.on('console', m => { if (m.type() === 'error') errs.push(m.text()); });
page.on('pageerror', e => errs.push('ERR ' + e.message));

// Start with the sample auto-loaded.
await page.goto('http://localhost:8848/index.html', { waitUntil: 'load' });
await page.waitForTimeout(3000);
const beforeSize = await page.locator('#m-size').textContent();

// Now upload a DIFFERENT .obj (bigger grid) and confirm it takes over.
await page.setInputFiles('#file', '/Users/nipun/git/obj-viewer/test_outcrop.obj');
// Poll for the progress bar turning on at some point.
let sawProgress = false;
for (let i = 0; i < 30; i++) {
  if (await page.locator('#progress.on').count()) { sawProgress = true; break; }
  await page.waitForTimeout(50);
}
await page.waitForTimeout(4000);

const afterSize = await page.locator('#m-size').textContent();
const status = await page.locator('#status').textContent();
const dl = await page.locator('#download').textContent();
const tris = await page.locator('#m-tris').textContent();
await page.screenshot({ path: '/tmp/objv_reload.png' });

console.log('saw progress bar:', sawProgress);
console.log('size before/after:', beforeSize, '->', afterSize);
console.log('status:', status);
console.log('download:', dl, '| tris:', tris);
console.log('reverted to sample?', status.includes('sample') ? 'YES (BUG)' : 'no');
if (errs.length) console.log('ERRORS:', errs.join(' ; '));
await browser.close();
