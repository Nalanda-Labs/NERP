// Check-in: working (where, a message to a chat, where they are) or not working (why); once a
// day; cancelling with a reason; undo; the reminder (who gets it, muting) and what is
// remembered; starting the work day; department views by day and month and who may see them;
// company settings.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Checkin' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const hed = await user('Hed');
const out = await user('Out');
const before = (await admin.get('/checkin/settings')).data;
const month = new Date().toISOString().slice(0, 7);

try {
  const team = (await admin.post('/departments', { name: `Team ${stamp}`, head_user_id: hed.id })).data;
  await admin.post(`/departments/${team.id}/members`, { user_id: ann.id });
  await admin.post(`/departments/${team.id}/members`, { user_id: bob.id });
  await admin.patch('/checkin/settings', { enabled: true, geo_enabled: true });
  const chat = (await ann.post('/im/chats', { title: `Daily ${stamp}`, members: [bob.id] })).data.id;
  const outsiders = (await out.post('/im/chats', { title: `Elsewhere ${stamp}`, members: [hed.id] })).data.id;

  // ---- before checking in ----
  let r = await ann.get('/checkin/today');
  ok('nothing yet; no reminder for someone who never checked in', r.data.checkin === null && r.data.remind === false && r.data.enabled === true, r.data);
  const workday = r.data.working_day;

  // ---- checking in as working ----
  ok('custom needs words', (await ann.post('/checkin', { status: 'working', location: 'custom' })).status === 400);
  ok('unknown location refused', (await ann.post('/checkin', { status: 'working', location: 'moon' })).status === 400);
  ok('half a position refused', (await ann.post('/checkin', { status: 'working', location: 'office', latitude: 51.5 })).status === 400);
  ok('not to a chat you are not in', (await ann.post('/checkin', { status: 'working', location: 'office', send_message: true, chat_id: outsiders })).status === 404);
  ok('nothing was saved by those', (await ann.get('/checkin/today')).data.checkin === null);
  r = await ann.post('/checkin', {
    status: 'working', location: 'office', message: 'In from 9, standup at 10', send_message: true, chat_id: chat,
    latitude: 51.50135, longitude: -0.14189, address: 'Buckingham Gate, London', start_workday: true,
  });
  const mine = r.data.checkin;
  ok('checked in, working in the office', r.status === 200 && mine.status === 'working' && mine.location === 'office' && mine.chat_title === `Daily ${stamp}`, r.data);
  ok('with where I am', mine.latitude === 51.50135 && mine.address === 'Buckingham Gate, London');
  ok('my work day started', r.data.workday_started === true && (await ann.get('/hr/workday')).data.state === 'working');
  const msgs = (await bob.get(`/im/chats/${chat}/messages`)).data.items;
  const posted = msgs.find((m: any) => m.body.startsWith('✅ Working in office today'));
  ok('posted to the chat with the message and a map link', posted && posted.body.includes('In from 9, standup at 10') && posted.body.includes('📍 Buckingham Gate, London') && posted.body.includes('openstreetmap.org/?mlat=51.501350'), msgs.map((m: any) => m.body));
  ok('only once a day', (await ann.post('/checkin', { status: 'working', location: 'home' })).status === 409);
  r = await ann.get('/checkin/today');
  ok('remembered for next time', r.data.prefs.location === 'office' && r.data.prefs.chat_id === chat && r.data.prefs.message === 'In from 9, standup at 10' && r.data.prefs.send_geo === true && r.data.prefs.start_workday === true, r.data.prefs);
  ok('no reminder once checked in', r.data.remind === false);

  // ---- the chat's members now get the reminder ----
  r = await bob.get('/checkin/today');
  ok('Bob, in the chat, is reminded on a working day', r.data.remind === workday, r.data);
  ok('Hed, not in it, is not', (await hed.get('/checkin/today')).data.remind === false);
  r = await bob.patch('/checkin/prefs', { mute: 'day' });
  ok('muted for today', r.data.remind === false && r.data.muted === true && new Date(r.data.prefs.muted_until) > new Date(), r.data);
  r = await bob.patch('/checkin/prefs', { mute: 'off' });
  ok('unmuted', r.data.muted === false && r.data.remind === workday);
  r = await bob.patch('/checkin/prefs', { mute: 'forever' });
  ok('muted for good', r.data.prefs.muted_forever === true && r.data.remind === false);
  ok('a bad mute refused', (await bob.patch('/checkin/prefs', { mute: 'year' })).status === 400);
  await bob.patch('/checkin/prefs', { mute: 'off' });

  // ---- not working ----
  ok('a reason is needed', (await bob.post('/checkin', { status: 'not_working' })).status === 400);
  ok('custom reason needs words', (await bob.post('/checkin', { status: 'not_working', reason: 'custom' })).status === 400);
  r = await bob.post('/checkin', { status: 'not_working', reason: 'sick_leave', send_message: true, chat_id: chat });
  ok('Bob is not working: sick leave', r.data.checkin.status === 'not_working' && r.data.checkin.reason_text === 'Sick leave' && r.data.checkin.location === null, r.data);
  ok('said in the chat', (await ann.get(`/im/chats/${chat}/messages`)).data.items.some((m: any) => m.body === '🚫 Not working today: Sick leave'));
  ok('not working can\'t be cancelled', (await bob.post(`/checkin/${r.data.checkin.id}/cancel`, { reason: 'illness' })).status === 400);
  ok('nor someone else\'s', (await bob.post(`/checkin/${mine.id}/cancel`, { reason: 'illness' })).status === 404);

  // ---- cancelling a working day ----
  r = await ann.post(`/checkin/${mine.id}/cancel`, { reason: 'custom', reason_text: 'Water leak at home' });
  ok('Ann isn\'t working after all', r.data.checkin.status === 'cancelled' && r.data.checkin.reason_text === 'Water leak at home' && r.data.checkin.cancelled_at, r.data);
  ok('the chat is told', (await bob.get(`/im/chats/${chat}/messages`)).data.items.some((m: any) => m.body === '🚫 Not working today after all: Water leak at home'));

  // ---- the department ----
  r = await ann.get('/checkin/departments');
  ok('members see their department', r.data.items.some((d: any) => d.id === team.id));
  ok('outsiders don\'t', !(await out.get('/checkin/departments')).data.items.some((d: any) => d.id === team.id));
  ok('nor its day', (await out.get(`/checkin/departments/${team.id}/day`)).status === 404);
  r = await hed.get(`/checkin/departments/${team.id}/day`);
  const row = (id: string) => r.data.people.find((p: any) => p.user_id === id);
  ok('the head sees the day: everyone, with their check-in', r.data.people.length === 3 && row(ann.id).checkin.status === 'cancelled' && row(bob.id).checkin.reason === 'sick_leave' && row(hed.id).checkin === null, r.data);
  ok('totals', r.data.totals.people === 3 && r.data.totals.not_working === 2 && r.data.totals.no_checkin === 1, r.data.totals);
  ok('the head sees where Ann was', row(ann.id).checkin.address === 'Buckingham Gate, London');
  r = await bob.get(`/checkin/departments/${team.id}/day`);
  ok('a colleague sees the check-in but not the exact place', row(ann.id).checkin.location === 'office' && row(ann.id).checkin.address === null && row(ann.id).checkin.latitude === null, row(ann.id));
  r = await hed.get(`/checkin/departments/${team.id}/month?month=${month}`);
  ok('the month: each person\'s days', r.data.people.find((p: any) => p.user_id === bob.id).not_working === 1 && r.data.working_days >= 0 && r.data.people.find((p: any) => p.user_id === ann.id).days.length === 1, r.data);
  ok('a bad month refused', (await hed.get(`/checkin/departments/${team.id}/month?month=May`)).status === 400);
  await ann.patch('/checkin/prefs', { department_id: team.id });
  ok('the department looked at is remembered', (await ann.get('/checkin/today')).data.prefs.department_id === team.id);

  // ---- history ----
  r = await ann.get(`/checkin/history?month=${month}`);
  ok('my month', r.data.items.length === 1 && r.data.working_days === 0, r.data);
  ok('the head sees Ann\'s', (await hed.get(`/checkin/history?month=${month}&user=${ann.id}`)).data.items.length === 1);
  ok('a colleague doesn\'t', (await bob.get(`/checkin/history?month=${month}&user=${ann.id}`)).status === 403);

  // ---- undo ----
  ok('someone else can\'t undo it', (await bob.del(`/checkin/${mine.id}`)).status === 404);
  r = await ann.del(`/checkin/${mine.id}`);
  ok('undone: can check in again', r.data.checkin === null && (await ann.post('/checkin', { status: 'working', location: 'home' })).data.checkin.location === 'home');
  const bobs = (await bob.get('/checkin/today')).data.checkin;
  ok('HR can clear anyone\'s', (await admin.del(`/checkin/${bobs.id}`)).status === 200 && (await bob.get('/checkin/today')).data.checkin === null);

  // ---- settings ----
  ok('only HR changes settings', (await ann.patch('/checkin/settings', { enabled: false })).status === 403);
  await admin.patch('/checkin/settings', { geo_enabled: false });
  ok('no positions when sharing where you are is off', (await bob.post('/checkin', { status: 'working', location: 'office', latitude: 1, longitude: 1 })).status === 400);
  await admin.patch('/checkin/settings', { enabled: false });
  r = await bob.get('/checkin/today');
  ok('turned off: no reminder, no checking in', r.data.enabled === false && r.data.remind === false && (await bob.post('/checkin', { status: 'working', location: 'office' })).status === 400, r.data);
} finally {
  await admin.patch('/checkin/settings', { enabled: before.enabled, geo_enabled: before.geo_enabled });
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
