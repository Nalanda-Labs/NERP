// Calendar: events (validation, guests, answers, who may change them, private events seen
// as busy), repeats (occurrences, removing one, local time), all-day events, moving an event,
// task deadlines and CRM meetings in the view, the subscription feed, reminders, deleting.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Cal' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const eve = await user('Eve');
const iso = (d: Date) => d.toISOString();
const day = 86400000;
// Next Monday 10:00 UTC, a week ahead at least.
const mon = (() => { const d = new Date(); d.setUTCHours(10, 0, 0, 0); d.setUTCDate(d.getUTCDate() + ((8 - d.getUTCDay()) % 7 || 7) + 7); return d; })();
const range = (c: Client, from: Date, to: Date, extra = '') => c.get(`/calendar?from=${encodeURIComponent(iso(from))}&to=${encodeURIComponent(iso(to))}${extra}`);
const notes = async (c: Client, text: string) => (await c.get('/notifications')).data.items?.filter((n: any) => n.title.includes(text)) ?? [];

try {
  // ---- creating ----
  const ev = (b: any) => ann.post('/calendar/events', { timezone: 'UTC', ...b });
  ok('title needed', (await ev({ title: ' ', start: iso(mon), end: iso(mon) })).status === 400);
  ok('ends before it starts', (await ev({ title: `X ${stamp}`, start: iso(mon), end: iso(new Date(mon.getTime() - 1000)) })).status === 400);
  ok('time zone checked', (await ev({ title: `X ${stamp}`, start: iso(mon), end: iso(mon), timezone: 'Mars/Base' })).status === 400);
  ok('guests are active employees', (await ev({ title: `X ${stamp}`, start: iso(mon), end: iso(mon), attendees: ['00000000-0000-0000-0000-000000000123'] })).status === 400);
  const deal = (await admin.post('/crm/deal/items', { title: `Big deal ${stamp}` })).data;
  let r = await ev({ title: `Kickoff ${stamp}`, start: iso(mon), end: iso(new Date(mon.getTime() + 3600000)), location: 'Room 1', attendees: [bob.id], item_id: deal.id, remind_minutes: 15 });
  const kick = r.data;
  ok('event made', r.status === 200 && kick.can_edit && kick.people.length === 2 && kick.people.find((p: any) => p.id === bob.id).status === 'pending' && kick.item?.id === deal.id, r.data);
  ok('guest invited', (await waitFor(async () => (await notes(bob, `Kickoff ${stamp}`))[0], 5000))?.title.startsWith('Ann invited you to'));
  ok('the deal says so', (await admin.get(`/crm/items/${deal.id}/timeline`)).data.items.some((e: any) => e.kind === 'calendar' && e.data.title === `Kickoff ${stamp}`));

  // ---- seeing it ----
  const week = [mon, new Date(mon.getTime() + 7 * day)] as const;
  r = await range(bob, ...week);
  ok('on the guest\'s calendar, with their answer pending', r.data.items.some((e: any) => e.id === kick.id && e.my_status === 'pending'), r.data);
  ok('only the owner changes it', (await bob.patch(`/calendar/events/${kick.id}`, { title: 'Mine now' })).status === 403);
  ok('others answer only their own invitations', (await eve.post(`/calendar/events/${kick.id}/respond`, { status: 'accepted' })).status === 404);
  r = await bob.post(`/calendar/events/${kick.id}/respond`, { status: 'accepted' });
  ok('accepted', r.data.people.find((p: any) => p.id === bob.id).status === 'accepted');
  ok('the owner is told', !!(await waitFor(async () => (await notes(ann, `accepted “Kickoff ${stamp}”`))[0], 5000)));
  const priv = (await ev({ title: `Doctor ${stamp}`, start: iso(new Date(mon.getTime() + 2 * 3600000)), end: iso(new Date(mon.getTime() + 3 * 3600000)), is_private: true })).data;
  r = await range(eve, ...week, `&users=${ann.id}`);
  const seen = r.data.items.filter((e: any) => e.user_id === ann.id);
  ok('a colleague\'s calendar: details, and private events as busy', seen.some((e: any) => e.title === `Kickoff ${stamp}`) && seen.some((e: any) => e.id === priv.id && e.busy && e.title === 'Busy' && !e.location), seen);
  ok('a private event cannot be opened by others', (await eve.get(`/calendar/events/${priv.id}`)).status === 404);

  // ---- repeats ----
  const until = new Date(mon.getTime() + 20 * day).toISOString().slice(0, 10);
  r = await ev({ title: `Standup ${stamp}`, start: iso(new Date(mon.getTime() - 3600000)), end: iso(new Date(mon.getTime() - 3600000 + 15 * 60000)), repeat: { freq: 'weekly', weekdays: [1, 3], ends_on: until } });
  const standup = r.data;
  ok('repeating event', r.status === 200 && /Every week on Mon, Wed/.test(standup.repeat), r.data);
  const month = [new Date(mon.getTime() - day), new Date(mon.getTime() + 30 * day)] as const;
  let occ = (await range(ann, ...month)).data.items.filter((e: any) => e.id === standup.id);
  ok('six occurrences until the end date', occ.length === 6 && occ.every((e: any) => e.recurring && e.occurrence && new Date(e.start).getUTCHours() === 9), occ.map((e: any) => e.start));
  await ann.req('DELETE', `/calendar/events/${standup.id}?occurrence=${encodeURIComponent(occ[1].occurrence)}`);
  occ = (await range(ann, ...month)).data.items.filter((e: any) => e.id === standup.id);
  ok('one occurrence removed', occ.length === 5);

  // ---- all-day ----
  const d1 = new Date(mon.getTime() + 3 * day).toISOString().slice(0, 10), d2 = new Date(mon.getTime() + 4 * day).toISOString().slice(0, 10);
  r = await ev({ title: `Offsite ${stamp}`, all_day: true, start: d1, end: d2 });
  ok('all-day event over two days', r.data.all_day && r.data.starts_at.startsWith(d1) && new Date(r.data.ends_at).getTime() - new Date(r.data.starts_at).getTime() === 2 * day, r.data);
  ok('days checked', (await ev({ title: `X ${stamp}`, all_day: true, start: d2, end: d1 })).status === 400);

  // ---- moving ----
  const later = new Date(mon.getTime() + 4 * 3600000);
  r = await ann.patch(`/calendar/events/${kick.id}`, { start: iso(later), end: iso(new Date(later.getTime() + 3600000)) });
  ok('moved: guests asked again', new Date(r.data.starts_at).getTime() === later.getTime() && r.data.people.find((p: any) => p.id === bob.id).status === 'pending', r.data);
  ok('and told', !!(await waitFor(async () => (await notes(bob, `Ann moved “Kickoff ${stamp}”`))[0], 5000)));
  r = await ann.patch(`/calendar/events/${kick.id}`, { attendees: [bob.id, eve.id] });
  ok('a guest added later is invited', r.data.people.length === 3 && !!(await waitFor(async () => (await notes(eve, `Kickoff ${stamp}`))[0], 5000)));

  // ---- tasks and CRM in the view ----
  await ann.post('/tasks', { title: `Report ${stamp}`, responsible_id: ann.id, deadline: iso(new Date(mon.getTime() + day)) });
  await admin.patch(`/crm/deal/items/${deal.id}`, { assigned_to: ann.id });
  await ann.post('/crm/activities', { item_id: deal.id, kind: 'meeting', subject: `Demo ${stamp}`, starts_at: iso(new Date(mon.getTime() + 2 * day)), ends_at: iso(new Date(mon.getTime() + 2 * day + 3600000)), responsible_id: ann.id });
  r = await range(ann, ...week, '&extras=true');
  ok('task deadlines and CRM meetings', r.data.items.some((e: any) => e.kind === 'task' && e.title.includes(`Report ${stamp}`) && e.link.startsWith('/tasks/')) && r.data.items.some((e: any) => e.kind === 'activity' && e.title === `Meeting: Demo ${stamp}` && e.link === `/crm/deal/${deal.id}`), r.data.items.map((e: any) => [e.kind, e.title]));
  ok('not without asking', !(await range(ann, ...week)).data.items.some((e: any) => e.kind !== 'event'));

  // ---- subscription feed ----
  ok('no link yet', (await ann.get('/calendar/feed')).data.active === false);
  const f1 = (await ann.post('/calendar/feed', {})).data;
  const path = (u: string) => u.replace(/^https?:\/\/[^/]+/, 'http://localhost:8080');
  let res = await fetch(path(f1.url));
  let ics = await res.text();
  ok('feed', res.status === 200 && res.headers.get('content-type')?.startsWith('text/calendar') && ics.startsWith('BEGIN:VCALENDAR\r\n') && ics.includes(`SUMMARY:Kickoff ${stamp}`) && ics.includes(`SUMMARY:Meeting: Demo ${stamp}`) && (ics.match(new RegExp(`SUMMARY:Standup ${stamp}`, 'g')) ?? []).length === 5 && ics.includes('DTSTART;VALUE=DATE:'), ics.slice(0, 600));
  ok('the link is only shown when made', !('url' in (await ann.get('/calendar/feed')).data));
  await bob.post(`/calendar/events/${kick.id}/respond`, { status: 'declined' });
  const fb = (await bob.post('/calendar/feed', {})).data;
  ok('declined events are left out of a guest\'s feed', !(await (await fetch(path(fb.url))).text()).includes(`Kickoff ${stamp}`));
  const f2 = (await ann.post('/calendar/feed', {})).data;
  ok('a new link replaces the old', (await fetch(path(f1.url))).status === 404 && (await fetch(path(f2.url))).status === 200);
  await ann.req('DELETE', '/calendar/feed');
  ok('turned off', (await fetch(path(f2.url))).status === 404);

  // ---- reminders ----
  const soon = new Date(Date.now() + 5 * 60000);
  await ev({ title: `Call soon ${stamp}`, start: iso(soon), end: iso(new Date(soon.getTime() + 600000)), remind_minutes: 10, attendees: [bob.id] });
  const rem = await waitFor(async () => (await notes(ann, `Call soon ${stamp}`)).find((n: any) => /^In \d+ min:/.test(n.title)), 90000);
  ok('reminder', !!rem, rem);
  ok('guests reminded too', (await notes(bob, `Call soon ${stamp}`)).some((n: any) => /^In \d+ min:/.test(n.title)));
  await new Promise((res) => setTimeout(res, 65000));
  ok('only once', (await notes(ann, `Call soon ${stamp}`)).filter((n: any) => /^In \d+ min:/.test(n.title)).length === 1);

  // ---- deleting ----
  await ann.req('DELETE', `/calendar/events/${kick.id}`);
  ok('deleted; guests told', (await ann.get(`/calendar/events/${kick.id}`)).status === 404 && !!(await waitFor(async () => (await notes(eve, `Ann cancelled “Kickoff ${stamp}”`))[0], 5000)));
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
