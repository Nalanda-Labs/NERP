// Attachments in the office editor, in the browser: a task's document has an Edit link that
// opens it in the editor.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { OUTPUT } from './output.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail95-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
const me = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user;
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const status = (await admin.get('/office/status')).data;
if (!status.editing) { console.log('skipped: no document server (COLLABORA_URL)'); process.exit(0); }
const fd = new FormData(); fd.append('file', new Blob([`notes ${s}`], { type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' }), `notes-${s}.docx`);
const doc = (await admin.req('POST', '/files', fd) as any).data.id;
const task = (await admin.post('/tasks', { title: `Write ${s}`, responsible_id: me.id })).data;
await db.query('INSERT INTO task_files (task_id, file_id, added_by) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING', [task.id, doc, me.id]);

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource|frame-ancestors|cool|wopi/i.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('a task\'s document opens in the editor', async () => {
    await pa.goto(`${B}/tasks/${task.id}`);
    await pa.click(`[data-edit-attachment="notes-${s}.docx"]`);
    await pa.waitForURL(new RegExp(`/files/edit/${doc}`));
    await pa.locator('iframe[data-office]').waitFor();
    await pa.locator(`h1:has-text("notes-${s}.docx")`).waitFor();
    if (await pa.locator('[role="alert"]').count()) throw new Error(await pa.locator('[role="alert"]').innerText());
  });
  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query('DELETE FROM tasks WHERE id = $1', [task.id]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
