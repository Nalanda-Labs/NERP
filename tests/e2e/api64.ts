// HR depth: deputy heads (manage and approve with the head); work reports (rules for the
// company and per person, the prepared draft with the period's figures, writing, sending to
// the readers, marks, who may see what, reminders); My day (work day, tasks, events, one's
// own plan); shifts and the rota (who may plan whom, ranges of weekdays, late starts).
import pg from 'pg';
import { Client, sleep } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Hr${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [hed, dep, ann, bob] = await Promise.all(['Hed', 'Dep', 'Ann', 'Bob'].map(user));
const today = new Date().toISOString().slice(0, 10);
const notes = async (c: Client) => JSON.stringify((await c.get('/notifications')).data);

try {
  // ---- deputies ----
  const team = (await admin.post('/departments', { name: `Team ${stamp}`, head_user_id: hed.id })).data;
  for (const u of [dep, ann, bob]) await admin.post(`/departments/${team.id}/members`, { user_id: u.id });
  let r = await admin.patch(`/departments/${team.id}`, { deputy_user_id: dep.id });
  ok('a department gets a deputy head', r.data.deputy_user_id === dep.id && r.data.deputy_name === `Dep Hr${stamp}`, r.data);
  r = await ann.post('/hr/absences', { kind: 'vacation', starts_on: '2027-03-01', ends_on: '2027-03-02' });
  ok('the deputy is asked to approve time off too', (await notes(dep)).includes(`Ann Hr${stamp}`) && (await notes(hed)).includes(`Ann Hr${stamp}`), r.data);
  ok('…and may decide it', (await dep.post(`/hr/absences/${r.data.id}/decide`, { approve: true })).status === 200);

  // ---- work report rules ----
  ok('only HR sets the company rule', (await ann.put('/hr/report-rules/company', { period: 'daily' })).status === 403);
  r = await admin.put('/hr/report-rules/company', { period: 'weekly' });
  ok('the company reports weekly', r.data.company === 'weekly');
  ok('a manager sets a person\'s own rule', (await hed.put('/hr/report-rules', { user_id: bob.id, period: 'daily' })).status === 200);
  ok('but not their own', (await hed.put('/hr/report-rules', { user_id: hed.id, period: 'none' })).status === 403);
  ok('nor outsiders\'', (await ann.put('/hr/report-rules', { user_id: bob.id, period: 'none' })).status === 403);

  // ---- a report ----
  const task = (await ann.post('/tasks', { title: `Ship it ${stamp}`, responsible_id: ann.id })).data;
  await ann.post(`/tasks/${task.id}/status`, { status: 'done' });
  await ann.post('/hr/workday/start', {});
  r = await ann.get('/hr/reports/current');
  const rep = r.data.report;
  ok('the current report is a weekly draft for this week', r.data.period === 'weekly' && rep.status === 'draft' && rep.period_end > rep.period_start, r.data);
  ok('read by the head and the deputy', r.data.readers.includes(`Hed Hr${stamp}`) && r.data.readers.includes(`Dep Hr${stamp}`), r.data.readers);
  ok('with the week\'s figures: tasks done', rep.figures.tasks_done.some((t: any) => t.title === `Ship it ${stamp}`), rep.figures);
  ok('nothing written: can\'t be sent', (await ann.post(`/hr/reports/${rep.id}/send`, {})).status === 400);
  ok('only its author writes it', (await bob.put(`/hr/reports/${rep.id}`, { body: 'x' })).status === 404);
  await ann.put(`/hr/reports/${rep.id}`, { body: 'Shipped the release.', plans: 'Next: the docs.' });
  r = await ann.post(`/hr/reports/${rep.id}/send`, {});
  ok('sent', r.data.status === 'sent' && r.data.reviewer_ids.includes(hed.id) && r.data.reviewer_ids.includes(dep.id), r.data);
  ok('the readers are told', (await notes(hed)).includes(`Ann Hr${stamp} sent a work report`));
  ok('a sent report can\'t change', (await ann.put(`/hr/reports/${rep.id}`, { body: 'y' })).status === 400);
  ok('it is in the readers\' list', (await dep.get('/hr/reports?scope=review')).data.items.some((x: any) => x.id === rep.id));
  ok('colleagues can\'t read it', (await bob.get(`/hr/reports/${rep.id}`)).status === 404);
  ok('nor mark it', (await bob.post(`/hr/reports/${rep.id}/review`, { mark: 'good' })).status === 403);
  ok('its author can\'t mark it', (await ann.post(`/hr/reports/${rep.id}/review`, { mark: 'good' })).status === 403);
  r = await hed.post(`/hr/reports/${rep.id}/review`, { mark: 'good', comment: 'Well done' });
  ok('the head marks it', r.data.status === 'reviewed' && r.data.mark === 'good' && r.data.reviewed_by_name === `Hed Hr${stamp}`, r.data);
  ok('the author is told', (await notes(ann)).includes('liked your report'));
  ok('a manager sees a person\'s reports', (await hed.get(`/hr/reports?scope=person&user_id=${ann.id}`)).data.items.length === 1);
  ok('Bob reports daily', (await bob.get('/hr/reports/current')).data.period === 'daily');
  ok('a person with "none" has no report', (await admin.put('/hr/report-rules', { user_id: bob.id, period: 'none' })).status === 200 && (await bob.get('/hr/reports/current')).data.period === 'none');

  // ---- reminders ----
  await admin.put('/hr/report-rules', { user_id: bob.id, period: 'daily' });
  const yday = new Date(Date.now() - 86400e3).toISOString().slice(0, 10);
  await db.query(`UPDATE users SET created_at = now() - interval '10 days' WHERE id = $1`, [bob.id]);
  await db.query(`INSERT INTO hr_work_days (user_id, day, started_at, ended_at) VALUES ($1, $2::date, $2::date + time '09:00', $2::date + time '17:00')`, [bob.id, yday]);
  let reminded = false;
  // Run the hourly job now.
  await db.query(`INSERT INTO jobs (kind, payload) VALUES ('hr.reports.remind', '{}')`);
  for (let i = 0; i < 30 && !reminded; i++) {
    await sleep(1000);
    reminded = (await notes(bob)).includes('is due');
  }
  ok('someone who worked yesterday without a report is reminded', reminded);

  // ---- My day ----
  await ann.post('/tasks', { title: `Due today ${stamp}`, responsible_id: ann.id, deadline: new Date(Date.now() + 3600e3).toISOString() });
  await ann.post('/calendar/events', { title: `Standup ${stamp}`, start: new Date(Date.now() + 1800e3).toISOString(), end: new Date(Date.now() + 2700e3).toISOString(), timezone: 'UTC' });
  r = await ann.get('/hr/my-day');
  ok('My day: the work day, tasks due, today\'s events', r.data.workday.state === 'working' && r.data.tasks.some((t: any) => t.title === `Due today ${stamp}`) && r.data.events.some((e: any) => e.title === `Standup ${stamp}`), r.data);
  r = await ann.post('/hr/my-day/plan', { title: 'Call the bank' });
  const tdue = (await ann.get('/hr/my-day')).data.tasks.find((t: any) => t.title === `Due today ${stamp}`);
  r = await ann.post('/hr/my-day/plan', { task_id: tdue.id });
  ok('a plan of free items and tasks', r.data.plan.length === 2 && r.data.plan.some((p: any) => p.task_number === tdue.number), r.data.plan);
  ok('a task once only', (await ann.post('/hr/my-day/plan', { task_id: tdue.id })).data.plan.length === 2);
  ok('only one\'s own tasks or ones one can see', (await bob.post('/hr/my-day/plan', { task_id: tdue.id })).status === 404);
  const item = r.data.plan.find((p: any) => p.title === 'Call the bank');
  r = await ann.patch(`/hr/my-day/plan/${item.id}`, { done: true });
  ok('ticked off', r.data.plan.find((p: any) => p.id === item.id).done === true);
  await ann.post(`/tasks/${tdue.id}/status`, { status: 'done' });
  ok('a finished task counts as done in the plan', (await ann.get('/hr/my-day')).data.plan.find((p: any) => p.task_id === tdue.id).done === true);
  ok('nobody else\'s plan changes', (await bob.patch(`/hr/my-day/plan/${item.id}`, { done: false })).status === 404);
  r = await ann.get('/hr/reports/current?period=daily');
  ok('the day\'s report can be written from My day', r.data.period === 'daily' && r.data.report.period_start === today, r.data);

  // ---- shifts ----
  ok('only HR sets up shifts', (await hed.post('/hr/shifts', { name: 'x' })).status === 403);
  ok('a shift is checked', (await admin.post('/hr/shifts', { name: `Bad ${stamp}`, start_min: 600, end_min: 500 })).status === 400);
  r = await admin.post('/hr/shifts', { name: `Early ${stamp}`, start_min: 360, end_min: 840, break_minutes: 30 });
  const early = r.data.items.find((s: any) => s.name === `Early ${stamp}`);
  const night = (await admin.post('/hr/shifts', { name: `Night ${stamp}`, start_min: 1320, end_min: 1800 })).data.items.find((s: any) => s.name === `Night ${stamp}`);
  ok('shifts, including one past midnight', !!early && night?.end_min === 1800);
  const mon = (() => { const d = new Date(); d.setUTCDate(d.getUTCDate() + ((8 - d.getUTCDay()) % 7 || 7)); return d.toISOString().slice(0, 10); })();
  const sun = new Date(Date.parse(mon) + 6 * 86400e3).toISOString().slice(0, 10);
  ok('people can\'t plan their colleagues', (await ann.put('/hr/rota', { entries: [{ user_id: bob.id, day: mon, shift_id: early.id }] })).status === 403);
  r = await dep.put('/hr/rota', { entries: [{ user_id: bob.id, from: mon, to: sun, weekdays: [1, 2, 3, 4, 5], shift_id: early.id }, { user_id: ann.id, day: mon, shift_id: night.id }] });
  ok('the deputy plans the week: weekdays only', r.status === 200, r.data);
  r = await hed.get(`/hr/rota?from=${mon}&to=${sun}&department_id=${team.id}`);
  const bobCells = r.data.cells.filter((c: any) => c.user_id === bob.id);
  ok('the rota shows them', bobCells.length === 5 && r.data.cells.some((c: any) => c.user_id === ann.id && c.shift_id === night.id), r.data.cells);
  ok('Bob is told', (await notes(bob)).includes('changed your shifts'));
  ok('people see only their own rota', (await bob.get(`/hr/rota?from=${mon}&to=${sun}`)).data.people.length === 1);
  await hed.put('/hr/rota', { entries: [{ user_id: bob.id, day: sun, shift_id: early.id }] });
  await hed.put('/hr/rota', { entries: [{ user_id: bob.id, day: sun, shift_id: null }] });
  ok('taken off a day', (await hed.get(`/hr/rota?from=${sun}&to=${sun}&department_id=${team.id}`)).data.cells.filter((c: any) => c.user_id === bob.id).length === 0);
  // A late start: Bob starts 40 minutes after his shift (in UTC, his zone).
  await db.query(`UPDATE users SET timezone = 'UTC' WHERE id = $1`, [bob.id]);
  await db.query(`INSERT INTO hr_work_days (user_id, day, started_at, ended_at) VALUES ($1, $2::date, $2::date + time '06:40', $2::date + time '14:00')`, [bob.id, mon]);
  r = await hed.get(`/hr/rota?from=${mon}&to=${mon}&department_id=${team.id}`);
  const cell = r.data.cells.find((c: any) => c.user_id === bob.id);
  ok('a late start is marked', cell.late === true && cell.late_minutes === 40, cell);
  await admin.put('/hr/report-rules/company', { period: 'none' });
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
