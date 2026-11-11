// Calendar depth in the browser: an event with a meeting room and a guest by email; the
// guest answers on the invitation page; sharing a calendar as free/busy with one colleague
// allowed to change it, who adds the calendar and puts an event on it; a booking link made,
// booked from its public page and cancelled through the answer link.
import { chromium } from 'playwright';
import pg from 'pg';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail54-${n}-${i}.png`, fullPage: true }); }
};
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
for (const first of ['Lea', 'Max']) await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Cal${s}` });
await admin.post('/booking/resources', { name: `Blue room ${s}`, kind: 'room', capacity: 1, timezone: 'UTC' });

const newPage = async () => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  return p;
};
const ctx = async (email: string) => {
  const p = await newPage();
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const ann = `ann.${s}@example.com`;
const eventTitle = `Board review ${s}`;

try {
  const lea = await ctx(`lea.${s}@acme.test`);
  const max = await ctx(`max.${s}@acme.test`);

  await step('an event with a meeting room and a guest by email', async () => {
    await lea.goto(B + '/calendar');
    await lea.click('button:has-text("New event")');
    await lea.fill('#ev-title', eventTitle);
    await lea.locator(`#ev-room option:has-text("Blue room ${s}")`).waitFor({ state: 'attached' });
    await lea.selectOption('#ev-room', { label: `Blue room ${s}` });
    await lea.fill('#ev-email', `Ann Guest <${ann}>, not-an-address`);
    await lea.click('dialog button:has-text("Add")');
    await lea.locator('[data-email-guests] >> text=Ann Guest').waitFor();
    if ((await lea.inputValue('#ev-email')) !== 'not-an-address') throw new Error('a bad address should stay in the box');
    await lea.fill('#ev-email', '');
    await lea.click('button[form="ev-form"]');
    await lea.locator(`[data-entry="${eventTitle}"]`).first().click();
    await lea.locator('#ev-room').waitFor();
    if ((await lea.locator('#ev-room option:checked').textContent())?.includes(`Blue room ${s}`) !== true) throw new Error('room not kept');
    await lea.locator(`[data-email-guests] >> text=${ann}`).waitFor();
    await lea.click('dialog button:has-text("Cancel")');
  });

  await step('the guest answers on the invitation page', async () => {
    let token = '';
    for (let i = 0; i < 40 && !token; i++) {
      const r = await db.query(`SELECT payload->>'text' AS t FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1`, [ann]);
      token = r.rows[0]?.t.match(/\/rsvp\/([0-9a-f.-]+)/)?.[1] ?? '';
      if (!token) await new Promise((r) => setTimeout(r, 150));
    }
    if (!token) throw new Error('no invitation email');
    const guest = await newPage();
    await guest.goto(`${B}/rsvp/${token}`);
    await guest.locator(`h1:has-text("${eventTitle}")`).waitFor();
    await guest.locator(`text=Blue room ${s}`).waitFor();
    await guest.click('button:has-text("Going")');
    await guest.locator('[data-status="accepted"]').waitFor();
    await lea.reload();
    await lea.locator(`[data-entry="${eventTitle}"]`).first().click();
    await lea.locator(`[data-email-guests] >> text=going`).waitFor();
    await lea.click('dialog button:has-text("Cancel")');
  });

  await step('sharing: free/busy for everyone, Max may change it', async () => {
    await lea.click('button:has-text("Sharing")');
    await lea.selectOption('#cs-all', 'busy');
    await lea.fill('dialog input[placeholder="Add a person…"]', `Max Cal${s}`);
    await lea.click(`dialog button:has-text("Max Cal${s}")`);
    await lea.selectOption(`select[aria-label="Level for Max Cal${s}"]`, 'edit');
    await lea.click('dialog button:has-text("Save")');
    await lea.locator('text=Sharing saved').waitFor();
  });

  await step('Max adds Lea\'s calendar and puts an event on it', async () => {
    await max.goto(B + '/calendar');
    await max.click(`button:has-text("+ Lea Cal${s}")`);
    await max.locator(`[data-entry="${eventTitle}"]`).first().waitFor();
    await max.click('button:has-text("New event")');
    await max.fill('#ev-title', `Prep for Lea ${s}`);
    await max.selectOption('#ev-cal', { label: `Lea Cal${s}` });
    await max.click('button[form="ev-form"]');
    await max.locator(`[data-entry="Prep for Lea ${s}"]`).first().waitFor();
    await lea.reload();
    await lea.locator(`[data-entry="Prep for Lea ${s}"]`).first().waitFor();
  });

  let linkUrl = '';
  await step('a booking link', async () => {
    await lea.click('button:has-text("Booking links")');
    await lea.click('dialog button:has-text("New link")');
    await lea.fill('#sl-title', `Office hours ${s}`);
    for (let i = 0; i < 7; i++) {
      const box = lea.locator('fieldset input[type="checkbox"]').nth(i);
      if (!(await box.isChecked())) await box.check();
    }
    await lea.fill('input[aria-label="Mon from"]', '00:00');
    for (const d of ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']) { await lea.fill(`input[aria-label="${d} from"]`, '00:00'); await lea.fill(`input[aria-label="${d} to"]`, '23:30'); }
    await lea.fill('#sl-notice', '0');
    await lea.click('button[form="sl-form"]');
    const card = lea.locator(`[data-slot-link="Office hours ${s}"]`);
    await card.waitFor();
    linkUrl = (await card.locator('a').getAttribute('href'))!;
    await lea.click('dialog button:has-text("Close")');
  });

  await step('someone books it from the public page, then cancels', async () => {
    const visitor = await newPage();
    await visitor.goto(linkUrl.replace(/^https?:\/\/[^/]+/, B));
    await visitor.locator(`h1:has-text("Office hours ${s}")`).waitFor();
    await visitor.locator('[data-slot]').first().click();
    await visitor.fill('#mt-n', `Vic ${s}`);
    await visitor.fill('#mt-e', `vic.${s}@example.com`);
    await visitor.click('button:has-text("Book")');
    await visitor.locator('[data-done]').waitFor();
    await lea.reload();
    await lea.click('button:has-text("Week")');
    await lea.locator(`[data-entry="Office hours ${s}: Vic ${s}"]`).first().waitFor();
    await visitor.click('a:has-text("See or cancel it")');
    await visitor.locator('[data-status="accepted"]').waitFor();
    await visitor.click('button:has-text("Not going")');
    await visitor.locator('[data-cancelled]').waitFor();
    await lea.reload();
    await lea.waitForTimeout(800);
    if (await lea.locator(`[data-entry="Office hours ${s}: Vic ${s}"]`).count()) throw new Error('the cancelled meeting is still there');
    await lea.click('button:has-text("Month")');
  });

  await step('an app password on the profile works for a calendar app', async () => {
    await lea.goto(B + '/profile');
    await lea.fill('[data-app-passwords] input[aria-label="App name"]', `Phone ${s}`);
    await lea.click('[data-app-passwords] button:has-text("Make a password")');
    const pw = (await lea.locator('[data-password]').textContent())!.trim();
    if (!/^[a-z]{4}(-[a-z]{4}){3}$/.test(pw)) throw new Error(`odd password ${pw}`);
    const r = await fetch(`${B}/dav/`, { method: 'PROPFIND', headers: { depth: '0', authorization: 'Basic ' + Buffer.from(`lea.${s}@acme.test:${pw}`).toString('base64') } });
    if (r.status !== 207) throw new Error(`CalDAV said ${r.status}`);
    await lea.click('[data-new-password] button:has-text("Done")');
    await lea.reload();
    await lea.locator(`[data-app-passwords] >> text=Phone ${s}`).waitFor();
    await lea.locator('[data-app-passwords] >> text=Last used').waitFor();
    lea.once('dialog', (d) => d.accept());
    await lea.click('[data-app-passwords] button:has-text("Revoke")');
    await lea.locator(`[data-app-passwords] >> text=Phone ${s}`).waitFor({ state: 'detached' });
    await lea.goto(B + '/calendar');
    await lea.click('button:has-text("Subscribe")');
    await lea.locator('[data-caldav] >> text=/dav/').waitFor();
    await lea.click('dialog button:has-text("Close")');
  });

  await step('connecting a CalDAV calendar syncs it both ways', async () => {
    const maxApi = new Client();
    await maxApi.post('/auth/login', { email: `max.${s}@acme.test`, password: 'password123' });
    const pw = (await maxApi.post('/auth/app-passwords', { name: `Sync ${s}` })).data.password;
    await maxApi.post('/calendar/events', { title: `Max elsewhere ${s}`, start: new Date(Date.now() + 86400e3).toISOString(), end: new Date(Date.now() + 90000e3).toISOString(), timezone: 'UTC' });
    await lea.goto(B + '/calendar');
    await lea.click('button:has-text("Connected calendars")');
    await lea.click('dialog button:has-text("Connect a CalDAV account")');
    await lea.fill('input[aria-label="Server address"]', `${B}/dav/`);
    await lea.fill('input[aria-label="User name"]', `max.${s}@acme.test`);
    await lea.fill('input[aria-label="Password"]', pw);
    await lea.click('dialog button:has-text("Find calendars")');
    await lea.click('[data-found] button:has-text("Sync this one")');
    await lea.locator('text=Connected and synced').waitFor();
    await lea.locator(`[data-account="max.${s}@acme.test"]`).waitFor();
    await lea.click('dialog button:has-text("Close")');
    await lea.click('button:has-text("Week")');
    await lea.click('button[aria-label="Next"]');
    await lea.click('button:has-text("Today")');
    await lea.locator(`[data-entry="Max elsewhere ${s}"]`).first().waitFor();
    await lea.click('button:has-text("Month")');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
