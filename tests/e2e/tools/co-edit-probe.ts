// Probe: two people typing into different paragraphs of a KB page at the same time in the
// visual editor; prints the final text so a lost or misplaced keystroke shows.
import { chromium } from 'playwright';
import { Client } from '../lib.ts';
import { cleanupRun } from '../cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const space = (await admin.post('/kb/spaces', { name: `Probe ${s}` })).data;
const page = (await admin.post(`/kb/spaces/${space.id}/pages`, { title: `P ${s}`, body: 'First paragraph.\n\nSecond paragraph.' })).data;
const open = async () => {
  const ctx = await browser.newContext();
  await ctx.addInitScript(() => localStorage.setItem('nerp:kb-editor', 'visual'));
  const p = await ctx.newPage();
  await p.goto(B + '/login'); await p.fill('#email', 'admin@acme.test'); await p.fill('#password', 'supersecret1');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  await p.goto(B + `/kb/page/${page.id}?edit=1`);
  await p.locator('[data-visual-editor] p', { hasText: 'Second paragraph.' }).waitFor();
  return p;
};
try {
  const a = await open(), b = await open();
  await a.locator('[data-visual-editor] p', { hasText: 'First' }).click(); await a.keyboard.press('End');
  await b.locator('[data-visual-editor] p', { hasText: 'Second' }).click(); await b.keyboard.press('End');
  await Promise.all([a.keyboard.type(' AAAAAAAAAAAAAAAAAAAA', { delay: 40 }), b.keyboard.type(' BBBBBBBBBBBBBBBBBBBB', { delay: 40 })]);
  await a.waitForTimeout(1500);
  // B clicks the line where A's caret sits, then presses End and types.
  const sel = () => b.evaluate(() => { const s = getSelection()!; return [s.anchorNode?.nodeName, (s.anchorNode?.textContent ?? '').slice(0, 30), s.anchorOffset]; });
  const line = b.locator('[data-visual-editor] p', { hasText: 'First' });
  await line.click();
  console.log('after click:', JSON.stringify(await sel()));
  await b.keyboard.press('End');
  console.log('after End:', JSON.stringify(await sel()));
  await line.click({ position: { x: 5, y: 5 } });
  console.log('click at left:', JSON.stringify(await sel()));
  const box = await line.locator('text=AAAA').boundingBox();
  console.log('box', JSON.stringify(box));
  console.log('A sees:', JSON.stringify(await a.locator('[data-visual-editor]').innerText()));
  console.log('B sees:', JSON.stringify(await b.locator('[data-visual-editor]').innerText()));
} finally {
  await browser.close();
  await admin.del(`/kb/spaces/${space.id}`);
  await cleanupRun(s);
}
