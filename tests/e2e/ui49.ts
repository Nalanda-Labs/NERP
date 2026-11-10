// Booking in the browser: set up a resource, a service and a form; book from the day schedule
// by clicking an empty time; a client books on the public form and cancels from their link;
// the booking shows on the schedule and in its dialog.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail49-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });

const ctx = async () => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  return p;
};
const ada = await ctx();
await ada.goto(B + '/login'); await ada.fill('#email', 'admin@acme.test'); await ada.fill('#password', 'supersecret1');
await ada.click('button:has-text("Sign in")'); await ada.waitForURL(B + '/');
// A weekday a week ahead.
const d = new Date(); d.setUTCDate(d.getUTCDate() + 7); while ([0, 6].includes(d.getUTCDay())) d.setUTCDate(d.getUTCDate() + 1);
const day = d.toISOString().slice(0, 10);
let formUrl = '';

try {
  await step('set up a room, a service and a form', async () => {
    await ada.goto(B + '/booking/settings');
    await ada.click('button:has-text("Add a resource")');
    await ada.selectOption('#rs-kind', 'room');
    await ada.fill('#rs-name', `Studio ${s}`);
    await ada.fill('#rs-tz', 'UTC');
    await ada.fill('#rs-notice', '0');
    await ada.click('button[form="res-form"]');
    await ada.locator(`[data-res="Studio ${s}"]`).waitFor();
    await ada.click('[role=tab]:has-text("Services")');
    await ada.click('button:has-text("Add a service")');
    await ada.fill('#sv-n', `Photo shoot ${s}`);
    await ada.fill('#sv-min', '60');
    await ada.fill('#sv-p', '80');
    await ada.click('button[form="svc-form"]');
    await ada.locator(`[data-svc="Photo shoot ${s}"]`).waitFor();
    await ada.click('[role=tab]:has-text("Booking forms")');
    await ada.click('button:has-text("New form")');
    await ada.fill('#bf-n', `Studio booking ${s}`);
    await ada.click('button[form="bf-form"]');
    const row = ada.locator(`[data-form="Studio booking ${s}"]`);
    await row.waitFor();
    formUrl = (await row.locator('a:has-text("Open")').getAttribute('href'))!;
  });

  await step('book by clicking an empty time on the schedule', async () => {
    await ada.goto(B + '/booking');
    await ada.fill('input[aria-label="Day"]', day);
    await ada.locator('input[aria-label="Day"]').dispatchEvent('change');
    const lane = ada.locator(`[data-resource="Studio ${s}"] [title="Click to book"]`);
    const box = (await lane.boundingBox())!;
    // 08:00–20:00 is shown; click near 10:00.
    await ada.mouse.click(box.x + box.width * (2.1 / 12), box.y + box.height / 2);
    await ada.selectOption('#bk-svc', { label: new RegExp(`Photo shoot ${s}`) as any }).catch(async () => {
      const opts = await ada.locator('#bk-svc option').allTextContents();
      await ada.selectOption('#bk-svc', { label: opts.find((o) => o.includes(`Photo shoot ${s}`))! });
    });
    await ada.fill('#bk-name', `Walk-in ${s}`);
    await ada.click('button[form="booking-form"]');
    await ada.locator(`[data-tile="Walk-in ${s}"]`).waitFor();
  });

  await step('a client books on the public form', async () => {
    const c = await ctx();
    await c.goto(B + formUrl);
    await c.locator(`h1:has-text("Studio booking ${s}")`).waitFor();
    const opts = await c.locator('#bk-s option').allTextContents();
    await c.selectOption('#bk-s', { label: opts.find((o) => o.includes(`Photo shoot ${s}`))! });
    while ((await c.locator('[data-day]').textContent())!.trim() !== new Date(day + 'T12:00:00Z').toLocaleDateString('en-GB', { weekday: 'long', day: 'numeric', month: 'long' })) {
      await c.click('button[aria-label="Next day"]');
      await c.waitForTimeout(150);
    }
    // 10:00 is taken by the walk-in; 11:00 is free.
    await c.locator('[data-slot="11:00"]').waitFor();
    if (await c.locator('[data-slot="10:00"]').count()) throw new Error('a taken time is offered');
    await c.click('[data-slot="11:00"]');
    await c.fill('#bk-n', `Cleo ${s}`);
    await c.fill('#bk-e', `cleo.${s}@example.com`);
    await c.click('button:has-text("Book 11:00")');
    await c.locator('[data-done]:has-text("You\'re booked")').waitFor();
    await c.click('a:has-text("See, confirm or cancel")');
    await c.locator('[data-status="booked"]').waitFor();
    c.once('dialog', (dlg) => dlg.accept());
    await c.click('button:has-text("Cancel")');
    await c.locator('[data-status="cancelled"]').waitFor();
  });

  await step('the schedule shows it, cancelled', async () => {
    await ada.reload();
    await ada.check('label:has-text("Show cancelled") input');
    await ada.locator(`[data-tile="Cleo ${s}"]`).click();
    await ada.locator('[data-status="cancelled"]').waitFor();
    await ada.click('dialog[open] button:has-text("Close")');
  });

  await step('mark the walk-in as came', async () => {
    await ada.locator(`[data-tile="Walk-in ${s}"]`).click();
    await ada.click('dialog[open] button:has-text("Came")');
    await ada.locator('dialog[open] [data-status="came"]').waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
