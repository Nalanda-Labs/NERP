// Collaboration in the browser, part 2: a video announcement recorded in the feed (and played
// from the post), a board for a post, a board made in the drive with a drive document placed on
// it, and an office file shared by a link anyone can edit.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
// A camera and microphone that make test pictures and sound, allowed without asking.
const browser = await chromium.launch({ executablePath: exe, args: ['--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream'] });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail73-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB', permissions: ['camera', 'microphone'] });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
// The app's permissions policy refuses the board library's unload handler (as in ui55).
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|409|Failed to load resource|unload is not allowed/.test(m.text()) && errors.push(m.text() + ' @ ' + pa.url()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('a video announcement recorded in the feed', async () => {
    await pa.goto(B + '/feed');
    await pa.click('[data-record-video]');
    await pa.click('[data-start-recording]');
    await pa.waitForTimeout(2500);
    await pa.click('[data-stop-recording]');
    await pa.click('[data-use-video]');
    await pa.locator('text=Video announcement.webm').first().waitFor({ timeout: 15000 });
    await pa.locator('textarea').first().fill(`Hello team ${s}`);
    await pa.click('button:has-text("Post")');
    const post = pa.locator('article, [data-post]', { hasText: `Hello team ${s}` }).first();
    await post.locator('[data-video-player] video').waitFor({ timeout: 15000 });
    // Converted in the background; the player then has the copy (and a poster).
    const fileId = await post.locator('[data-video-player]').getAttribute('data-video-player');
    const end = Date.now() + 90000;
    let st = '';
    while (Date.now() < end && st !== 'ready') { st = (await admin.get(`/files/${fileId}/video-status`)).data.status; if (st !== 'ready') await pa.waitForTimeout(1000); }
    if (st !== 'ready') throw new Error(`conversion ${st}`);
  });

  await step('a board for a post', async () => {
    await pa.goto(B + '/feed');
    await pa.locator('textarea').first().fill(`Ideas for the offsite ${s}`);
    await pa.click('[data-post-board]');
    await pa.click('button:has-text("Post")');
    const card = pa.locator('[data-post-board-card]', { hasText: `Ideas for the offsite ${s}` });
    await card.waitFor({ timeout: 15000 });
    await card.click();
    await pa.waitForURL(/\/boards\/[0-9a-f-]+$/);
  });

  let folderId = '';
  await step('a board made in the drive, with a drive document on it', async () => {
    const folder = (await admin.post('/drive/folders', { name: `Offsite ${s}` })).data;
    folderId = folder.id;
    const fd = new FormData();
    fd.append('file', new Blob(['agenda']), 'agenda.txt');
    const fid = (await admin.req('POST', '/files', fd)).data.id;
    await admin.post('/drive/files', { file_id: fid, name: `Agenda ${s}.txt`, parent_id: folder.id });
    await pa.goto(B + `/drive?folder=${folder.id}`);
    pa.once('dialog', (d) => d.accept(`Plan ${s}`));
    await pa.click('[data-new-board]');
    await pa.waitForURL(/\/boards\/[0-9a-f-]+$/);
    await pa.locator('.excalidraw').waitFor({ timeout: 20000 });
    await pa.click('[data-add-document]');
    await pa.fill('input[aria-label="Find a file"]', `Agenda ${s}`);
    await pa.locator('[data-doc-hit]', { hasText: `Agenda ${s}.txt` }).click();
    await pa.locator('[data-saving]:has-text("Saved")').waitFor({ timeout: 15000 });
    const boardId = pa.url().split('/').pop()!;
    const end = Date.now() + 10000;
    let has = false;
    while (Date.now() < end && !has) {
      const scene = (await admin.get(`/boards/${boardId}/scene`)).data.scene;
      has = scene.elements.some((e: any) => typeof e.link === 'string' && e.link.includes('/drive?file='));
      if (!has) await pa.waitForTimeout(500);
    }
    if (!has) throw new Error('no document card saved');
    await pa.goto(B + `/drive?folder=${folder.id}`);
    await pa.locator(`text=Plan ${s}.board`).click();
    await pa.waitForURL(new RegExp(`/boards/${boardId}$`));
  });

  await step('an office file shared by a link anyone can edit', async () => {
    if (!(await admin.get('/office/status')).data.editing) return console.log('     (skipped: no COLLABORA_URL)');
    const fd = new FormData();
    fd.append('file', new Blob(['{\\rtf1 Draft}']), 'draft.rtf');
    const fid = (await admin.req('POST', '/files', fd)).data.id;
    const node = (await admin.post('/drive/files', { file_id: fid, name: `Draft ${s}.rtf`, parent_id: folderId })).data;
    await pa.goto(B + `/drive?folder=${folderId}`);
    await pa.locator(`text=Draft ${s}.rtf`).click();
    await pa.locator('[data-link-edit]').check();
    await pa.click('button:has-text("Make a link")');
    const url = await pa.locator('[data-link]').innerText();
    if (!(await admin.get(`/drive/nodes/${node.id}`)).data.links.some((l: any) => l.can_edit)) throw new Error('not an editing link');
    // A visitor (not signed in) opens it in the editor.
    const visitor = await (await browser.newContext()).newPage();
    pages.push(visitor);
    await visitor.goto(url);
    await visitor.fill('#visitor', 'Pat Partner');
    await visitor.click('[data-open-editor] button:has-text("Edit")');
    await visitor.locator('text=Changes are saved for everyone.').waitFor({ timeout: 15000 });
    await visitor.locator('iframe[data-office]').waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
