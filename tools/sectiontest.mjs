import { chromium } from 'playwright-core';
const EXE = process.env.HOME + '/Library/Caches/ms-playwright/chromium-1208/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing';
const browser = await chromium.launch({ executablePath: EXE, args: ['--enable-unsafe-swiftshader','--ignore-gpu-blocklist','--use-angle=swiftshader'] });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
await page.goto('http://localhost:8848/index.html?mesh=test.objv', { waitUntil: 'load' });
await page.waitForTimeout(3500);
await page.locator('#tools button[data-key="s"]').click();
for (const [x,y] of [[420,430],[760,360]]) { await page.mouse.click(x,y); await page.waitForTimeout(150); }
await page.waitForTimeout(300);
console.log('SECTION:', (await page.locator('#results').textContent()).replace(/\n/g,' | '));
await page.screenshot({ path: '/tmp/objv_section.png' });
await browser.close();
