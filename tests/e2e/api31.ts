// HR: the work day (start, pause, resume, end, start again), timesheets and who may see them
// (heads of departments, HR), corrections with reasons, team figures; time off (working
// days without weekends and holidays, overlaps, balances and allowances, approvers, deciding,
// cancelling, the chart hiding notes), settings.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Hr' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const hed = await user('Hed');
const bob = await user('Bob');
const before = (await admin.get('/hr/settings')).data.settings;
const notes = async (c: Client, text: string) => (await c.get('/notifications')).data.items?.filter((n: any) => n.title.includes(text)) ?? [];
const ymd = (d: Date) => d.toISOString().slice(0, 10);
// Next year's March: the first Monday, and that week.
const year = new Date().getUTCFullYear() + 1;
const mon = (() => { const d = new Date(Date.UTC(year, 2, 1)); while (d.getUTCDay() !== 1) d.setUTCDate(d.getUTCDate() + 1); return d; })();
const day = (n: number) => ymd(new Date(mon.getTime() + n * 86400000));
const thisMonth = new Date().toISOString().slice(0, 7);

try {
  const ops = (await admin.post('/departments', { name: `Ops ${stamp}`, head_user_id: hed.id })).data;
  await admin.post(`/departments/${ops.id}/members`, { user_id: ann.id });
  await admin.post(`/departments/${ops.id}/members`, { user_id: hed.id });

  // ---- the work day ----
  ok('nothing yet', (await ann.get('/hr/workday')).data.state === 'none');
  ok('can\'t pause before starting', (await ann.post('/hr/workday/pause', {})).status === 400);
  ok('started', (await ann.post('/hr/workday/start', {})).data.state === 'working');
  ok('only once', (await ann.post('/hr/workday/start', {})).status === 400);
  ok('a break', (await ann.post('/hr/workday/pause', {})).data.state === 'paused');
  ok('back', (await ann.post('/hr/workday/resume', {})).data.state === 'working');
  ok('ended', (await ann.post('/hr/workday/end', {})).data.state === 'ended');
  let r = await ann.post('/hr/workday/start', {});
  ok('started again: the gap is a break', r.data.state === 'working' && r.data.breaks === 2, r.data);
  await ann.post('/hr/workday/end', {});
  r = await ann.get(`/hr/timesheet?month=${thisMonth}`);
  ok('my timesheet', r.data.days.length === 1 && r.data.can_edit === false && r.data.expected_minutes >= 0, r.data);
  const today = r.data.days[0];

  // ---- who sees timesheets ----
  ok('not someone else\'s', (await ann.get(`/hr/timesheet?month=${thisMonth}&user=${bob.id}`)).status === 403);
  ok('the head sees their people', (await hed.get(`/hr/timesheet?month=${thisMonth}&user=${ann.id}`)).data.can_edit === true);
  ok('but not others', (await hed.get(`/hr/timesheet?month=${thisMonth}&user=${bob.id}`)).status === 403);
  ok('HR sees everyone', (await admin.get(`/hr/timesheet?month=${thisMonth}&user=${bob.id}`)).status === 200);
  r = await hed.get(`/hr/team?month=${thisMonth}`);
  ok('the head\'s team', r.data.items.length === 1 && r.data.items[0].user_id === ann.id && r.data.items[0].days === 1, r.data);

  // ---- corrections ----
  const s = new Date(Date.now() - 8 * 3600000), e = new Date(Date.now() - 3600000);
  ok('a reason is needed', (await hed.patch(`/hr/workdays/${today.id}`, { started_at: s.toISOString(), ended_at: e.toISOString(), reason: '' })).status === 400);
  ok('ends after it starts', (await hed.patch(`/hr/workdays/${today.id}`, { started_at: e.toISOString(), ended_at: s.toISOString(), reason: 'x' })).status === 400);
  ok('people don\'t correct themselves', (await ann.patch(`/hr/workdays/${today.id}`, { started_at: s.toISOString(), ended_at: e.toISOString(), reason: 'x' })).status === 403);
  r = await hed.patch(`/hr/workdays/${today.id}`, { started_at: s.toISOString(), ended_at: e.toISOString(), reason: 'Forgot to start' });
  const fixed = r.data.days[0];
  ok('corrected', fixed.worked_minutes === 420 && fixed.edit_reason === 'Forgot to start' && fixed.edited_by_name === 'Hed Hr', fixed);
  const prev = new Date(); prev.setUTCDate(1); prev.setUTCMonth(prev.getUTCMonth() - 1);
  const pd = ymd(prev);
  r = await hed.post('/hr/workdays', { user_id: ann.id, day: pd, started_at: `${pd}T08:00:00Z`, ended_at: `${pd}T16:30:00Z`, reason: 'Was on site' });
  ok('a missing day added', r.data.days.some((d: any) => d.day === pd && d.worked_minutes === 510), r.data);
  ok('not twice', (await hed.post('/hr/workdays', { user_id: ann.id, day: pd, started_at: `${pd}T08:00:00Z`, ended_at: `${pd}T09:00:00Z`, reason: 'x' })).status === 400);

  // ---- settings ----
  ok('only HR changes settings', (await ann.patch('/hr/settings', { annual_vacation_days: 99 })).status === 403);
  await admin.patch('/hr/settings', { annual_vacation_days: 10, work_weekdays: [1, 2, 3, 4, 5] });
  await admin.post('/hr/holidays', { day: day(2), name: `Founders day ${stamp}` });

  // ---- time off ----
  ok('ends before it starts', (await ann.post('/hr/absences', { kind: 'vacation', starts_on: day(4), ends_on: day(0) })).status === 400);
  ok('weekend only', (await ann.post('/hr/absences', { kind: 'vacation', starts_on: day(5), ends_on: day(6) })).status === 400);
  r = await ann.post('/hr/absences', { kind: 'vacation', starts_on: day(0), ends_on: day(6), note: `Family trip ${stamp}` });
  const vac = r.data;
  ok('requested: four working days (weekend and holiday don\'t count)', r.status === 200 && vac.days === 4 && vac.status === 'pending', r.data);
  ok('no overlaps', (await ann.post('/hr/absences', { kind: 'personal', starts_on: day(3), ends_on: day(3) })).status === 400);
  r = await ann.post('/hr/absences', { kind: 'vacation', starts_on: day(7), ends_on: day(15) });
  ok('not more than is left', r.status === 400 && /you have 6 left/.test(JSON.stringify(r.data)), r.data);
  ok('the head is asked', !!(await waitFor(async () => (await notes(hed, 'Ann Hr asks for vacation'))[0], 5000)));
  ok('others cannot decide', (await bob.post(`/hr/absences/${vac.id}/decide`, { approve: true })).status === 403 && (await ann.post(`/hr/absences/${vac.id}/decide`, { approve: true })).status === 403);
  r = await hed.get('/hr/absences/pending');
  ok('waiting for the head, with the balance', r.data.items.length === 1 && r.data.items[0].days === 4 && Number(r.data.items[0].balance.left) === 10 && r.data.items[0].balance.requested === 4, r.data);
  await hed.post(`/hr/absences/${vac.id}/decide`, { approve: true, note: 'Enjoy' });
  ok('approved; Ann is told', !!(await waitFor(async () => (await notes(ann, 'Hed Hr approved your vacation'))[0], 5000)));
  ok('decided once', (await hed.post(`/hr/absences/${vac.id}/decide`, { approve: false })).status === 400);
  r = await ann.get(`/hr/balance?year=${year}`);
  ok('balance', Number(r.data.allowance) === 10 && r.data.used === 4 && Number(r.data.left) === 6, r.data);

  // The chart: everyone sees who is away; notes only for the person and their managers.
  const chartOf = async (c: Client) => (await c.get(`/hr/absences?from=${day(0)}&to=${day(6)}&department=${ops.id}`)).data;
  const seen = (await chartOf(bob)).items.find((a: any) => a.id === vac.id);
  ok('the chart shows it, without the note for others', seen && seen.kind === 'vacation' && seen.note === null, seen);
  ok('the head sees the note', (await chartOf(hed)).items.find((a: any) => a.id === vac.id).note === `Family trip ${stamp}`);
  ok('holidays on the chart', (await chartOf(bob)).holidays.includes(day(2)));

  // Nobody above: HR decides.
  const sick = (await bob.post('/hr/absences', { kind: 'sick', starts_on: day(9), ends_on: day(9) })).data;
  ok('without a head, HR is asked', (await admin.get('/hr/absences/pending')).data.items.some((a: any) => a.id === sick.id) && !(await hed.get('/hr/absences/pending')).data.items.some((a: any) => a.id === sick.id));
  await admin.post(`/hr/absences/${sick.id}/decide`, { approve: false, note: 'Use a personal day' });
  ok('rejected with a reason', (await bob.get('/hr/absences/mine')).data.items[0].decision_note === 'Use a personal day');

  // Cancelling approved future time off tells the approver.
  r = await ann.post(`/hr/absences/${vac.id}/cancel`, {});
  const after = (await ann.get(`/hr/balance?year=${year}`)).data;
  ok('cancelled: the days are back', r.data.items.find((a: any) => a.id === vac.id).status === 'cancelled' && after.used === 0 && Number(after.left) === 10, after);
  ok('the head is told', !!(await waitFor(async () => (await notes(hed, 'Ann Hr cancelled approved time off'))[0], 5000)));

  // ---- allowances ----
  r = await admin.put(`/hr/allowances/${ann.id}/${year}`, { days: 15 });
  ok('a personal allowance', Number(r.data.allowance) === 15 && r.data.custom);
  r = await admin.put(`/hr/allowances/${ann.id}/${year}`, { days: null });
  ok('back to the default', Number(r.data.allowance) === 10 && !r.data.custom);
  ok('managers see balances of their people only', (await hed.get(`/hr/balance?user=${ann.id}&year=${year}`)).status === 200 && (await hed.get(`/hr/balance?user=${bob.id}&year=${year}`)).status === 403);
} finally {
  await admin.patch('/hr/settings', { annual_vacation_days: Number(before.annual_vacation_days), work_weekdays: before.work_weekdays, workday_hours: Number(before.workday_hours) });
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
