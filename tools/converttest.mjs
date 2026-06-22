// Verify the in-browser OBJ→OBJV conversion: upload a .obj via the file input,
// confirm it converts, renders, and exposes a download link.
import { chromium } from 'playwright-core';
const EXE = process.env.HOME +
  '/Library/Caches/ms-playwright/chromium-1208/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing';

const browser = await chromium.launch({
  executablePath: EXE,
  args: ['--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--use-angle=swiftshader'],
});
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
const logs = [];
page.on('console', m => logs.push(`[${m.type()}] ${m.text()}`));

// Start on a blank page (no auto-loaded mesh) so we test the .obj path cleanly.
await page.goto('http://localhost:8848/index.html?mesh=none', { waitUntil: 'load' });
await page.waitForTimeout(1500);

await page.setInputFiles('#file', '/Users/nipun/git/obj-viewer/test_outcrop.obj');
await page.waitForTimeout(4000);

const status = await page.locator('#status').textContent();
const dl = await page.locator('#download').textContent();
const size = await page.locator('#m-size').textContent();
const verts = await page.locator('#m-verts').textContent();
await page.screenshot({ path: '/tmp/objv_convert.png' });

console.log('status :', status);
console.log('download:', dl);
console.log('m-size :', size, '| verts:', verts);
console.log('convert log:', logs.filter(l => l.includes('converted OBJ')).join(' '));
await browser.close();
