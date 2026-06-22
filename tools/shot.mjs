// Headless render check: load the viewer, capture Rust/JS console output,
// screenshot the canvas. Usage: node shot.mjs <mesh> <out.png> [waitMs]
import { chromium } from 'playwright-core';

const EXE = process.env.HOME +
  '/Library/Caches/ms-playwright/chromium-1208/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing';

const mesh = process.argv[2] || 'test.objv';
const out = process.argv[3] || '/tmp/objv_shot.png';
const waitMs = parseInt(process.argv[4] || '4000', 10);

const browser = await chromium.launch({
  executablePath: EXE,
  args: [
    '--enable-unsafe-swiftshader', // allow software WebGL2 in headless
    '--ignore-gpu-blocklist',
    '--use-angle=swiftshader',
  ],
});
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });

const logs = [];
page.on('console', m => logs.push(`[${m.type()}] ${m.text()}`));
page.on('pageerror', e => logs.push(`[pageerror] ${e.message}`));

const url = `http://localhost:8848/index.html?mesh=${encodeURIComponent(mesh)}`;
await page.goto(url, { waitUntil: 'load', timeout: 30000 });

// Give wasm init + decode + a few RAF frames time to land.
await page.waitForTimeout(waitMs);

await page.screenshot({ path: out });

// Sample the canvas: is anything other than the clear color present?
const stats = await page.evaluate(() => {
  const c = document.getElementById('gl');
  return { w: c.width, h: c.height, status: document.getElementById('status').textContent };
});

console.log('--- console ---');
console.log(logs.join('\n'));
console.log('--- stats ---');
console.log(JSON.stringify(stats));
console.log('screenshot:', out);
await browser.close();
