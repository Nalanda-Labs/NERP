// Recurring records: which types can repeat, validation, the schedule, "create one now",
// what a copy carries, the background job (due copies, run limits, deleted patterns), a
// responsible person who left, stopping.
import pg from 'pg';
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const db = new pg.Client({ connectionString: dbUrl }); await db.connect();

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const sam = (await admin.post('/users', { email: `sam.${stamp}@acme.test`, password: 'password123', first_name: 'Sam', last_name: 'Rep' })).data;
const cfg = (await admin.get('/crm/config')).data;
const dealType = cfg.types.find((t: any) => t.code === 'deal');
const pipeline = dealType.categories.find((c: any) => c.is_default) ?? dealType.categories[0];
const firstStage = pipeline.stages.filter((s: any) => s.semantics === 'process')[0];
const laterStage = pipeline.stages.filter((s: any) => s.semantics === 'process')[1];
const tz = 'Europe/Berlin';
const today = new Date().toLocaleDateString('en-CA', { timeZone: tz });
const monthly = { freq: 'monthly', interval: 1, weekdays: [], month_day: 1, time: '09:00', timezone: tz, starts_on: today, ends_on: null, max_runs: null };

try {
  const company = (await admin.post('/crm/company/items', { company_name: `Retainer Co ${stamp}` })).data;
  const contact = (await admin.post('/crm/contact/items', { first_name: 'Rae', last_name: `Rep${stamp}` })).data;
  const deal = (await admin.post('/crm/deal/items', { title: `Monthly support ${stamp}`, company_id: company.id, contact_ids: [contact.id], stage_id: laterStage.id, comments: 'Support hours', assigned_to: sam.id })).data;
  await admin.put(`/crm/items/${deal.id}/products`, { rows: [{ name: `Support plan ${stamp}`, price: '500', quantity: '2', discount_kind: 'percent', discount_value: '0', tax_rate: '10' }] });

  // ---- what can repeat ----
  ok('contacts cannot repeat', (await admin.put(`/crm/items/${contact.id}/recurring`, { rule: monthly })).status === 400);
  ok('bad rule rejected', (await admin.put(`/crm/items/${deal.id}/recurring`, { rule: { ...monthly, month_day: 40 } })).status === 400);
  ok('bad days rejected', (await admin.put(`/crm/items/${deal.id}/recurring`, { rule: monthly, due_days: -1 })).status === 400);
  ok('ended schedule rejected', (await admin.put(`/crm/items/${deal.id}/recurring`, { rule: { ...monthly, starts_on: '2020-01-01', ends_on: '2020-02-01' } })).status === 400);
  ok('not repeating yet', (await admin.get(`/crm/items/${deal.id}/recurring`)).data.rule === null);
  ok('no copy without a schedule', (await admin.post(`/crm/items/${deal.id}/recurring/now`, {})).status === 400);

  let r = await admin.put(`/crm/items/${deal.id}/recurring`, { rule: monthly, due_days: 14 });
  ok('schedule set', r.status === 200 && r.data.summary.startsWith('Every month') && r.data.is_active && r.data.run_count === 0 && new Date(r.data.next_run_at) > new Date(), r.data);

  // ---- a copy ----
  r = await admin.post(`/crm/items/${deal.id}/recurring/now`, {});
  ok('create one now', r.status === 200 && r.data.code === 'deal', r.data);
  const copy = (await admin.get(`/crm/deal/items/${r.data.id}`)).data;
  const due = new Date(Date.now() + 14 * 86400000).toLocaleDateString('en-CA', { timeZone: tz });
  ok('copy carries the record', copy.title === `Monthly support ${stamp}` && !!company.id && copy.company_id === company.id && copy.contacts[0]?.id === contact.id && copy.comments === 'Support hours' && copy.assigned_to === sam.id, copy);
  ok('copy starts in the first stage, dated ahead', copy.stage_id === firstStage.id && copy.close_date === due && copy.source_item_id === deal.id, [copy.stage_id, copy.close_date, due]);
  const rows = (await admin.get(`/crm/items/${copy.id}/products`)).data;
  ok('copy has the products and amount', rows.rows.length === 1 && Number(rows.totals.total) === 1100 && Number(copy.opportunity) === 1100, rows);
  ok('the pattern lists it', (await admin.get(`/crm/deal/items/${deal.id}`)).data.children.some((c: any) => c.id === copy.id));
  ok('schedule untouched by an extra copy', (await admin.get(`/crm/items/${deal.id}/recurring`)).data.run_count === 0);
  const tl = (await admin.get(`/crm/items/${deal.id}/timeline`)).data.items;
  ok('timeline entries', tl.some((e: any) => e.kind === 'recurring' && e.data.event === 'set') && tl.some((e: any) => e.kind === 'recurring' && e.data.event === 'made' && e.data.item_id === copy.id), tl.map((e: any) => e.kind));

  // ---- the job ----
  const copies = async (id: string) => (await admin.get(`/crm/deal/items/${id}`)).data.children.length;
  await db.query("UPDATE crm_recurring SET next_run_at = now() - interval '1 minute' WHERE item_id = $1", [deal.id]);
  const runs = await waitFor(async () => { const v = (await admin.get(`/crm/items/${deal.id}/recurring`)).data; return v.run_count === 1 ? v : null; }, 90000);
  ok('due copy made by the job', runs && (await copies(deal.id)) === 2 && runs.is_active && new Date(runs.next_run_at) > new Date(), runs);

  // A limit of one run: the schedule finishes.
  const once = (await admin.post('/crm/deal/items', { title: `One-off ${stamp}` })).data;
  await admin.put(`/crm/items/${once.id}/recurring`, { rule: { ...monthly, max_runs: 1 } });
  // A deleted pattern does nothing.
  const gone = (await admin.post('/crm/deal/items', { title: `Gone ${stamp}` })).data;
  await admin.put(`/crm/items/${gone.id}/recurring`, { rule: monthly });
  await admin.req('DELETE', `/crm/deal/items/${gone.id}`);
  await db.query("UPDATE crm_recurring SET next_run_at = now() - interval '1 minute' WHERE item_id = ANY($1)", [[once.id, gone.id]]);
  const fin = await waitFor(async () => { const v = (await admin.get(`/crm/items/${once.id}/recurring`)).data; return v.run_count === 1 ? v : null; }, 90000);
  ok('run limit finishes the schedule', fin && !fin.is_active && fin.next_run_at === null && (await copies(once.id)) === 1, fin);
  const g = (await db.query('SELECT run_count FROM crm_recurring WHERE item_id = $1', [gone.id])).rows[0];
  ok('deleted pattern skipped', g?.run_count === 0, g);

  // ---- a responsible person who left ----
  await admin.patch(`/users/${sam.id}`, { is_active: false });
  r = await admin.post(`/crm/items/${deal.id}/recurring/now`, {});
  ok('copy of someone who left has no responsible person', r.status === 200 && (await admin.get(`/crm/deal/items/${r.data.id}`)).data.assigned_to === null, r.data);

  // ---- stopping ----
  r = await admin.req('DELETE', `/crm/items/${deal.id}/recurring`);
  ok('stopped', r.status === 200 && r.data.rule === null && (await copies(deal.id)) === 3);
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
