// Collaboration in the browser: two people editing a knowledge base page together in the
// visual editor (each sees the other's text and name as they type; saving keeps Markdown),
// importing pages into a space, and the employee list as vCards.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { makeZip } from './zip.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail72-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
let spaceId = '';

const signIn = async (email: string, password: string) => {
  const ctx = await browser.newContext({ viewport: { width: 1300, height: 900 }, timezoneId: 'UTC', locale: 'en-GB', acceptDownloads: true });
  await ctx.addInitScript(() => localStorage.setItem('nerp:kb-editor', 'visual'));
  const p = await ctx.newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/400|401|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};

try {
  for (const first of ['Ann', 'Bob']) await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Live${s}` });
  const users = (await admin.get(`/users?q=live${s}`)).data.items ?? (await admin.get(`/users?q=live${s}`)).data;
  const space = (await admin.post('/kb/spaces', { name: `Team ${s}` })).data;
  spaceId = space.id;
  await admin.patch(`/kb/spaces/${space.id}`, { to_all: true, editors: { users: users.map((u: any) => u.id), departments: [] } });
  const page = (await admin.post(`/kb/spaces/${space.id}/pages`, { title: `Plan ${s}`, body: 'The **plan** so far.' })).data;

  const pa = await signIn(`ann.${s}@acme.test`, 'password123');
  const pb = await signIn(`bob.${s}@acme.test`, 'password123');

  await step('the visual editor shows the saved page formatted', async () => {
    await pa.goto(B + `/kb/page/${page.id}?edit=1`);
    const ed = pa.locator('[data-visual-editor]');
    await ed.locator('strong:has-text("plan")').waitFor({ timeout: 15000 });
  });

  await step('two people edit at once and see each other', async () => {
    await pb.goto(B + `/kb/page/${page.id}?edit=1`);
    const eb = pb.locator('[data-visual-editor]');
    await eb.locator('strong:has-text("plan")').waitFor({ timeout: 15000 });
    // Ann types at the end; Bob sees it.
    const ea = pa.locator('[data-visual-editor]');
    await ea.locator('p').last().click();
    await pa.keyboard.press('End');
    await pa.keyboard.type(' Ann adds this.');
    await eb.locator('text=Ann adds this.').waitFor({ timeout: 15000 });
    // Bob types a new line (once Ann's last keystrokes and cursor have arrived); Ann sees it.
    await pb.waitForTimeout(500);
    await eb.locator('p', { hasText: 'Ann adds this.' }).click();
    await pb.keyboard.press('End');
    await pb.keyboard.press('Enter');
    await pb.keyboard.type('Bob was here');
    await ea.locator('text=Bob was here').waitFor({ timeout: 15000 });
    // Each sees the other's name.
    await pa.locator(`[data-co-editors] >> text=Bob Live${s}`).waitFor({ timeout: 15000 });
    await pb.locator(`[data-co-editors] >> text=Ann Live${s}`).waitFor({ timeout: 15000 });
  });

  await step('saving keeps the page as Markdown, with both people\'s text', async () => {
    await pa.click('button:has-text("Save")');
    await pa.waitForURL(new RegExp(`/kb/page/${page.id}$`));
    const saved = (await admin.get(`/kb/pages/${page.id}`)).data;
    if (!/The \*\*plan\*\* so far\. Ann adds this\.\s+Bob was here/.test(saved.body)) throw new Error(saved.body);
    await pa.locator('[data-markdown] strong:has-text("plan")').waitFor();
  });

  await step('formatting from the toolbar', async () => {
    await pb.locator('[data-visual-editor] p', { hasText: 'Bob was here' }).click();
    await pb.keyboard.press('End');
    await pb.keyboard.press('Enter');
    await pb.click('button[aria-label="Heading 2"]');
    await pb.waitForTimeout(200);
    await pb.keyboard.type(`Next steps ${s}`);
    await pb.click('button:has-text("Save")');
    await pb.waitForURL(new RegExp(`/kb/page/${page.id}$`));
    const saved = (await admin.get(`/kb/pages/${page.id}`)).data.body;
    if (!new RegExp(`Bob was here\\s+## Next steps ${s}`).test(saved)) throw new Error(saved);
  });

  await step('importing pages into a space', async () => {
    await pa.goto(B + `/kb/${space.id}`);
    const zip = makeZip({ 'Docs/Welcome.md': `# Welcome ${s}\n\nHello.`, 'Docs/Guides/Setup.md': '# Setup\n\nSteps.' });
    await pa.locator('[data-kb-import] input[type="file"]').setInputFiles({ name: 'docs.zip', mimeType: 'application/zip', buffer: Buffer.from(zip) });
    await pa.locator('text=/3 pages imported/').waitFor({ timeout: 15000 });
    await pa.locator(`text=Welcome ${s}`).first().waitFor();
  });

  await step('the employee list as vCards', async () => {
    await pa.goto(B + '/employees');
    const [dl] = await Promise.all([pa.waitForEvent('download'), pa.click('[data-export-vcf]')]);
    const text = await (await dl.createReadStream()).toArray().then((c) => Buffer.concat(c).toString());
    if (!text.includes(`FN:Bob Live${s}`)) throw new Error(text.slice(0, 200));
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  if (spaceId) await admin.del(`/kb/spaces/${spaceId}`);
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
