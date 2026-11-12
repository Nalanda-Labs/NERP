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
const me0 = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user;
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const tz = (await db.query("SELECT value #>> '{}' AS v FROM settings WHERE key = 'default_timezone'")).rows[0]?.v ?? 'UTC';
const today = new Intl.DateTimeFormat('en-CA', { timeZone: tz }).format(new Date());
const day = (n: number) => { const d = new Date(today + 'T12:00:00Z'); d.setUTCDate(d.getUTCDate() + n); return d.toISOString().slice(0, 10); };
const due = day(21);
const docs: string[] = [];
let original: any[] | null = null, roleId = '';

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
  // A two-hour meeting on a working day, a meeting overlapping it by an hour, a weekly
  // one-hour meeting, one they were invited to and one they declined; a 20-hour task due
  // before the date.
  const tuesday = (() => { for (let i = 1; i < 8; i++) { const d = new Date(day(i) + 'T00:00:00Z'); if (d.getUTCDay() === 2) return day(i); } return day(1); })();
  const ev = async (title: string, d: string, from: string, to: string, owner: string, recurrence: unknown = null) =>
    (await db.query("INSERT INTO cal_events (owner_id, created_by, title, starts_at, ends_at, timezone, recurrence) VALUES ($1, $1, $2, ($3 || ' ' || $4)::timestamp AT TIME ZONE $6, ($3 || ' ' || $5)::timestamp AT TIME ZONE $6, $6, $7) RETURNING id",
      [owner, `${title} ${s}`, d, from, to, tz, recurrence])).rows[0].id;
  await ev('Review', tuesday, '10:00', '12:00', u.id);
  await ev('Overlap', tuesday, '11:00', '12:30', u.id);
  await ev('Weekly', tuesday, '15:00', '16:00', u.id, { freq: 'weekly', interval: 1, weekdays: [2], time: '15:00', timezone: tz, starts_on: tuesday });
  const invited = await ev('Invited', tuesday, '13:00', '14:00', me0.id);
  const declined = await ev('Declined', tuesday, '16:30', '17:30', me0.id);
  await db.query("INSERT INTO cal_attendees (event_id, user_id, status) VALUES ($1, $3, 'accepted'), ($2, $3, 'declined')", [invited, declined, u.id]);
  await db.query("INSERT INTO tasks (title, created_by, responsible_id, deadline, time_estimate_secs, status) VALUES ($1, $2, $2, $3, 72000, 'new')", [`Big job ${s}`, u.id, `${day(7)}T12:00:00Z`]);
  await db.query("INSERT INTO tasks (title, created_by, responsible_id, deadline, status) VALUES ($1, $2, $2, $3, 'new')", [`Vague job ${s}`, u.id, `${day(7)}T12:00:00Z`]);
  r = await check();
  const q = r.data.people[0];
  // Tuesdays from the first one up to the day before the due date.
  let weeks = 0;
  for (let d = tuesday; d < due; d = (() => { const x = new Date(d + 'T12:00:00Z'); x.setUTCDate(x.getUTCDate() + 7); return x.toISOString().slice(0, 10); })()) weeks++;
  ok('overlapping meetings count once, repeating ones every time, invitations unless declined', q.meeting_hours === 2.5 + weeks + 1, [q.meeting_hours, 2.5 + weeks + 1, weeks]);
  ok('meetings and tasks due come off the free time', q.task_hours === 20 && q.tasks_without_estimate === 1 && Math.abs(q.free_hours - (q.hours - q.meeting_hours - 20)) < 0.01, q);

  // A service measured in days is days of the company's working day.
  const workday = Number((await db.query('SELECT workday_hours FROM hr_settings LIMIT 1')).rows[0].workday_hours);
  await admin.put(`/crm/items/${deal.id}/products`, { rows: [{ product_id: work.id, name: `Fitting ${s}`, price: '400', quantity: '2', unit: 'days' }] });
  r = await check();
  ok('two days of work', r.data.work.needed_hours === 2 * workday, [r.data.work.needed_hours, workday]);
  await rows(deal.id, 4, 10);

  // ---- people ----
  await db.query("INSERT INTO hr_absences (user_id, kind, starts_on, ends_on, status) VALUES ($1, 'vacation', $2, $3, 'approved')", [u.id, day(20), day(22)]);
  r = await check();
  ok('the person is away on the day: risky, with the reason', r.data.verdict === 'risky' && r.data.people[0].away_on_the_day === true && /away on/.test(r.data.reasons.join()), r.data.reasons);
  ok('their time off comes off the working days', r.data.people[0].working_days < p.working_days, [r.data.people[0].working_days, p.working_days]);
  r = await check(`?users=${u.id},${me0.id}`);
  ok('several people: their free time adds up', r.data.people.length === 2 && r.data.work.free_hours === r.data.people[0].free_hours + r.data.people[1].free_hours, r.data.people);
  r = await check(`?date=${day(60)}`);
  ok('another date', r.data.date === day(60) && r.data.verdict === 'ok', r.data);

  ok('a date that has passed is refused', (await check(`?date=${day(-1)}`)).status === 400);
  const contact = (await admin.post('/crm/contact/items', { first_name: 'Pat', last_name: `P${s}` })).data;
  ok('only deals', (await admin.get(`/crm/items/${contact.id}/promise?date=${due}`)).status === 400);
  // With employees seeing only their own deals, Wes checks his deal but not someone else's.
  const empRole = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
  const perms = (await admin.get('/crm/permissions')).data;
  original = Object.entries(perms.matrix[empRole.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
  roleId = empRole.id;
  await admin.put(`/crm/permissions/${roleId}`, { entries: original.map((e: any) => (e.entity_type === 2 && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
  const wes = new Client();
  await wes.post('/auth/login', { email: `wes.${s}@acme.test`, password: 'password123' });
  ok('their own deal: yes', (await wes.get(`/crm/items/${deal.id}/promise?date=${due}`)).status === 200);
  ok('someone else\'s: no', (await wes.get(`/crm/items/${other.id}/promise?date=${due}`)).status === 404);
  ok('only for those signed in', (await new Client().get(`/crm/items/${other.id}/promise?date=${due}`)).status === 401);
} finally {
  if (original) await admin.put(`/crm/permissions/${roleId}`, { entries: original }).catch(() => {});
  for (const d of docs) await db.query('DELETE FROM inv_documents WHERE id = $1', [d]).catch(() => {});
  await db.query('DELETE FROM tasks WHERE title LIKE $1', [`% ${s}`]).catch(() => {});
  await db.query('DELETE FROM cal_events WHERE title LIKE $1', [`% ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title LIKE $1 OR last_name = $2", [`% ${s}`, `P${s}`]).catch(() => {});
  await db.query('DELETE FROM crm_products WHERE name LIKE $1', [`% ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
