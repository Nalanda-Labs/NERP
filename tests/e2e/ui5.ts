import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { console.log('FAIL', name, e.message.split('\n')[0]); await page.screenshot({ path: `fail-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('CRM nav + deal kanban', async () => {
  await page.click('nav >> text=Deals'); await page.waitForURL(/\/crm\/deal$/);
  await page.waitForSelector('section h3:text-is("New")'); await page.waitForSelector('section h3:text-is("Won")');
  await page.screenshot({ path: '01-kanban.png' });
});
await step('create company with address', async () => {
  await page.goto(B + '/crm/company/new');
  await page.fill('#crm-cname', `Initech ${s}`);
  await page.selectOption('#crm-ind', 'it');
  await page.click('text=More fields');
  await page.fill('input[aria-label="City"]', 'Bengaluru'); await page.selectOption('select[aria-label="Country"]', 'IN');
  await page.click('button:has-text("Create company")');
  await page.waitForSelector(`h1:has-text("Initech ${s}")`); await page.waitForSelector('text=Bengaluru');
  await page.waitForSelector('text=India');
});
const companyUrl = page.url();
await step('create contact; duplicate warning; company picker', async () => {
  await page.goto(B + '/crm/contact/new');
  await page.fill('#crm-first', 'Peter'); await page.fill('#crm-last', `Gibbons${s}`);
  await page.click('text=+ Add phone'); await page.fill('input[type=tel]', '+91 98765 43210');
  await page.click('text=+ Add email'); await page.fill('input[type=email]', `peter.${s}@initech.example`);
  await page.fill('input[placeholder="Find a company…"]', `Initech ${s}`); await page.click(`ul >> button:has-text("Initech ${s}")`);
  await page.screenshot({ path: '02-contact-form.png', fullPage: true });
  await page.click('button:has-text("Create contact")');
  await page.waitForSelector(`h1:has-text("Peter Gibbons${s}")`);
  // second contact with same phone shows duplicate warning
  await page.goto(B + '/crm/contact/new');
  await page.click('text=+ Add phone'); await page.fill('input[type=tel]', '098765 43210');
  await page.waitForSelector('text=Already in the CRM?');
  await page.waitForSelector(`a:has-text("Peter Gibbons${s}")`);
});
let dealUrl;
await step('create deal in EUR with contact + company', async () => {
  await page.goto(B + '/crm/deal/new');
  await page.fill('#crm-title', `TPS reports ${s}`);
  await page.fill('#crm-amount', '2500');
  await page.fill('input[placeholder="Find a company…"]', `Initech ${s}`); await page.click(`ul >> button:has-text("Initech ${s}")`);
  await page.fill('input[placeholder="Add a contact…"]', `Gibbons${s}`); await page.click(`ul >> button:has-text("Gibbons${s}")`);
  await page.click('button:has-text("Create deal")');
  await page.waitForSelector(`h1:has-text("TPS reports ${s}")`);
  dealUrl = page.url();
});
await step('stage bar + products + timeline composer', async () => {
  await page.click('button[title="Move to Proposal sent"]'); await page.waitForSelector('text=moved it from New to Proposal sent');
  await page.click('button:has-text("Add products")');
  await page.click('button:has-text("Add custom row")');
  await page.fill('input[aria-label="Product name"]', 'Consulting days');
  await page.fill('input[aria-label="Price"]', '800'); await page.fill('input[aria-label="Quantity"]', '3'); await page.fill('input[aria-label="Tax rate"]', '18');
  await page.click('button:has-text("Save products")'); await page.waitForSelector('text=Products saved');
  await page.waitForSelector('dd:has-text("2,832")');
  await page.fill('textarea[placeholder="Leave a comment…"]', 'Budget approved by Lumbergh'); await page.click('button:has-text("Post")');
  await page.waitForSelector('text=Budget approved by Lumbergh');
  await page.click('button:has-text("To-do")'); await page.fill('textarea[placeholder="What needs to be done?"]', 'Send contract');
  await page.fill('input[aria-label="Deadline"]', '2030-01-15T10:00'); await page.click('button:has-text("Save")');
  await page.waitForSelector('section:has(h2:has-text("Planned")) >> text=Send contract');
  await page.screenshot({ path: '03-deal.png', fullPage: true });
});
await step('kanban drag to Won', async () => {
  await page.goto(B + '/crm/deal');
  await page.waitForSelector(`a:has-text("TPS reports ${s}")`);
  await page.locator(`a:has-text("TPS reports ${s}")`).dragTo(page.locator('section:has(h3:text-is("Negotiation"))'));
  await page.waitForSelector(`section:has(h3:text-is("Negotiation")) >> text=TPS reports ${s}`);
  await sleep(600); await page.screenshot({ path: '04-kanban-moved.png' });
});
await step('list view + filter + bulk select', async () => {
  await page.click('button[aria-label="List"]');
  await page.fill('input[placeholder^="Search name"]', `tps reports ${s}`);
  await page.waitForSelector(`td >> text=TPS reports ${s}`);
  await page.check('tbody input[type=checkbox] >> nth=0'); await page.waitForSelector('text=1 selected');
  await page.screenshot({ path: '05-list.png' });
});
await step('lead: create and convert', async () => {
  await page.goto(B + '/crm/lead/new');
  await page.fill('#crm-title', `Web enquiry ${s}`); await page.fill('#crm-first', 'Milton'); await page.fill('#crm-last', `Waddams${s}`); await page.fill('#crm-cname', `Stapler Co ${s}`);
  await page.click('button:has-text("Create lead")'); await page.waitForSelector(`h1:has-text("Web enquiry ${s}")`);
  await page.click('button:has-text("Convert")');
  await page.check('dialog label:has-text("Company") input');
  await page.click('dialog button:has-text("Convert")');
  await page.waitForURL(/\/crm\/deal\//); await page.waitForSelector(`text=Created from`);
});
await step('settings: add stage + custom field; access matrix', async () => {
  await page.goto(B + '/crm/settings');
  await page.fill('input[placeholder="New stage"] >> nth=0', `Legal review ${s}`); await page.click('button:has-text("Add stage") >> nth=0');
  await page.waitForFunction((v) => [...document.querySelectorAll<HTMLInputElement>('input[aria-label="Stage name"]')].some((i) => i.value === v), `Legal review ${s}`);
  await page.click('a[href="?tab=fields"]');
  await page.fill('#nf-l', `Segment ${s}`); await page.selectOption('#nf-k', 'list'); await page.fill('#nf-o', 'SMB\nEnterprise');
  await page.click('button:has-text("Add field")');
  await page.waitForFunction((v) => [...document.querySelectorAll<HTMLInputElement>('input[aria-label="Label"]')].some((i) => i.value === v), `Segment ${s}`);
  await page.click('a[href="?tab=currencies"]'); await page.waitForSelector('text=Base currency');
  await page.click('a[href="?tab=access"]'); await page.waitForSelector('select[aria-label="Deals Edit"]');
  await page.screenshot({ path: '06-access.png' });
});
await step('import wizard', async () => {
  await page.goto(B + '/crm/contact/import');
  const csv = `First name,Last name,Email,Country\nSamir,Nag${s},samir.${s}@example.com,IN\nMichael,Bolton${s},michael.${s}@example.com,US\n`;
  await page.setInputFiles('input[type=file]', { name: `people-${s}.csv`, mimeType: 'text/csv', buffer: Buffer.from(csv) });
  await page.waitForSelector(`text=people-${s}.csv`);
  await page.screenshot({ path: '07-import-map.png' });
  await page.click('button:has-text("Start import")');
  await page.waitForSelector('text=Import finished', { timeout: 20000 }); await page.waitForSelector('text=2 created');
});
await step('activities, products, recycle bin pages', async () => {
  await page.goto(B + '/crm/activities'); await page.waitForSelector('text=Send contract');
  await page.goto(B + '/crm/products'); await page.waitForSelector('h1:has-text("Products")');
  await page.goto(companyUrl); await page.click('button[aria-label="Delete"]').catch(() => {});
});
{ // Remove the stage this test added to the default pipeline.
  const tok = (await (await fetch(B + '/api/auth/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: 'admin@acme.test', password: 'supersecret1' }) })).json()).access_token;
  const cfg = await (await fetch(B + '/api/crm/config', { headers: { authorization: `Bearer ${tok}` } })).json();
  for (const c of cfg.types.find((t) => t.code === 'deal').categories) for (const st of c.stages) if (st.name === `Legal review ${s}`) {
    const r = await fetch(B + `/api/crm/stages/${st.id}`, { method: 'DELETE', headers: { authorization: `Bearer ${tok}`, 'content-type': 'application/json' }, body: '{}' });
    if (r.status !== 200) errors.push('cleanup: stage not removed ' + r.status);
  }
  // ... and the custom field.
  const fields = await (await fetch(B + '/api/crm/deal/fields', { headers: { authorization: `Bearer ${tok}` } })).json();
  for (const f of fields) if (f.label === `Segment ${s}`) {
    const r = await fetch(B + `/api/crm/deal/fields/${f.id}`, { method: 'DELETE', headers: { authorization: `Bearer ${tok}` } });
    if (r.status !== 200) errors.push('cleanup: field not removed ' + r.status);
  }
}
console.log('errors:', errors.length ? errors : 'none');
await browser.close();
await cleanupRun(s);
