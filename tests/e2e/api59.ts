// CalDAV: app passwords; discovery (well-known, principal, calendar home, the calendar);
// events made in NERP read by apps; events made in apps (repeats, removed and changed
// occurrences, alarms, guests) kept in NERP; preconditions and UID conflicts; a guest's app
// answering; queries, multiget and sync (changes and deletions); and a real CalDAV client
// (tsdav) discovering, reading, creating, changing, syncing and deleting.
import pg from 'pg';
import { createDAVClient } from 'tsdav';
import { Client, sleep } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const BASE = 'http://localhost:8080';
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Dav${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  c.email = email;
  return c;
};
const [ida, ken] = await Promise.all(['Ida', 'Ken'].map(user));
const auth = (email: string, pw: string) => 'Basic ' + Buffer.from(`${email}:${pw}`).toString('base64');
const dav = async (method: string, path: string, opts: { pw?: string; who?: any; body?: string; depth?: string; headers?: Record<string, string> } = {}) => {
  const who = opts.who ?? ida;
  const headers: Record<string, string> = { ...(opts.headers ?? {}) };
  if (opts.pw !== '') headers.authorization = auth(who.email, opts.pw ?? who.pw);
  if (opts.depth) headers.depth = opts.depth;
  if (opts.body) headers['content-type'] = method === 'PUT' ? 'text/calendar; charset=utf-8' : 'application/xml; charset=utf-8';
  const r = await fetch(BASE + path, { method, headers, body: opts.body, redirect: 'manual' });
  return { status: r.status, headers: r.headers, text: await r.text() };
};
const unfold = (s: string) => s.replace(/\r\n[ \t]/g, '');
const propfind = (props: string) => `<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/"><d:prop>${props}</d:prop></d:propfind>`;
const monday = (() => { const d = new Date(); d.setUTCHours(0, 0, 0, 0); d.setUTCDate(d.getUTCDate() + ((8 - d.getUTCDay()) % 7 || 7)); return d; })();
const at = (h: number, m = 0, days = 0) => new Date(+monday + days * 86400e3 + (h * 60 + m) * 60e3);
const ics = (t: Date) => t.toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '');
const local = (t: Date) => ics(t).replace('Z', '');
const range = (from: Date, to: Date) => `from=${encodeURIComponent(from.toISOString())}&to=${encodeURIComponent(to.toISOString())}`;

try {
  // ---- app passwords ----
  let r = await ida.post('/auth/app-passwords', { name: `Phone ${stamp}` });
  ida.pw = r.data.password;
  ok('an app password, shown once, easy to type', r.status === 200 && /^[a-z]{4}(-[a-z]{4}){3}$/.test(ida.pw), r.data);
  r = await ida.get('/auth/app-passwords');
  ok('listed without the password', r.data.items.length === 1 && !JSON.stringify(r.data).includes(ida.pw), r.data);
  ken.pw = (await ken.post('/auth/app-passwords', { name: `Laptop ${stamp}` })).data.password;
  ok('it can\'t sign in to NERP itself', (await new Client().post('/auth/login', { email: ida.email, password: ida.pw })).status === 401);

  // ---- discovery ----
  let d = await dav('GET', '/.well-known/caldav', { pw: '' });
  ok('the well-known address points to /dav/', d.status === 301 && d.headers.get('location') === '/dav/', d.status);
  d = await dav('OPTIONS', '/dav/', { pw: '' });
  ok('OPTIONS says it speaks CalDAV', /calendar-access/.test(d.headers.get('dav') ?? ''), [...d.headers]);
  d = await dav('PROPFIND', '/dav/', { pw: '', body: propfind('<d:current-user-principal/>'), depth: '0' });
  ok('signing in is needed', d.status === 401 && /Basic/.test(d.headers.get('www-authenticate') ?? ''), d.status);
  ok('a wrong password is refused', (await dav('PROPFIND', '/dav/', { pw: 'aaaa-bbbb-cccc-dddd', depth: '0' })).status === 401);
  ok('typing it in capitals with spaces works', (await dav('PROPFIND', '/dav/', { pw: ida.pw.toUpperCase().replace(/-/g, ' '), depth: '0' })).status === 207);
  d = await dav('PROPFIND', '/dav/', { body: propfind('<d:current-user-principal/>'), depth: '0' });
  ok('the principal is found', d.status === 207 && d.text.includes(`/dav/principals/${ida.id}/`), d.text);
  d = await dav('PROPFIND', `/dav/principals/${ida.id}/`, { body: propfind('<c:calendar-home-set/><c:calendar-user-address-set/><d:displayname/><d:unknown-thing/>'), depth: '0' });
  ok('the principal has a calendar home and address', d.text.includes(`/dav/calendars/${ida.id}/`) && d.text.includes(`mailto:${ida.email}`) && d.text.includes(`Ida Dav${stamp}`) && /404 Not Found/.test(d.text), d.text);
  d = await dav('PROPFIND', `/dav/calendars/${ida.id}/`, { body: propfind('<d:resourcetype/><d:displayname/><cs:getctag/><d:sync-token/><c:supported-calendar-component-set/>'), depth: '1' });
  ok('the home holds one calendar for events', d.text.includes(`/dav/calendars/${ida.id}/default/`) && /<c:calendar\/>/.test(d.text) && /name="VEVENT"/.test(d.text) && /urn:nerp:sync:\d+/.test(d.text), d.text);
  ok('another person\'s calendar is off limits', (await dav('PROPFIND', `/dav/calendars/${ken.id}/default/`, { depth: '0' })).status === 403);

  // ---- an event made in NERP, read by the app ----
  const nerpEv = (await ida.post('/calendar/events', { title: `Made in NERP ${stamp}`, start: at(9).toISOString(), end: at(10).toISOString(), timezone: 'UTC', attendees: [ken.id], email_guests: [{ email: `out.${stamp}@example.com` }], remind_minutes: 10 })).data;
  d = await dav('PROPFIND', `/dav/calendars/${ida.id}/default/`, { body: propfind('<d:getetag/>'), depth: '1' });
  ok('it is listed with an ETag', d.text.includes(`${nerpEv.id}.ics`) && /<d:getetag>&quot;\d+&quot;<\/d:getetag>/.test(d.text), d.text);
  d = await dav('GET', `/dav/calendars/${ida.id}/default/${nerpEv.id}.ics`);
  let body = unfold(d.text);
  ok('GET gives the event with its guests and alarm', d.status === 200 && body.includes(`UID:${nerpEv.id}@nerp`) && body.includes(`SUMMARY:Made in NERP ${stamp}`) && body.includes(`mailto:${ken.email}`) && body.includes(`mailto:out.${stamp}@example.com`) && body.includes('TRIGGER:-PT10M') && !!d.headers.get('etag'), body);
  const etag1 = d.headers.get('etag')!;
  await ida.patch(`/calendar/events/${nerpEv.id}`, { title: `Renamed in NERP ${stamp}` });
  d = await dav('GET', `/dav/calendars/${ida.id}/default/${nerpEv.id}.ics`);
  ok('a change in NERP changes the ETag', d.headers.get('etag') !== etag1 && d.text.includes(`Renamed in NERP ${stamp}`), d.headers.get('etag'));

  // ---- an event made in an app ----
  const uid = `app-${stamp}@example.org`;
  const series = [
    'BEGIN:VCALENDAR', 'VERSION:2.0', 'PRODID:-//Test//EN',
    'BEGIN:VTIMEZONE', 'TZID:Europe/Berlin', 'BEGIN:STANDARD', 'DTSTART:19701025T030000', 'TZOFFSETFROM:+0200', 'TZOFFSETTO:+0100', 'END:STANDARD', 'END:VTIMEZONE',
    'BEGIN:VEVENT', `UID:${uid}`, `DTSTAMP:${ics(new Date())}`, `DTSTART;TZID=Europe/Berlin:${local(at(10))}`, `DTEND;TZID=Europe/Berlin:${local(at(10, 30))}`,
    `SUMMARY:App series ${stamp}`, 'DESCRIPTION:Made\\, in an app', 'RRULE:FREQ=DAILY;COUNT=5', `EXDATE;TZID=Europe/Berlin:${local(at(10, 0, 1))}`,
    `ATTENDEE;CN=Ken;PARTSTAT=NEEDS-ACTION:mailto:${ken.email}`, `ATTENDEE;CN=Pia Outside;PARTSTAT=ACCEPTED:mailto:pia.${stamp}@example.com`,
    'BEGIN:VALARM', 'ACTION:DISPLAY', 'TRIGGER:-PT15M', 'DESCRIPTION:x', 'END:VALARM', 'END:VEVENT',
    'BEGIN:VEVENT', `UID:${uid}`, `DTSTAMP:${ics(new Date())}`, `RECURRENCE-ID;TZID=Europe/Berlin:${local(at(10, 0, 2))}`, `DTSTART;TZID=Europe/Berlin:${local(at(15, 0, 2))}`, `DTEND;TZID=Europe/Berlin:${local(at(16, 0, 2))}`,
    `SUMMARY:App series moved ${stamp}`, 'END:VEVENT', 'END:VCALENDAR', ''].join('\r\n');
  const path = `/dav/calendars/${ida.id}/default/${encodeURIComponent(`my event ${stamp}.ics`)}`;
  d = await dav('PUT', path, { body: series, headers: { 'if-none-match': '*' } });
  ok('PUT makes a new event', d.status === 201, d);
  const week = (await ida.get(`/calendar?${range(at(0), at(0, 0, 7))}`)).data.items;
  const occ = week.filter((x: any) => x.title === `App series ${stamp}`);
  const moved = week.find((x: any) => x.title === `App series moved ${stamp}`);
  ok('NERP shows the repeats, minus the removed and changed ones', occ.length === 3 && !occ.some((x: any) => +new Date(x.start) === +new Date(at(10, 0, 1).getTime() - 3600e3)) , occ.map((x: any) => x.start));
  ok('Berlin time kept: 10:00 there is 09:00 UTC in winter', occ.every((x: any) => new Date(x.start).getUTCHours() === 9), occ.map((x: any) => x.start));
  ok('the changed occurrence on its own', moved && new Date(moved.start).getUTCHours() === 14, moved);
  const appEv = (await ida.get(`/calendar/events/${occ[0].id}`)).data;
  ok('the alarm, notes and guests are kept', appEv.remind_minutes === 15 && appEv.description === 'Made, in an app' && appEv.people.some((p: any) => p.id === ken.id && p.status === 'pending')
    && appEv.email_guests.some((g: any) => g.email === `pia.${stamp}@example.com` && g.status === 'accepted'), appEv);
  ok('Ken is told in NERP', JSON.stringify((await ken.get('/notifications')).data).includes(`App series ${stamp}`));
  await sleep(300);
  ok('no email goes to the outside guest (the app invites them)', (await db.query(`SELECT 1 FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1`, [`pia.${stamp}@example.com`])).rowCount === 0);
  d = await dav('GET', path);
  body = unfold(d.text);
  ok('GET returns it under the app\'s name and UID, with the rule and both kinds of occurrences', d.status === 200 && body.includes(`UID:${uid}`) && body.includes('RRULE:FREQ=DAILY;COUNT=5')
    && /EXDATE;TZID=Europe\/Berlin:\d{8}T100000/.test(body) && /RECURRENCE-ID;TZID=Europe\/Berlin:\d{8}T100000/.test(body) && body.includes(`SUMMARY:App series moved ${stamp}`), body);
  const etag = d.headers.get('etag')!;
  ok('a stale ETag is refused', (await dav('PUT', path, { body: series.replace('App series', 'Stale'), headers: { 'if-match': '"1"' } })).status === 412);
  ok('creating over it is refused', (await dav('PUT', path, { body: series, headers: { 'if-none-match': '*' } })).status === 412);
  d = await dav('PUT', `/dav/calendars/${ida.id}/default/other-${stamp}.ics`, { body: series });
  ok('the same UID under another name is a conflict', d.status === 403 && /no-uid-conflict/.test(d.text) && d.text.includes(encodeURIComponent(`my event ${stamp}.ics`).replace(/%20/g, '%20')), d.text);
  d = await dav('PUT', path, { body: series.replace('COUNT=5', 'COUNT=2').replace(`SUMMARY:App series ${stamp}`, `SUMMARY:App series v2 ${stamp}`), headers: { 'if-match': etag } });
  ok('PUT with the right ETag changes it', d.status === 204, d);
  const two = (await ida.get(`/calendar?${range(at(0), at(0, 0, 7))}`)).data.items.filter((x: any) => x.title === `App series v2 ${stamp}`);
  ok('…to two repeats (one removed)', two.length === 1, two);
  ok('an unsupported repeat rule is refused', (await dav('PUT', `/dav/calendars/${ida.id}/default/h-${stamp}.ics`, { body: series.replace(uid, `h-${uid}`).replace('FREQ=DAILY;COUNT=5', 'FREQ=HOURLY') })).status === 403);
  ok('to-dos aren\'t taken', (await dav('PUT', `/dav/calendars/${ida.id}/default/t-${stamp}.ics`, { body: `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:t-${stamp}\r\nSUMMARY:x\r\nEND:VTODO\r\nEND:VCALENDAR\r\n` })).status === 403);
  ok('nonsense is refused', (await dav('PUT', `/dav/calendars/${ida.id}/default/n-${stamp}.ics`, { body: 'hello' })).status === 403);

  // ---- a guest's app ----
  d = await dav('PROPFIND', `/dav/calendars/${ken.id}/default/`, { who: ken, body: propfind('<d:getetag/>'), depth: '1' });
  ok('Ken\'s app sees what he is invited to', d.text.includes(`${nerpEv.id}.ics`) && d.text.includes(`${appEv.id}.ics`), d.text);
  d = await dav('GET', `/dav/calendars/${ken.id}/default/${appEv.id}.ics`, { who: ken });
  const accepted = d.text.replace(`PARTSTAT=NEEDS-ACTION:mailto:${ken.email}`, `PARTSTAT=ACCEPTED:mailto:${ken.email}`).replace(/SUMMARY:App series v2/, 'SUMMARY:Hijacked');
  const kenEtag = d.headers.get('etag')!;
  d = await dav('PUT', `/dav/calendars/${ken.id}/default/${appEv.id}.ics`, { who: ken, body: unfold(accepted).includes(`PARTSTAT=ACCEPTED:mailto:${ken.email}`) ? accepted : unfold(accepted).replace(`PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:${ken.email}`, `PARTSTAT=ACCEPTED;RSVP=TRUE:mailto:${ken.email}`), headers: { 'if-match': kenEtag } });
  const afterKen = (await ida.get(`/calendar/events/${appEv.id}`)).data;
  ok('his answer counts, his other changes don\'t', d.status === 204 && afterKen.people.find((p: any) => p.id === ken.id)?.status === 'accepted' && afterKen.title === `App series v2 ${stamp}`, { status: d.status, afterKen });
  ok('Ida is told he accepted', JSON.stringify((await ida.get('/notifications')).data).includes(`accepted “App series v2 ${stamp}”`));

  // ---- queries ----
  const q = (from: Date, to: Date) => `<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="${ics(from)}" end="${ics(to)}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>`;
  d = await dav('REPORT', `/dav/calendars/${ida.id}/default/`, { body: q(at(9, 45), at(9, 50)), depth: '1' });
  ok('a time-range query finds what overlaps', d.text.includes(`${nerpEv.id}.ics`) && !d.text.includes('my%20event'), d.text);
  d = await dav('REPORT', `/dav/calendars/${ida.id}/default/`, { body: q(at(14, 0, 2), at(16, 0, 2)), depth: '1' });
  ok('…including a changed occurrence', d.text.includes('my%20event'), d.text);
  const todo = `<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"/></c:comp-filter></c:filter></c:calendar-query>`;
  ok('a to-do query finds nothing', !(await dav('REPORT', `/dav/calendars/${ida.id}/default/`, { body: todo, depth: '1' })).text.includes('.ics'));
  d = await dav('REPORT', `/dav/calendars/${ida.id}/default/`, { body: `<?xml version="1.0"?><c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><d:href>/dav/calendars/${ida.id}/default/${nerpEv.id}.ics</d:href><d:href>/dav/calendars/${ida.id}/default/missing.ics</d:href></c:calendar-multiget>`, depth: '1' });
  ok('multiget returns data and a 404 for a missing one', d.text.includes('BEGIN:VCALENDAR') && d.text.includes(`Renamed in NERP ${stamp}`) && /missing\.ics<\/d:href><d:status>HTTP\/1.1 404/.test(d.text), d.text);

  // ---- sync ----
  const sync = (token: string) => dav('REPORT', `/dav/calendars/${ida.id}/default/`, { body: `<?xml version="1.0"?><d:sync-collection xmlns:d="DAV:"><d:sync-token>${token}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>`, depth: '1' });
  d = await sync('');
  const t1 = d.text.match(/<d:sync-token>([^<]+)<\/d:sync-token>/)?.[1]!;
  ok('a first sync lists everything with a token', !!t1 && d.text.includes(`${nerpEv.id}.ics`) && d.text.includes('my%20event'), d.text);
  ok('nothing changed: nothing listed', !(await sync(t1)).text.includes('.ics'));
  await ida.patch(`/calendar/events/${nerpEv.id}`, { location: 'Room 9' });
  d = await sync(t1);
  const t2 = d.text.match(/<d:sync-token>([^<]+)<\/d:sync-token>/)?.[1]!;
  ok('a change in NERP is the only thing listed', d.text.includes(`${nerpEv.id}.ics`) && !d.text.includes('my%20event') && t2 !== t1, d.text);
  await ida.patch(`/calendar/events/${nerpEv.id}`, { attendees: [] });
  ok('a guest taken off loses it from his sync', (await dav('REPORT', `/dav/calendars/${ken.id}/default/`, { who: ken, body: `<?xml version="1.0"?><d:sync-collection xmlns:d="DAV:"><d:sync-token>${t1}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>`, depth: '1' })).text.match(new RegExp(`${nerpEv.id}\\.ics</d:href><d:status>HTTP/1.1 404`)));
  await ida.del(`/calendar/events/${nerpEv.id}`);
  d = await sync(t2);
  ok('a deletion is reported as 404', new RegExp(`${nerpEv.id}\\.ics</d:href><d:status>HTTP/1.1 404`).test(d.text), d.text);
  ok('a token NERP never gave is refused', /valid-sync-token/.test((await sync('urn:other:1')).text));

  // ---- deleting from apps ----
  d = await dav('DELETE', `/dav/calendars/${ken.id}/default/${appEv.id}.ics`, { who: ken });
  ok('a guest deleting it declines', d.status === 204 && (await ida.get(`/calendar/events/${appEv.id}`)).data.people.find((p: any) => p.id === ken.id)?.status === 'declined');
  ok('a delete with a stale ETag is refused', (await dav('DELETE', path, { headers: { 'if-match': '"1"' } })).status === 412);
  d = await dav('DELETE', path);
  ok('the organiser deleting it removes it', d.status === 204 && (await ida.get(`/calendar/events/${appEv.id}`)).status === 404, d);
  ok('Ken is told', JSON.stringify((await ken.get('/notifications')).data).includes(`cancelled “App series v2 ${stamp}”`));

  // ---- a real client ----
  const client = await createDAVClient({ serverUrl: BASE, credentials: { username: ida.email, password: ida.pw }, authMethod: 'Basic', defaultAccountType: 'caldav' });
  const cals = await client.fetchCalendars();
  ok('tsdav finds the calendar by itself', cals.length === 1 && cals[0].url.endsWith(`/dav/calendars/${ida.id}/default/`), cals.map((c) => c.url));
  const cal = cals[0];
  const tsUid = `ts-${stamp}`;
  const made = await client.createCalendarObject({
    calendar: cal, filename: `${tsUid}.ics`,
    iCalString: `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//tsdav//EN\r\nBEGIN:VEVENT\r\nUID:${tsUid}\r\nDTSTAMP:${ics(new Date())}\r\nDTSTART:${ics(at(13, 0, 3))}\r\nDTEND:${ics(at(14, 0, 3))}\r\nSUMMARY:From tsdav ${stamp}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`
  });
  ok('it creates an event', made.status === 201, made.status);
  let found = (await ida.get(`/calendar?${range(at(0, 0, 3), at(23, 0, 3))}`)).data.items.find((x: any) => x.title === `From tsdav ${stamp}`);
  ok('NERP has it', !!found);
  let objs = await client.fetchCalendarObjects({ calendar: cal });
  const mine = objs.find((o) => o.url.endsWith(`${tsUid}.ics`))!;
  ok('it reads events back', !!mine && mine.data.includes(`From tsdav ${stamp}`) && !!mine.etag, objs.map((o) => o.url));
  mine.data = mine.data.replace(`From tsdav ${stamp}`, `Changed in tsdav ${stamp}`);
  const upd = await client.updateCalendarObject({ calendarObject: mine });
  ok('it changes it', upd.status === 204 && (await ida.get(`/calendar/events/${found.id}`)).data.title === `Changed in tsdav ${stamp}`, upd.status);
  await ida.patch(`/calendar/events/${found.id}`, { title: `Changed in NERP ${stamp}` });
  const synced = await client.syncCalendars({ oldCalendars: [{ ...cal, objects: objs }], detailedResult: true });
  const upd2 = synced.updated.find((c: any) => c.url === cal.url) as any;
  ok('a sync picks up the change made in NERP', !!upd2 && (upd2.objects ?? []).some((o: any) => (o.data ?? '').includes(`Changed in NERP ${stamp}`)), JSON.stringify(synced).slice(0, 600));
  objs = await client.fetchCalendarObjects({ calendar: cal });
  const del = await client.deleteCalendarObject({ calendarObject: objs.find((o) => o.url.endsWith(`${tsUid}.ics`))! });
  ok('and deletes it', del.status === 204 && (await ida.get(`/calendar/events/${found.id}`)).status === 404, del.status);

  // ---- revoking ----
  const pwId = (await ida.get('/auth/app-passwords')).data.items[0].id;
  await ida.del(`/auth/app-passwords/${pwId}`);
  ok('a revoked app password stops working', (await dav('PROPFIND', '/dav/', { depth: '0' })).status === 401);
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
