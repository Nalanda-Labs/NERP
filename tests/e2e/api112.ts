// Promise checker: can a deal be delivered by a date — stock (less other open deals due by then,
// plus receipts expected) and the free working time of the people doing the work (working
// days less time off, meetings and tasks due).
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const tz = (await db.query("SELECT value #>> '{}' AS v FROM settings WHERE key = 'default_timezone'")).rows[0]?.v ?? 'UTC';
const today = new Intl.DateTimeFormat('en-CA', { timeZone: tz }).format(new Date());
const day = (n: number) => { const d = new Date(today + 'T12:00:00Z'); d.setUTCDate(d.getUTCDate() + n); return d.toISOString().slice(0, 10); };
const due = day(21);
const docs: string[] = [];

try {
  // The person doing the work: a new employee with nothing on their plate.
  const u = (await admin.post('/users', { email: `wes.${s}@acme.test`, password: 'password123', first_name: 'Wes', last_name: `P${s}` })).data;
  const goods = (await admin.post('/crm/products', { name: `Bolt ${s}`, price: '2' })).data;
  const work = (await admin.post('/crm/products', { name: `Fitting ${s}`, price: '50', unit: 'h' })).data;
  await db.query('UPDATE crm_products SET stock = 10 WHERE id = $1', [goods.id]);
  await db.query('UPDATE crm_products SET is_service = TRUE WHERE id = $1', [work.id]);
  const deal = (await admin.post('/crm/deal/items', { title: `Promise ${s}`, assigned_to: u.id, close_date: due })).data;
  const rows = async (id: string, bolts: number, hours: number) => (await admin.put(`/crm/items/${id}/products`, { rows: [
    ...(bolts ? [{ product_id: goods.id, name: `Bolt ${s}`, price: '2', quantity: String(bolts), unit: 'pcs' }] : []),
    ...(hours ? [{ product_id: work.id, name: `Fitting ${s}`, price: '50', quantity: String(hours), unit: 'h' }] : []),
  ] })).status;
  ok('products on the deal', await rows(deal.id, 4, 10) === 200);
  const check = async (q = '') => (await admin.get(`/crm/items/${deal.id}/promise${q}`));

  let r = await check();
  const p = r.data.people?.[0];
  ok('the close date, the responsible person', r.status === 200 && r.data.date === due && p?.id === u.id, r.data);
  ok('working time is working days × the working day', p && p.working_days >= 13 && p.hours === p.working_days * 8 && p.free_hours === p.hours, p);
  ok('4 bolts of 10 in stock, 10 hours of work: can be done', r.data.verdict === 'ok' && r.data.goods[0].status === 'ok' && r.data.work.needed_hours === 10 && r.data.reasons.length === 0, r.data);

  // ---- stock ----
  const other = (await admin.post('/crm/deal/items', { title: `Other ${s}`, close_date: day(5) })).data;
  await rows(other.id, 8, 0);
  r = await check();
  ok('another deal due earlier needs 8 of them: short by 2', r.data.verdict === 'no' && Number(r.data.goods[0].committed) === 8 && /Short by 2/.test(r.data.goods[0].text) && r.data.earliest === null, r.data.goods);
  const wh = (await db.query('SELECT id FROM inv_warehouses ORDER BY created_at LIMIT 1')).rows[0]?.id ?? null;
  const receipt = async (date: string, qty: number) => {
    const d = (await db.query("INSERT INTO inv_documents (kind, number, status, to_warehouse, doc_date) VALUES ('receipt', $1, 'draft', $2, $3) RETURNING id", [`RC-${s}-${docs.length}`, wh, date])).rows[0].id;
    docs.push(d);
    await db.query('INSERT INTO inv_document_lines (document_id, product_id, quantity) VALUES ($1, $2, $3)', [d, goods.id, qty]);
  };
  await receipt(day(30), 5);
  r = await check();
  ok('a receipt expected after the date: not by then, but the earliest date is the receipt\'s', r.data.verdict === 'no' && r.data.earliest === day(30), [r.data.verdict, r.data.earliest, r.data.goods[0].text]);
  await receipt(day(10), 5);
  r = await check();
  ok('a receipt expected before the date: risky, with the receipt and its date named', r.data.verdict === 'risky' && new RegExp(`RC-${s}-1 arrives \\(${day(10)}\\)`).test(r.data.goods[0].text) && r.data.earliest === due, [r.data.verdict, r.data.earliest, r.data.goods[0].text]);
  await rows(other.id, 0, 0);

  // ---- work ----
  await rows(deal.id, 4, 1000);
  r = await check();
  const free = r.data.work.free_hours;
  ok('1000 hours of work: not by then, and the earliest date is later', r.data.verdict === 'no' && r.data.work.status === 'no' && r.data.earliest > due && /1000 hours of work/.test(r.data.reasons.join()), r.data);
  await rows(deal.id, 4, free - 5);
  r = await check();
  ok('nearly all the free time: risky', r.data.verdict === 'risky' && r.data.work.status === 'risky', r.data.work);
  await rows(deal.id, 4, 10);
  // A two-hour meeting on a working day and a 20-hour task due before the date.
  const monday = (() => { for (let i = 1; i < 8; i++) { const d = new Date(day(i) + 'T00:00:00Z'); if (d.getUTCDay() === 2) return day(i); } return day(1); })();
  await db.query("INSERT INTO cal_events (owner_id, created_by, title, starts_at, ends_at, timezone) VALUES ($1, $1, $2, ($3 || ' 10:00')::timestamp AT TIME ZONE $4, ($3 || ' 12:00')::timestamp AT TIME ZONE $4, $4)", [u.id, `Review ${s}`, monday, tz]);
  await db.query("INSERT INTO tasks (title, created_by, responsible_id, deadline, time_estimate_secs, status) VALUES ($1, $2, $2, $3, 72000, 'new')", [`Big job ${s}`, u.id, `${day(7)}T12:00:00Z`]);
  await db.query("INSERT INTO tasks (title, created_by, responsible_id, deadline, status) VALUES ($1, $2, $2, $3, 'new')", [`Vague job ${s}`, u.id, `${day(7)}T12:00:00Z`]);
  r = await check();
  const q = r.data.people[0];
  ok('meetings and tasks due come off the free time', q.meeting_hours === 2 && q.task_hours === 20 && q.tasks_without_estimate === 1 && q.free_hours === q.hours - 22, q);

  // ---- people ----
  await db.query("INSERT INTO hr_absences (user_id, kind, starts_on, ends_on, status) VALUES ($1, 'vacation', $2, $3, 'approved')", [u.id, day(20), day(22)]);
  r = await check();
  ok('the person is away on the day: risky, with the reason', r.data.verdict === 'risky' && r.data.people[0].away_on_the_day === true && /away on/.test(r.data.reasons.join()), r.data.reasons);
  ok('their time off comes off the working days', r.data.people[0].working_days < p.working_days, [r.data.people[0].working_days, p.working_days]);
  const me = (await admin.get('/auth/me')).data;
  r = await check(`?users=${u.id},${(me.user ?? me).id}`);
  ok('several people: their free time adds up', r.data.people.length === 2 && r.data.work.free_hours === r.data.people[0].free_hours + r.data.people[1].free_hours, r.data.people);
  r = await check(`?date=${day(60)}`);
  ok('another date', r.data.date === day(60) && r.data.verdict === 'ok', r.data);

  ok('a date that has passed is refused', (await check(`?date=${day(-1)}`)).status === 400);
  const contact = (await admin.post('/crm/contact/items', { first_name: 'Pat', last_name: `P${s}` })).data;
  ok('only deals', (await admin.get(`/crm/items/${contact.id}/promise?date=${due}`)).status === 400);
  ok('only for those signed in', (await new Client().get(`/crm/items/${other.id}/promise?date=${due}`)).status === 401);
} finally {
  for (const d of docs) await db.query('DELETE FROM inv_documents WHERE id = $1', [d]).catch(() => {});
  await db.query('DELETE FROM tasks WHERE title LIKE $1', [`% ${s}`]).catch(() => {});
  await db.query('DELETE FROM cal_events WHERE title = $1', [`Review ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title LIKE $1 OR last_name = $2", [`% ${s}`, `P${s}`]).catch(() => {});
  await db.query('DELETE FROM crm_products WHERE name LIKE $1', [`% ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
