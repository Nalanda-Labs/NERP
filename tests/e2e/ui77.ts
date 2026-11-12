// Booking inside CRM in the browser: booking from a deal's card (a free time, the deal's
// client) and seeing it listed there; a website's booking block showing the booking form.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { OUTPUT } from './output.ts';
import pg from 'pg';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail77-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const room = (await admin.post('/booking/resources', { name: `Studio ${s}`, kind: 'room', capacity: 5, timezone: 'UTC', min_notice_minutes: 0 })).data;
await admin.put(`/booking/resources/${room.id}/hours`, { hours: [1, 2, 3, 4, 5, 6, 7].map((weekday) => ({ weekday, start_min: 0, end_min: 1440 })) });
const svc = (await admin.post('/booking/services', { name: `Lesson ${s}`, duration_minutes: 30, price: '25.00', resource_ids: [room.id] })).data.items.find((x: any) => x.name === `Lesson ${s}`);
const form = (await admin.post('/booking/forms', { name: `Lessons ${s}`, service_ids: [svc.id] })).data.items.find((f: any) => f.name === `Lessons ${s}`);
const kim = (await admin.post('/crm/contact/items', { first_name: 'Kim', last_name: `Ui${s}`, multifields: [{ kind: 'email', value_type: 'work', value: `kim.${s}@example.com` }] })).data;
const deal = (await admin.post('/crm/deal/items', { title: `Lessons deal ${s}`, contact_ids: [kim.id] })).data;
const site = (await admin.post('/sites', { name: `Studio site ${s}`, slug: `studio-${s}` })).data;
await admin.patch(`/sites/pages/${site.page_list[0].id}`, { blocks: [{ kind: 'booking', heading: `Book a lesson ${s}`, booking_slug: form.slug }] });
await admin.post(`/sites/${site.id}/publish`, {});

const ctx = await browser.newContext({ viewport: { width: 1300, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text() + ' @ ' + pa.url()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('booking from the deal\'s card', async () => {
    await pa.goto(`${B}/crm/deal/${deal.id}`);
    const card = pa.locator('[data-bookings-card]');
    await card.waitFor();
    await card.locator('[data-book]').click();
    const f = card.locator('[data-book-form]');
    await f.locator('select[aria-label="Resource"]').selectOption(room.id);
    await f.locator('select[aria-label="Service"]').selectOption(svc.id);
    // Tomorrow, so the day surely has free times left.
    await f.locator('input[type="date"]').fill(new Date(Date.now() + 86400e3).toISOString().slice(0, 10));
    await f.locator('input[type="date"]').dispatchEvent('change');
    await f.locator('[data-slot]').nth(20).click();
    await f.locator('[data-book-submit]').click();
    await card.locator('[data-booking]').first().waitFor({ timeout: 10000 });
    const b = (await admin.get(`/crm/items/${deal.id}/bookings`)).data.items[0];
    if (b?.deal_id !== deal.id || b.contact_id !== kim.id || b.service_name !== `Lesson ${s}`) throw new Error(JSON.stringify(b).slice(0, 300));
  });

  await step('the contact shows it too', async () => {
    await pa.goto(`${B}/crm/contact/${kim.id}`);
    await pa.locator('[data-bookings-card] [data-booking]').first().waitFor();
  });

  await step('the website\'s booking block shows the form', async () => {
    await pa.goto(`${B}/s/studio-${s}`);
    await pa.locator(`[data-block="booking"] >> text=Book a lesson ${s}`).waitFor();
    const frame = pa.frameLocator('[data-block="booking"] iframe');
    await frame.locator(`text=Lessons ${s}`).first().waitFor({ timeout: 15000 });
    if (!(await frame.locator(`option:has-text("Lesson ${s}")`).count())) throw new Error("the service is not offered");
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.del(`/sites/${site.id}`).catch(() => {});
  await db.query("DELETE FROM bookings WHERE client_name LIKE $1", [`%${s}`]).catch(() => {});
  await db.query("DELETE FROM booking_forms WHERE name = $1", [`Lessons ${s}`]).catch(() => {});
  await db.query("DELETE FROM booking_services WHERE name = $1", [`Lesson ${s}`]).catch(() => {});
  await db.query("DELETE FROM booking_resources WHERE name = $1", [`Studio ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
