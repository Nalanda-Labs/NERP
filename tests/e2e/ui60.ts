// People in the browser: reacting to a post with an emoji; giving a colleague a badge, which
// shows in the feed and on their profile; online status and the profile's tabs; exporting
// the employee list; the search box finding a person and a task.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail60-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const ids: Record<string, string> = {};
for (const first of ['Ivo', 'Zia']) ids[first] = (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Crew${s}` })).data.id;
const zia = new Client();
await zia.post('/auth/login', { email: `zia.${s}@acme.test`, password: 'password123' });
// Ivo asks Zia to do it, so he may see the task.
const ivoApi = new Client();
await ivoApi.post('/auth/login', { email: `ivo.${s}@acme.test`, password: 'password123' });
await ivoApi.post('/tasks', { title: `Repaint the lobby ${s}`, responsible_id: ids.Zia });
await zia.post('/feed', { body: `We moved offices ${s}` });

const ctx = async (email: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB', acceptDownloads: true })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};

try {
  const ivo = await ctx(`ivo.${s}@acme.test`);

  await step('react to a post with an emoji', async () => {
    await ivo.goto(B + '/feed');
    const post = ivo.locator(`article:has-text("We moved offices ${s}")`).first();
    await post.locator('button[aria-label="More reactions"]').click();
    await post.locator('button[aria-label="clap"]').click();
    await post.locator('[data-reactions]:has-text("👏 1")').waitFor();
  });

  await step('give Zia a badge', async () => {
    await ivo.fill('textarea[aria-label="Post"]', `Thanks for the move, Zia ${s}`);
    await ivo.click('button:has-text("Give a badge")');
    await ivo.click('[aria-label="Which badge"] button:has-text("Team player")');
    await ivo.fill('[aria-label="Badge"] input', `Zia Crew${s}`);
    await ivo.click(`button:has-text("Zia Crew${s}")`);
    await ivo.click('form[aria-label="New post"] button[type="submit"], form[aria-label="New post"] button:has-text("Post")');
    await ivo.locator('[data-badge="team_player"]:has-text("Zia")').first().waitFor();
  });

  await step('Zia\'s profile: online, the badge, her tasks', async () => {
    await ivo.goto(`${B}/employees/${ids.Zia}`);
    await ivo.locator('[data-presence]:has-text("Online")').waitFor();
    await ivo.locator('[data-badges] >> text=Team player').first().waitFor();
    await ivo.locator(`[data-profile-tab="tasks"] >> text=Repaint the lobby ${s}`).waitFor();
    await ivo.click('[role="tab"]:has-text("Documents")');
    await ivo.locator('[data-profile-tab="documents"] >> text=Only the person').waitFor();
  });

  await step('export the employee list', async () => {
    await ivo.goto(B + '/employees');
    const [dl] = await Promise.all([ivo.waitForEvent('download'), ivo.click('button:has-text("Export CSV")')]);
    if (dl.suggestedFilename() !== 'employees.csv') throw new Error(dl.suggestedFilename());
  });

  await step('the search box finds people and tasks', async () => {
    await ivo.fill('input[aria-label="Search everything"]', `Crew${s}`);
    await ivo.locator(`[data-search-results] >> text=Zia Crew${s}`).waitFor();
    await ivo.fill('input[aria-label="Search everything"]', `Repaint the lobby ${s}`);
    await ivo.locator(`[data-search-results] button:has-text("Repaint the lobby ${s}")`).waitFor();
    await ivo.keyboard.press('Enter');
    await ivo.waitForURL(/\/tasks\/[0-9a-f-]+$/);
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
