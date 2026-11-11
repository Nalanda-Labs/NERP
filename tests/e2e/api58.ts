// Calendar depth: guests invited by email (iCalendar invitations, updates and cancellations,
// the signed answer link), meeting rooms shared with the booking module (no double booking
// either way), who may see or change a person's calendar, and slot links (open slots and
// slots shared by several people) booked from a public page.
import pg from 'pg';
import { Client, sleep } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const B = 'http://localhost:8080/api';
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Cal${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [mia, sam, tom] = await Promise.all(['Mia', 'Sam', 'Tom'].map(user));
const pub = async (method: string, path: string, body?: unknown) => {
  const r = await fetch(B + path, { method, headers: body ? { 'content-type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined });
  const t = await r.text();
  let data: any; try { data = JSON.parse(t); } catch { data = t; }
  return { status: r.status, data };
};
/** Emails queued to an address (oldest first). */
const mails = async (to: string) => (await db.query(`SELECT payload FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1 ORDER BY created_at, id`, [to])).rows
  // iCalendar lines are folded at 75 octets; unfold them to read.
  .map((r) => ({ ...r.payload, calendar: r.payload.calendar?.replace(/\r\n /g, '') }));
const waitMails = async (to: string, n: number) => { for (let i = 0; i < 40; i++) { const m = await mails(to); if (m.length >= n) return m; await sleep(100); } return mails(to); };
const tokenIn = (m: any) => m.text.match(/\/rsvp\/([0-9a-f-]+\.[0-9a-f]+)/)?.[1];
// Next Monday, at a UTC hour (days later with `days`).
const monday = (() => { const d = new Date(); d.setUTCHours(0, 0, 0, 0); d.setUTCDate(d.getUTCDate() + ((8 - d.getUTCDay()) % 7 || 7)); return d; })();
const at = (h: number, m = 0, days = 0) => new Date(+monday + days * 86400e3 + (h * 60 + m) * 60e3).toISOString().replace('.000Z', 'Z');
const iso = (s: string) => new Date(s).toISOString();

try {
  // ---- guests by email ----
  const ann = `ann.${stamp}@example.com`, bob = `bob.${stamp}@example.com`;
  ok('a guest address must be an address', (await mia.post('/calendar/events', { title: `Bad ${stamp}`, start: at(9), end: at(10), timezone: 'UTC', email_guests: [{ email: 'nope' }] })).status === 400);
  let r = await mia.post('/calendar/events', { title: `Kick-off ${stamp}`, start: at(9), end: at(10), timezone: 'Europe/Berlin', location: 'Online', attendees: [sam.id], email_guests: [{ email: ` ${ann.toUpperCase()} `, name: 'Ann' }, { email: bob }] });
  const ev = r.data;
  ok('an event with guests by email', r.status === 200 && ev.email_guests.length === 2 && ev.email_guests.some((g: any) => g.email === ann && g.name === 'Ann' && g.status === 'pending'), r.data);
  let m = await waitMails(ann, 1);
  const inv = m[0];
  ok('each gets an invitation email', inv?.subject === `Invitation: Kick-off ${stamp}` && /Accept or decline/.test(inv.html), inv);
  ok('with an iCalendar request: organiser, every guest, the sequence', /METHOD:REQUEST/.test(inv.calendar) && inv.calendar.includes(`UID:${ev.id}@nerp`) && /SEQUENCE:0/.test(inv.calendar)
    && inv.calendar.includes(`mailto:mia.${stamp}@acme.test`) && inv.calendar.includes(`mailto:${ann}`) && inv.calendar.includes(`mailto:sam.${stamp}@acme.test`) && /DTSTART:\d{8}T090000Z/.test(inv.calendar), inv.calendar);
  const annToken = tokenIn(inv);
  ok('and an answer link', !!annToken, inv.text);
  r = await pub('GET', `/public/calendar/rsvp/${annToken}`);
  ok('the link shows the meeting', r.status === 200 && r.data.title === `Kick-off ${stamp}` && r.data.organizer === `Mia Cal${stamp}` && r.data.location === 'Online' && r.data.guest.status === 'pending', r.data);
  ok('a tampered link finds nothing', (await pub('GET', `/public/calendar/rsvp/${annToken.slice(0, -2)}00`)).status === 404);
  ok('only real answers', (await pub('POST', `/public/calendar/rsvp/${annToken}`, { status: 'maybe' })).status === 400);
  r = await pub('POST', `/public/calendar/rsvp/${annToken}`, { status: 'accepted' });
  ok('the guest accepts', r.status === 200 && r.data.guest.status === 'accepted', r.data);
  ok('the organiser sees it', (await mia.get(`/calendar/events/${ev.id}`)).data.email_guests.find((g: any) => g.email === ann).status === 'accepted');
  const note = (await mia.get('/notifications')).data;
  ok('and is told', JSON.stringify(note).includes(`Ann accepted “Kick-off ${stamp}”`), note);
  const dl = await fetch(`${B}/public/calendar/rsvp/${annToken}/invite.ics`);
  ok('the invitation can be downloaded', dl.status === 200 && /text\/calendar/.test(dl.headers.get('content-type') ?? '') && (await dl.text()).includes('BEGIN:VEVENT'));

  // Moving it: an update with a higher sequence, answers asked again; a removed guest is told.
  r = await mia.patch(`/calendar/events/${ev.id}`, { start: at(11), end: at(12), email_guests: [{ email: ann, name: 'Ann' }] });
  ok('moved, one guest removed', r.status === 200 && r.data.email_guests.length === 1 && r.data.email_guests[0].status === 'pending' && r.data.sequence === 1, r.data);
  m = await waitMails(ann, 2);
  ok('the guest gets the update', m[1]?.subject === `Updated: Kick-off ${stamp}` && /SEQUENCE:1/.test(m[1].calendar) && /DTSTART:\d{8}T110000Z/.test(m[1].calendar), m[1]);
  m = await waitMails(bob, 2);
  ok('the removed guest gets a cancellation', m[1]?.subject === `Cancelled: Kick-off ${stamp}` && /METHOD:CANCEL/.test(m[1].calendar) && /STATUS:CANCELLED/.test(m[1].calendar), m[1]);
  r = await mia.patch(`/calendar/events/${ev.id}`, { description: 'Agenda to follow' });
  ok('a change to the notes alone sends nothing', r.data.sequence === 1 && (await mails(ann)).length === 2, r.data);

  // A repeating event: written in local time with its zone, one occurrence cancelled.
  r = await mia.post('/calendar/events', { title: `Weekly ${stamp}`, start: at(8), end: at(8, 30), timezone: 'Europe/Berlin', repeat: { freq: 'weekly', weekdays: [1], max_runs: 4 }, email_guests: [{ email: ann }] });
  const weekly = r.data;
  m = await waitMails(ann, 3);
  ok('a repeating invitation keeps local time', /RRULE:FREQ=WEEKLY;BYDAY=MO;WKST=MO;COUNT=4/.test(m[2]?.calendar) && /DTSTART;TZID=Europe\/Berlin:\d{8}T(09|10)0000/.test(m[2]?.calendar) && /BEGIN:VTIMEZONE/.test(m[2]?.calendar), m[2]?.calendar);
  await mia.del(`/calendar/events/${weekly.id}?occurrence=${encodeURIComponent(iso(at(8, 0, 7)))}`);
  m = await waitMails(ann, 4);
  ok('one cancelled occurrence', /METHOD:CANCEL/.test(m[3]?.calendar) && /RECURRENCE-ID;TZID=Europe\/Berlin:/.test(m[3]?.calendar) && !/RRULE/.test(m[3]?.calendar), m[3]?.calendar);
  await mia.del(`/calendar/events/${weekly.id}`);
  m = await waitMails(ann, 5);
  ok('the whole series cancelled', m[4]?.subject === `Cancelled: Weekly ${stamp}` && /STATUS:CANCELLED/.test(m[4]?.calendar), m[4]);

  // ---- meeting rooms ----
  const room = (await admin.post('/booking/resources', { name: `Room ${stamp}`, kind: 'room', capacity: 1, timezone: 'UTC', min_notice_minutes: 0 })).data;
  const desk = (await admin.post('/booking/resources', { name: `Desk ${stamp}`, kind: 'equipment', timezone: 'UTC' })).data;
  ok('only rooms are meeting rooms', (await mia.post('/calendar/events', { title: `Desk ${stamp}`, start: at(9, 0, 1), end: at(10, 0, 1), timezone: 'UTC', room_id: desk.id })).status === 400);
  r = await mia.post('/calendar/events', { title: `Planning ${stamp}`, start: at(9, 0, 1), end: at(10, 0, 1), timezone: 'UTC', room_id: room.id, location: 'Floor 2' });
  const planning = r.data;
  ok('an event in a room', r.status === 200 && planning.room?.name === `Room ${stamp}`, r.data);
  r = await sam.post('/calendar/events', { title: `Clash ${stamp}`, start: at(9, 30, 1), end: at(10, 30, 1), timezone: 'UTC', room_id: room.id });
  ok('the room can\'t be taken twice', r.status === 409 && /is taken/.test(JSON.stringify(r.data)), r.data);
  ok('right after it is fine', (await sam.post('/calendar/events', { title: `After ${stamp}`, start: at(10, 0, 1), end: at(11, 0, 1), timezone: 'UTC', room_id: room.id })).status === 200);
  r = await sam.post('/booking/bookings', { resource_ids: [room.id], starts_at: at(9, 15, 1), ends_at: at(9, 45, 1), client_name: `Client ${stamp}` });
  ok('a booking can\'t take it either', r.status === 409, r.data);
  r = await sam.get(`/calendar/rooms?from=${encodeURIComponent(at(9, 0, 1))}&to=${encodeURIComponent(at(10, 0, 1))}`);
  const mine = r.data.items.find((x: any) => x.id === room.id);
  ok('the room list shows it taken then', mine && mine.free === false && !r.data.items.some((x: any) => x.id === desk.id), r.data);
  r = await sam.get(`/calendar/rooms?from=${encodeURIComponent(at(9, 0, 1))}&to=${encodeURIComponent(at(10, 0, 1))}&except=${planning.id}`);
  ok('…but free for the event itself', r.data.items.find((x: any) => x.id === room.id)?.free === true, r.data);
  const view = (await mia.get(`/calendar?from=${encodeURIComponent(at(0, 0, 1))}&to=${encodeURIComponent(at(23, 0, 1))}`)).data;
  ok('the calendar shows the room as the place', view.items.find((x: any) => x.id === planning.id)?.location === `Room ${stamp}, Floor 2`, view.items);
  r = await admin.post('/booking/bookings', { resource_ids: [room.id], starts_at: at(14, 0, 8), ends_at: at(15, 0, 8), client_name: `Client ${stamp}` });
  ok('a booking in the room', r.status === 200, r.data);
  r = await mia.post('/calendar/events', { title: `Daily ${stamp}`, start: at(14, 0, 2), end: at(14, 30, 2), timezone: 'UTC', room_id: room.id, repeat: { freq: 'daily', max_runs: 10 } });
  ok('a repeating event clashing with a later booking is refused, naming the day', r.status === 409 && /is taken on/.test(JSON.stringify(r.data)), r.data);
  ok('moving into a taken room is refused too', (await mia.patch(`/calendar/events/${planning.id}`, { start: at(10, 30, 1), end: at(11, 30, 1) })).status === 409);

  // ---- who may see or change a calendar ----
  await mia.post('/calendar/events', { title: `Secret plan ${stamp}`, start: at(13, 0, 3), end: at(14, 0, 3), timezone: 'UTC' });
  const range = `from=${encodeURIComponent(at(0, 0, 3))}&to=${encodeURIComponent(at(23, 0, 3))}&users=${mia.id}`;
  let seen = (await sam.get(`/calendar?${range}`)).data;
  ok('by default colleagues see details', seen.items.some((x: any) => x.title === `Secret plan ${stamp}`), seen);
  ok('everyone can\'t get edit', (await mia.put('/calendar/access', { everyone: 'edit' })).status === 400);
  r = await mia.put('/calendar/access', { everyone: 'busy', grants: [{ kind: 'user', id: tom.id, level: 'edit' }] });
  ok('Mia shares busy times only, and lets Tom change her calendar', r.status === 200 && r.data.everyone === 'busy' && r.data.grants[0].name === `Tom Cal${stamp}`, r.data);
  seen = (await sam.get(`/calendar?${range}`)).data;
  const busyOne = seen.items.find((x: any) => x.user_id === mia.id && x.busy);
  ok('Sam sees only "Busy"', busyOne?.title === 'Busy' && !seen.items.some((x: any) => x.title === `Secret plan ${stamp}`), seen.items);
  ok('and can\'t open the event', (await sam.get(`/calendar/events/${busyOne.id}`)).status === 404);
  seen = (await tom.get(`/calendar?${range}`)).data;
  ok('Tom sees the details', seen.items.some((x: any) => x.title === `Secret plan ${stamp}`), seen.items);
  const secret = seen.items.find((x: any) => x.title === `Secret plan ${stamp}`);
  ok('and may change it', (await tom.get(`/calendar/events/${secret.id}`)).data.can_edit === true);
  r = await tom.patch(`/calendar/events/${secret.id}`, { title: `Secret plan B ${stamp}` });
  ok('Tom renames it', r.status === 200 && r.data.title === `Secret plan B ${stamp}`, r.data);
  r = await tom.post('/calendar/events', { title: `For Mia ${stamp}`, start: at(15, 0, 3), end: at(16, 0, 3), timezone: 'UTC', calendar_id: mia.id });
  ok('and puts an event on her calendar', r.status === 200 && r.data.owner_id === mia.id, r.data);
  ok('Sam can\'t', (await sam.post('/calendar/events', { title: `Nope ${stamp}`, start: at(15, 0, 3), end: at(16, 0, 3), timezone: 'UTC', calendar_id: mia.id })).status === 403);
  ok('nor change hers', (await sam.patch(`/calendar/events/${secret.id}`, { title: 'x' })).status !== 200);
  const miaNotes = (await mia.get('/notifications')).data;
  ok('Mia is told what Tom did', JSON.stringify(miaNotes).includes(`put on your calendar “For Mia ${stamp}”`) && JSON.stringify(miaNotes).includes('changed “Secret plan B'), miaNotes);
  ok('Tom finds it shared with him', (await tom.get('/calendar/shared-with-me')).data.items.some((x: any) => x.id === mia.id && x.level === 'edit'));
  await mia.put('/calendar/access', { everyone: 'none' });
  seen = (await sam.get(`/calendar?${range}`)).data;
  ok('a closed calendar shows nothing', seen.closed.includes(mia.id) && !seen.items.some((x: any) => x.user_id === mia.id), seen);
  ok('shared events still show to their guests', (await sam.get(`/calendar/events/${ev.id}`)).status === 200);
  const dept = (await admin.post('/departments', { name: `Team ${stamp}` })).data;
  await admin.post(`/departments/${dept.id}/members`, { user_id: sam.id });
  await mia.put('/calendar/access', { everyone: 'none', grants: [{ kind: 'department', id: dept.id, level: 'read' }] });
  seen = (await sam.get(`/calendar?${range}`)).data;
  ok('a department can be let in', seen.items.some((x: any) => x.title === `Secret plan B ${stamp}`), seen);
  await mia.put('/calendar/access', { everyone: 'read', grants: [{ kind: 'department', id: dept.id, level: 'read' }, { kind: 'user', id: sam.id, level: 'busy' }] });
  seen = (await sam.get(`/calendar?${range}`)).data;
  ok('a person\'s own level wins over their department\'s', !seen.items.some((x: any) => x.title === `Secret plan B ${stamp}`) && seen.items.some((x: any) => x.busy), seen.items);
  await mia.put('/calendar/access', { everyone: 'read' });

  // ---- slot links ----
  const allDay = [1, 2, 3, 4, 5, 6, 7].map((d) => [d, 0, 1440]);
  ok('hours are checked', (await mia.post('/calendar/slot-links', { title: `Bad ${stamp}`, hours: [[8, 0, 60]] })).status === 400);
  await sam.put('/calendar/access', { everyone: 'none' });
  ok('members must share their calendar with the owner', (await mia.post('/calendar/slot-links', { title: `Shared ${stamp}`, members: [sam.id], hours: allDay, timezone: 'UTC' })).status === 400);
  await sam.put('/calendar/access', { everyone: 'read' });
  r = await mia.post('/calendar/slot-links', { title: `Intro call ${stamp}`, members: [sam.id], hours: allDay, timezone: 'UTC', duration_minutes: 30, slot_step_minutes: 30, min_notice_minutes: 0, days_ahead: 60, max_per_day: 2 });
  const link = r.data;
  ok('a link with slots shared by Mia and Sam', r.status === 200 && link.people.length === 2 && link.url.endsWith(`/meet/${link.slug}`), r.data);
  ok('Sam sees it in his list too', (await sam.get('/calendar/slot-links')).data.items.some((x: any) => x.id === link.id));
  ok('but can\'t change it', (await sam.patch(`/calendar/slot-links/${link.id}`, { title: 'x' })).status === 403);
  const d4 = at(0, 0, 4).slice(0, 10);
  r = await pub('GET', `/public/calendar/meet/${link.slug}`);
  ok('the public page', r.status === 200 && r.data.title === `Intro call ${stamp}` && r.data.people.includes(`Sam Cal${stamp}`) && r.data.duration_minutes === 30, r.data);
  r = await pub('GET', `/public/calendar/meet/${link.slug}/slots?from=${d4}&to=${d4}`);
  let free: string[] = r.data.days[0].slots.map(iso);
  ok('free slots all day', free.length === 48 && free.includes(iso(at(9, 0, 4))), r.data);
  await sam.post('/calendar/events', { title: `Sam busy ${stamp}`, start: at(9, 0, 4), end: at(10, 0, 4), timezone: 'UTC' });
  await mia.post('/calendar/events', { title: `Mia busy ${stamp}`, start: at(12, 0, 4), end: at(12, 30, 4), timezone: 'UTC', attendees: [tom.id] });
  free = (await pub('GET', `/public/calendar/meet/${link.slug}/slots?from=${d4}&to=${d4}`)).data.days[0].slots.map(iso);
  ok('times either of them is busy are gone', !free.includes(iso(at(9, 0, 4))) && !free.includes(iso(at(9, 30, 4))) && !free.includes(iso(at(12, 0, 4))) && free.includes(iso(at(10, 0, 4))) && free.includes(iso(at(12, 30, 4))), free);
  ok('at most 31 days at once', (await pub('GET', `/public/calendar/meet/${link.slug}/slots?from=${d4}&to=2099-01-01`)).status === 400);
  ok('bots are refused', (await pub('POST', `/public/calendar/meet/${link.slug}`, { starts_at: at(10, 0, 4), name: 'Bot', email: `bot.${stamp}@example.com`, website: 'x' })).status === 400);
  ok('a taken time is refused', (await pub('POST', `/public/calendar/meet/${link.slug}`, { starts_at: at(9, 0, 4), name: 'Zoe', email: `zoe.${stamp}@example.com` })).status === 409);
  const zoe = `zoe.${stamp}@example.com`;
  r = await pub('POST', `/public/calendar/meet/${link.slug}`, { starts_at: at(10, 0, 4), name: `Zoe ${stamp}`, email: zoe, note: 'About pricing' });
  ok('Zoe books 10:00', r.status === 200 && iso(r.data.starts_at) === iso(at(10, 0, 4)) && iso(r.data.ends_at) === iso(at(10, 30, 4)) && /\/rsvp\//.test(r.data.manage_url), r.data);
  const manage = r.data.manage_url.split('/rsvp/')[1];
  ok('the same time again is refused', (await pub('POST', `/public/calendar/meet/${link.slug}`, { starts_at: at(10, 0, 4), name: 'Yan', email: `yan.${stamp}@example.com` })).status === 409);
  const day4 = (await mia.get(`/calendar?from=${encodeURIComponent(at(0, 0, 4))}&to=${encodeURIComponent(at(23, 0, 4))}`)).data;
  const booked = day4.items.find((x: any) => x.title === `Intro call ${stamp}: Zoe ${stamp}`);
  ok('a meeting on Mia\'s calendar', !!booked, day4.items);
  const bookedEv = (await mia.get(`/calendar/events/${booked.id}`)).data;
  ok('Sam invited (accepted), Zoe as an email guest, the note kept', bookedEv.people.some((p: any) => p.id === sam.id && p.status === 'accepted') && bookedEv.email_guests[0]?.email === zoe && /About pricing/.test(bookedEv.description), bookedEv);
  m = await waitMails(zoe, 1);
  ok('Zoe gets the invitation', m[0]?.subject === `Invitation: Intro call ${stamp}: Zoe ${stamp}` && /METHOD:REQUEST/.test(m[0].calendar), m[0]);
  ok('Sam is told', JSON.stringify((await sam.get('/notifications')).data).includes(`Zoe ${stamp} booked “Intro call ${stamp}”`));
  await pub('POST', `/public/calendar/meet/${link.slug}`, { starts_at: at(11, 0, 4), name: `Uma ${stamp}`, email: `uma.${stamp}@example.com` });
  free = (await pub('GET', `/public/calendar/meet/${link.slug}/slots?from=${d4}&to=${d4}`)).data.days[0].slots;
  ok('two a day at most', free.length === 0, free);
  r = await pub('POST', `/public/calendar/rsvp/${manage}`, { status: 'declined' });
  ok('Zoe cancels through her link', r.status === 200 && r.data.cancelled === true, r.data);
  ok('the meeting is gone', (await mia.get(`/calendar/events/${booked.id}`)).status === 404);
  ok('so the day has room again', (await pub('GET', `/public/calendar/meet/${link.slug}/slots?from=${d4}&to=${d4}`)).data.days[0].slots.length > 0);
  await mia.patch(`/calendar/slot-links/${link.id}`, { is_active: false });
  ok('a switched-off link is gone', (await pub('GET', `/public/calendar/meet/${link.slug}`)).status === 404);
  ok('and can be deleted', (await mia.del(`/calendar/slot-links/${link.id}`)).status === 200);
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
