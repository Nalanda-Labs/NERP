// Editing with the real Collabora Online (tests/e2e/tools/collabora-up.sh, the official image)
// and the server started with COLLABORA_URL=http://127.0.0.1:9980: a text document, an Excel
// workbook and a PowerPoint deck open in the editor; what is typed is saved back through WOPI
// as a new drive version (on Ctrl+S, and when the last person leaves); two people edit one
// file at once and both changes are kept; a colleague with view access cannot save.
// Skips when the server has no Collabora. Makes the Office files with LibreOffice (`soffice`).
import { chromium } from 'playwright';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const status = (await admin.get('/office/status')).data;
if (!status.editing || !status.editable.includes('docx') || !(await fetch('http://127.0.0.1:9980/hosting/discovery').then((r) => r.ok, () => false))) {
  console.log('skip: needs Collabora on :9980 and the server started with COLLABORA_URL=http://127.0.0.1:9980');
  process.exit(0);
}

// ---- files: made with LibreOffice, read back with it ----
const work = mkdtempSync(join(tmpdir(), 'nerp-ui67-'));
const soffice = (file: string, to: string) => {
  execFileSync('soffice', [`-env:UserInstallation=file://${work}/profile`, '--headless', '--convert-to', to, '--outdir', work, file], { stdio: 'ignore', timeout: 120000 });
};
writeFileSync(join(work, 'budget.csv'), 'Item,Amount\nRent,1200\n');
soffice(join(work, 'budget.csv'), 'xlsx');
const FODP = `<?xml version="1.0" encoding="UTF-8"?><office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.presentation"><office:body><office:presentation><draw:page draw:name="page1"><draw:frame svg:x="2cm" svg:y="2cm" svg:width="20cm" svg:height="3cm"><draw:text-box><text:p>Roadmap</text:p></draw:text-box></draw:frame></draw:page></office:presentation></office:body></office:document>`;
writeFileSync(join(work, 'deck.fodp'), FODP);
soffice(join(work, 'deck.fodp'), 'pptx');
/** The text of an Office file, through LibreOffice. */
const textOf = (bytes: Uint8Array, ext: string, to: string) => {
  const f = join(work, `back-${Date.now()}.${ext}`);
  writeFileSync(f, bytes);
  soffice(f, to);
  return readFileSync(f.replace(/\.[a-z]+$/, `.${to.split(':')[0]}`), 'utf8');
};

const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${s}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Co${s}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return { c, email };
};
const ann = await user('Ann'), bob = await user('Bob'), cat = await user('Cat');
const toDrive = async (name: string, bytes: string | Uint8Array, type: string) => {
  const fd = new FormData();
  fd.append('file', new Blob([bytes as BlobPart], { type }), name);
  const file = (await ann.c.req('POST', '/files', fd)).data;
  return (await ann.c.post('/drive/files', { file_id: file.id, name, space: 'personal' })).data;
};
const RTF = '{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Arial;}}\\f0\\fs24 Quarterly plan\\par}';
const doc = await toDrive(`Plan ${s}.rtf`, RTF, 'application/rtf');
const sheet = await toDrive(`Budget ${s}.xlsx`, readFileSync(join(work, 'budget.xlsx')), 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet');
const deck = await toDrive(`Deck ${s}.pptx`, readFileSync(join(work, 'deck.pptx')), 'application/vnd.openxmlformats-officedocument.presentationml.presentation');
await ann.c.put(`/drive/nodes/${doc.id}/shares`, [{ user_id: bob.c.id, level: 'view' }, { user_id: cat.c.id, level: 'edit' }]);
const version = async (id: string) => (await ann.c.get(`/drive/nodes/${id}`)).data.version as number;
const download = async (id: string) => new Uint8Array(await (await ann.c.req('GET', `/drive/nodes/${id}/download`, undefined, true)).arrayBuffer());
const waitVersion = async (id: string, min: number) => {
  for (let i = 0; i < 60; i++) {
    if ((await version(id)) >= min) return;
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(`still version ${await version(id)}, wanted ${min}`);
};

const browser = await chromium.launch({ executablePath: exe });
let failed = 0, n = 0;
const open: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); for (const [i, pg] of open.entries()) await pg.screenshot({ path: `fail67-${++n}-${i}.png` }).catch(() => {}); }
};
const signIn = async (email: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  open.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const editor = (p: import('playwright').Page) => p.frameLocator('iframe[data-office]');
/** Opens a drive file in Collabora and waits until the document is drawn. */
const openFile = async (p: import('playwright').Page, id: string, view = false) => {
  await p.goto(B + `/drive/edit/${id}${view ? '?view=1' : ''}`);
  await editor(p).locator('#document-container canvas, canvas#document-canvas').first().waitFor({ timeout: 90000 });
  await p.waitForTimeout(3000);
  // The first-run tour, when it shows.
  const skip = editor(p).locator('button:has-text("Skip")');
  if (await skip.isVisible().catch(() => false)) await skip.click();
};
const focusDoc = (p: import('playwright').Page) => editor(p).locator('#document-container').click({ position: { x: 300, y: 90 }, force: true });

try {
  const pa = await signIn(ann.email);

  await step('a text document: typed text is saved as a new version on Ctrl+S', async () => {
    await openFile(pa, doc.id);
    await focusDoc(pa);
    await pa.keyboard.press('Control+End');
    await pa.keyboard.type(` revised ${s}`, { delay: 30 });
    await pa.waitForTimeout(1000);
    await pa.keyboard.press('Control+s');
    await waitVersion(doc.id, 2);
    const text = new TextDecoder().decode(await download(doc.id));
    if (!text.includes(`revised ${s}`)) throw new Error(text.slice(0, 400));
  });

  await step('two people edit it at once and both changes are kept', async () => {
    const pc = await signIn(cat.email);
    await openFile(pc, doc.id);
    // Each adds a line of their own, Ann at the end and Cat at the start.
    await focusDoc(pa);
    await pa.keyboard.press('Control+End');
    await pa.keyboard.press('Enter');
    await pa.keyboard.type(`from Ann ${s}`, { delay: 30 });
    await focusDoc(pc);
    await pc.keyboard.press('Control+Home');
    await pc.keyboard.type(`from Cat ${s} `, { delay: 30 });
    await pc.waitForTimeout(1500);
    const v = await version(doc.id);
    await pc.keyboard.press('Control+s');
    await waitVersion(doc.id, v + 1);
    // RTF splits words with formatting codes: read the text through LibreOffice.
    // (Autocorrect capitalises a sentence's first word.)
    const text = textOf(await download(doc.id), 'rtf', 'txt:Text').toLowerCase();
    if (!text.includes(`from ann ${s}`) || !text.includes(`from cat ${s}`)) throw new Error(text.slice(0, 500));
    await pc.context().close();
    open.splice(open.indexOf(pc), 1);
  });

  await step('a spreadsheet (xlsx): a cell typed in is saved', async () => {
    await openFile(pa, sheet.id);
    const name = editor(pa).locator('#addressInput input, input#addressInput, #addressInput');
    await name.first().click();
    await pa.keyboard.press('Control+a');
    await pa.keyboard.type('A4');
    await pa.keyboard.press('Enter');
    await pa.waitForTimeout(500);
    await pa.keyboard.type(`Travel ${s}`);
    await pa.keyboard.press('Enter');
    await pa.waitForTimeout(1000);
    await pa.keyboard.press('Control+s');
    await waitVersion(sheet.id, 2);
    const csv = textOf(await download(sheet.id), 'xlsx', 'csv');
    if (!csv.includes(`Travel ${s}`) || !csv.includes('Rent')) throw new Error(csv);
  });

  await step('a presentation (pptx): saved when the last person leaves', async () => {
    await openFile(pa, deck.id);
    // Into the text box on the slide: the document area's centre-left, then type.
    const box = await editor(pa).locator('#document-container').boundingBox();
    if (!box) throw new Error('no document');
    await pa.waitForTimeout(1000);
    await editor(pa).locator('#document-container').dblclick({ position: { x: box.width * 0.3, y: box.height * 0.22 }, force: true });
    await pa.keyboard.press('Control+End');
    await pa.keyboard.type(` 2027 ${s}`, { delay: 30 });
    await pa.waitForTimeout(1000);
    await pa.keyboard.press('Escape');
    await pa.waitForTimeout(1000);
    await pa.goto(B + '/drive');
    await waitVersion(deck.id, 2);
    const text = textOf(await download(deck.id), 'pptx', 'pdf');
    void text; // a PDF: only that it converts; the words are checked below
    const odp = textOf(await download(deck.id), 'pptx', 'fodp');
    if (!odp.includes(`2027 ${s}`)) throw new Error(odp.slice(-800));
  });

  await step('a colleague with view access cannot save', async () => {
    const pb = await signIn(bob.email);
    await openFile(pb, doc.id, true);
    await pb.locator('text=View only').waitFor();
    const before = await version(doc.id);
    await focusDoc(pb);
    await pb.keyboard.type('should not stick');
    await pb.keyboard.press('Control+s');
    await pb.waitForTimeout(4000);
    await pb.goto(B + '/drive');
    await pb.waitForTimeout(3000);
    const after = await version(doc.id);
    if (after !== before) throw new Error(`version ${before} → ${after}`);
  });
} finally {
  await browser.close();
  rmSync(work, { recursive: true, force: true });
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
