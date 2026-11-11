// Office files in the browser: a Drive file's PDF preview opens in a window; "Edit" opens the
// editor page, which posts the signed token into the editor's frame (a stand-in for
// Collabora on :9988 records it and, like Collabora, reads the file through NERP's WOPI).
// The editing part needs the server started with COLLABORA_URL=http://127.0.0.1:9988.
import http from 'node:http';
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail57-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `ivy.${s}@acme.test`, password: 'password123', first_name: 'Ivy', last_name: `Office${s}` });
const ivy = new Client();
await ivy.post('/auth/login', { email: `ivy.${s}@acme.test`, password: 'password123' });
const RTF = `{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Helvetica;}}\\f0 Budget ${s}\\par}`;
const fd = new FormData();
fd.append('file', new Blob([RTF]), `Budget ${s}.rtf`);
const up = (await ivy.req('POST', '/files', fd)).data;
await ivy.post('/drive/files', { file_id: up.id, name: `Budget ${s}.rtf`, space: 'personal' });

// A stand-in Collabora: discovery, and the editor page that reads the file over WOPI.
let posted: Record<string, string> | null = null;
let fileInfo: any = null;
const collabora = http.createServer(async (req, res) => {
  const u = new URL(req.url!, 'http://127.0.0.1:9988');
  if (u.pathname === '/hosting/discovery') {
    res.writeHead(200, { 'content-type': 'text/xml' });
    return res.end(`<wopi-discovery><net-zone name="external-http"><app name="writer"><action ext="rtf" name="edit" urlsrc="http://127.0.0.1:9988/browser/x/cool.html?"/></app></net-zone></wopi-discovery>`);
  }
  if (u.pathname === '/browser/x/cool.html' && req.method === 'POST') {
    let body = '';
    for await (const c of req) body += c;
    posted = Object.fromEntries(new URLSearchParams(body));
    const src = u.searchParams.get('WOPISrc')!;
    fileInfo = await (await fetch(`${src}?access_token=${encodeURIComponent(posted.access_token)}`)).json();
    res.writeHead(200, { 'content-type': 'text/html' });
    return res.end(`<!doctype html><title>editor</title><p id="doc">Editing ${fileInfo.BaseFileName}</p>`);
  }
  res.writeHead(404).end();
});
await new Promise<void>((r) => collabora.listen(9988, '127.0.0.1', () => r()));

try {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', `ivy.${s}@acme.test`); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');

  await step('a Drive file shows its PDF preview', async () => {
    await p.goto(B + '/drive');
    await p.click(`text=Budget ${s}.rtf`);
    await p.click('dialog button:has-text("Preview")');
    const src = await p.locator('iframe[data-preview]').getAttribute('src', { timeout: 30000 });
    if (!src?.startsWith('blob:')) throw new Error(`preview src ${src}`);
  });

  const office = (await ivy.get('/office/status')).data;
  const editing = office.editing && !office.editable.includes('docx');
  if (!office.editing) console.log('skip editing: the server has no COLLABORA_URL (see README)');
  else if (!editing) console.log('skip editing: the server uses a real Collabora; see ui67');
  else await step('Edit opens the file in the editor with a signed token', async () => {
    await p.keyboard.press('Escape');
    await p.goto(B + '/drive');
    await p.click(`text=Budget ${s}.rtf`);
    await p.click('dialog a:has-text("Edit")');
    await p.waitForURL(/\/drive\/edit\//);
    await p.frameLocator('iframe[data-office]').locator(`#doc:has-text("Editing Budget ${s}.rtf")`).waitFor({ timeout: 15000 });
    if (!posted?.access_token || !posted.access_token_ttl) throw new Error(JSON.stringify(posted));
    if (fileInfo?.UserCanWrite !== true) throw new Error(JSON.stringify(fileInfo));
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  collabora.close();
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
