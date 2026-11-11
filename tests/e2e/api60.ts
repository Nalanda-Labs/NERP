// Two-way sync with outside calendars:
// - a CalDAV account (here NERP's own CalDAV server, as another person: a real CalDAV peer):
//   finding the calendar, NERP events sent there, events made there brought in, changes and
//   deletions both ways, sync-only-in, disconnecting;
// - Google (mock sign-in on :8099, and a mock of Google's CalDAV that forwards to NERP's own
//   CalDAV server with a bearer token);
// - Microsoft 365 (mock sign-in and a mock Graph calendar), incl. a series with a changed
//   occurrence.
// The Google and Microsoft parts need the server started with the mock OAuth settings (see
// README); without them they are skipped.
import http from 'node:http';
import { createHash } from 'node:crypto';
import { Client, sleep } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const BASE = 'http://localhost:8080';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Sync${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  c.email = email;
  return c;
};
const [ida, ken, gus] = await Promise.all(['Ida', 'Ken', 'Gus'].map(user));
const monday = (() => { const d = new Date(); d.setUTCHours(0, 0, 0, 0); d.setUTCDate(d.getUTCDate() + ((8 - d.getUTCDay()) % 7 || 7)); return d; })();
const at = (h: number, m = 0, days = 0) => new Date(+monday + days * 86400e3 + (h * 60 + m) * 60e3);
const range = (days = 14) => `from=${encodeURIComponent(at(0).toISOString())}&to=${encodeURIComponent(at(0, 0, days).toISOString())}`;
const titles = async (c: any) => (await c.get(`/calendar?${range()}`)).data.items.filter((x: any) => x.kind === 'event').map((x: any) => x.title);
const ics = (t: Date) => t.toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '');
const vevent = (uid: string, title: string, s: Date, e: Date, extra = '') => `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\nUID:${uid}\r\nDTSTAMP:${ics(new Date())}\r\nDTSTART:${ics(s)}\r\nDTEND:${ics(e)}\r\nSUMMARY:${title}\r\n${extra}END:VEVENT\r\nEND:VCALENDAR\r\n`;
const basic = (c: any) => 'Basic ' + Buffer.from(`${c.email}:${c.pw}`).toString('base64');

// ---- mocks: OAuth for both, Google's CalDAV (forwarding to Gus's NERP calendar), Graph ----
const b64u = (b: string) => Buffer.from(b).toString('base64url');
const codes = new Map<string, any>();
const gEmail = `gus.${stamp}@gmail.example`, mEmail = `mo.${stamp}@contoso.example`;
type GEvent = any;
const graph = new Map<string, GEvent>();
let gseq = 0;
const ck = () => `ck${++gseq}`;
let graphCalls: string[] = [];
const mock = http.createServer(async (req, res) => {
  const u = new URL(req.url!, 'http://127.0.0.1:8099');
  let body = '';
  for await (const c of req) body += c;
  const json = (s: number, o: unknown) => { res.writeHead(s, { 'content-type': 'application/json' }); res.end(JSON.stringify(o)); };
  if (u.pathname.endsWith('/authorize')) {
    const code = 'code-' + Math.random().toString(36).slice(2);
    codes.set(code, { challenge: u.searchParams.get('code_challenge'), provider: u.pathname.split('/')[1], scope: u.searchParams.get('scope') });
    res.writeHead(302, { location: `${u.searchParams.get('redirect_uri')}?code=${code}&state=${u.searchParams.get('state')}` });
    return res.end();
  }
  if (u.pathname.endsWith('/token')) {
    const f = new URLSearchParams(body);
    if (f.get('grant_type') === 'refresh_token') return json(200, { access_token: `tok-${u.pathname.split('/')[1]}`, expires_in: 3600 });
    const c = codes.get(f.get('code')!);
    if (!c || createHash('sha256').update(f.get('code_verifier')!).digest('base64url') !== c.challenge) return json(400, { error: 'invalid_grant' });
    const claims = c.provider === 'google' ? { email: gEmail } : { preferred_username: mEmail };
    return json(200, { access_token: `tok-${c.provider}`, refresh_token: 'rt-1', expires_in: 3600, id_token: `${b64u('{}')}.${b64u(JSON.stringify(claims))}.s`, scope: c.scope });
  }
  // Google's CalDAV: bearer token in, Gus's NERP calendar behind.
  const gPrefix = `/google/caldav/v2/${gEmail}/events/`;
  if (u.pathname.startsWith('/google/caldav/')) {
    if (req.headers.authorization !== 'Bearer tok-google') return json(401, {});
    if (!u.pathname.startsWith(gPrefix)) { res.writeHead(404); return res.end(); }
    const nerp = `/dav/calendars/${gus.id}/default/`;
    const headers: Record<string, string> = { authorization: basic(gus) };
    for (const h of ['depth', 'if-match', 'if-none-match', 'content-type']) if (req.headers[h]) headers[h] = String(req.headers[h]);
    const r = await fetch(BASE + nerp + u.pathname.slice(gPrefix.length), { method: req.method, headers, body: body ? body.split(gPrefix).join(nerp) : undefined });
    const out: Record<string, string> = {};
    for (const h of ['etag', 'content-type']) { const v = r.headers.get(h); if (v) out[h] = v; }
    res.writeHead(r.status, out);
    return res.end((await r.text()).split(nerp).join(gPrefix));
  }
  // Microsoft Graph.
  const g = '/microsoft/graph/v1.0';
  if (u.pathname.startsWith(g)) {
    if (req.headers.authorization !== 'Bearer tok-microsoft') return json(401, { error: { message: 'no' } });
    const p = u.pathname.slice(g.length);
    graphCalls.push(`${req.method} ${p}`);
    if (p === '/me/calendar') return json(200, { id: 'cal-1', name: 'Calendar' });
    if (p === '/me/calendars/cal-1/events' && req.method === 'GET') return json(200, { value: [...graph.values()].filter((e) => e.type !== 'exception').map((e) => ({ id: e.id, changeKey: e.changeKey, type: e.type })) });
    if (p === '/me/calendars/cal-1/events' && req.method === 'POST') {
      const e = { ...JSON.parse(body), id: `ms-${++gseq}`, changeKey: ck(), type: JSON.parse(body).recurrence ? 'seriesMaster' : 'singleInstance', iCalUId: `ical-${gseq}` };
      graph.set(e.id, e);
      return json(201, e);
    }
    const m = p.match(/^\/me\/events\/([^/]+)(\/instances)?$/);
    if (m) {
      const id = decodeURIComponent(m[1]);
      const e = graph.get(id);
      if (!e) return json(404, { error: { message: 'gone' } });
      if (m[2]) return json(200, { value: [...graph.values()].filter((x) => x.seriesMasterId === id) });
      if (req.method === 'GET') return json(200, e);
      if (req.method === 'DELETE') { graph.delete(id); return res.writeHead(204).end(); }
      if (req.method === 'PATCH') {
        if (req.headers['if-match'] && req.headers['if-match'] !== `W/"${e.changeKey}"`) return json(412, { error: { message: 'changed' } });
        Object.assign(e, JSON.parse(body), { changeKey: ck() });
        return json(200, e);
      }
    }
    return json(404, { error: { message: `no ${p}` } });
  }
  res.writeHead(404); res.end();
});
await new Promise<void>((r) => mock.listen(8099, '127.0.0.1', () => r()));

async function connect(c: any, kind: string) {
  const s = await c.post(`/calendar/accounts/oauth/${kind}`);
  if (s.status !== 200) return { start: s };
  const a = await fetch(s.data.url, { redirect: 'manual' });
  const cb = new URL(a.headers.get('location')!);
  const back = await fetch(`${BASE}${cb.pathname}${cb.search}`, { redirect: 'manual' });
  return { start: s, back: new URL(back.headers.get('location')!) };
}
const waitFor = async (fn: () => Promise<unknown>, ms = 15000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(300); } return v; };

try {
  // ================= a CalDAV account =================
  ken.pw = (await ken.post('/auth/app-passwords', { name: `Sync ${stamp}` })).data.password;
  ok('a wrong password is reported', (await ida.post('/calendar/accounts/caldav/discover', { url: `${BASE}/dav/`, username: ken.email, password: 'zzzz-zzzz-zzzz-zzzz' })).status === 400);
  let r = await ida.post('/calendar/accounts/caldav/discover', { url: `${BASE}/`, username: ken.email, password: ken.pw });
  ok('the calendar is found from the server address', r.status === 200 && r.data.items.length === 1 && r.data.items[0].url.endsWith(`/dav/calendars/${ken.id}/default/`), r.data);
  const before = (await ida.post('/calendar/events', { title: `Ida plan ${stamp}`, start: at(9).toISOString(), end: at(10).toISOString(), timezone: 'UTC', attendees: [gus.id] })).data;
  await ken.post('/calendar/events', { title: `Ken talk ${stamp}`, start: at(11, 0, 1).toISOString(), end: at(12, 0, 1).toISOString(), timezone: 'UTC' });
  r = await ida.post('/calendar/accounts/caldav', { url: `${BASE}/dav/`, username: ken.email, password: ken.pw, name: 'My other calendar' });
  const acc = r.data;
  ok('connected and synced at once', r.status === 200 && acc.kind === 'caldav' && !acc.last_error && !!acc.last_sync_at && acc.events === 2 && acc.from_there === 1, r.data);
  ok('their event came in', (await titles(ida)).includes(`Ken talk ${stamp}`));
  ok('NERP\'s event went there', (await titles(ken)).includes(`Ida plan ${stamp}`));
  const sentCopy = (await ken.get(`/calendar?${range()}`)).data.items.find((x: any) => x.title === `Ida plan ${stamp}`);
  const kenCopy = (await ken.get(`/calendar/events/${sentCopy.id}`)).data;
  ok('…without its guests (so nobody is invited from there)', kenCopy.people.length === 1 && kenCopy.email_guests.length === 0, kenCopy);
  // Changes both ways.
  await ida.patch(`/calendar/events/${before.id}`, { title: `Ida plan v2 ${stamp}` });
  await ken.patch(`/calendar/events/${(await ken.get(`/calendar?${range()}`)).data.items.find((x: any) => x.title === `Ken talk ${stamp}`).id}`, { title: `Ken talk v2 ${stamp}`, start: at(13, 0, 1).toISOString(), end: at(14, 0, 1).toISOString() });
  r = await ida.post(`/calendar/accounts/${acc.id}/sync`);
  ok('a sync runs cleanly', !r.data.last_error, r.data);
  let mine = await titles(ida);
  ok('a change there comes in', mine.includes(`Ken talk v2 ${stamp}`) && !mine.includes(`Ken talk ${stamp}`), mine);
  ok('a change here goes there', (await titles(ken)).includes(`Ida plan v2 ${stamp}`));
  ok('the guests stay as they are in NERP', (await ida.get(`/calendar/events/${before.id}`)).data.people.some((p: any) => p.id === gus.id));
  const imported = (await ida.get(`/calendar?${range()}`)).data.items.find((x: any) => x.title === `Ken talk v2 ${stamp}`);
  ok('…at the new time', new Date(imported.start).getUTCHours() === 13, imported);
  // A repeating event made there.
  const series = vevent(`series-${stamp}`, `Ken weekly ${stamp}`, at(8, 0, 2), at(8, 30, 2), 'RRULE:FREQ=WEEKLY;COUNT=2\r\n');
  await fetch(`${BASE}/dav/calendars/${ken.id}/default/series-${stamp}.ics`, { method: 'PUT', headers: { authorization: basic(ken), 'content-type': 'text/calendar' }, body: series });
  await ida.post(`/calendar/accounts/${acc.id}/sync`);
  ok('a repeating event from there repeats here', (await titles(ida)).filter((t: string) => t === `Ken weekly ${stamp}`).length === 2);
  // Nothing changed: nothing written.
  const quiet = await ida.post(`/calendar/accounts/${acc.id}/sync`);
  ok('a second sync with nothing new is quiet', !quiet.data.last_error && quiet.data.events === 3, quiet.data);
  // Deletions both ways.
  await ida.del(`/calendar/events/${imported.id}`);
  const kensv2 = (await ken.get(`/calendar?${range()}`)).data.items.find((x: any) => x.title === `Ida plan v2 ${stamp}`);
  await ken.del(`/calendar/events/${kensv2.id}`);
  await ida.post(`/calendar/accounts/${acc.id}/sync`);
  ok('deleted here: deleted there', !(await titles(ken)).includes(`Ken talk v2 ${stamp}`));
  ok('deleted there: deleted here', (await ida.get(`/calendar/events/${before.id}`)).status === 404);
  // New events here go there on the next sync.
  await ida.post('/calendar/events', { title: `Ida later ${stamp}`, start: at(15, 0, 3).toISOString(), end: at(16, 0, 3).toISOString(), timezone: 'UTC' });
  await ida.post(`/calendar/accounts/${acc.id}/sync`);
  ok('a new event here goes there', (await titles(ken)).includes(`Ida later ${stamp}`));
  // Bring-in only.
  r = await ida.patch(`/calendar/accounts/${acc.id}`, { push: false });
  ok('switched to bringing events in only', r.data.push === false);
  await ida.post('/calendar/events', { title: `Ida private ${stamp}`, start: at(15, 0, 4).toISOString(), end: at(16, 0, 4).toISOString(), timezone: 'UTC' });
  await ida.post(`/calendar/accounts/${acc.id}/sync`);
  ok('…so new NERP events stay here', !(await titles(ken)).includes(`Ida private ${stamp}`));
  ok('the account isn\'t anyone else\'s to see', (await ken.get('/calendar/accounts')).data.items.length === 0 && (await ken.post(`/calendar/accounts/${acc.id}/sync`)).status === 404);
  r = await ida.del(`/calendar/accounts/${acc.id}`);
  mine = await titles(ida);
  ok('disconnecting removes what came from there, keeps NERP\'s own', r.status === 200 && !mine.includes(`Ken weekly ${stamp}`) && mine.includes(`Ida later ${stamp}`), mine);
  ok('…and deletes nothing there', (await titles(ken)).includes(`Ida later ${stamp}`));

  // ================= Google and Microsoft =================
  const prov = (await ida.get('/calendar/accounts/providers')).data;
  if (!prov.google || !prov.microsoft) {
    console.log('skip Google and Microsoft: the server has no mock OAuth settings (see README)');
  } else {
    gus.pw = (await gus.post('/auth/app-passwords', { name: `Google ${stamp}` })).data.password;
    await gus.post('/calendar/events', { title: `Gus at Google ${stamp}`, start: at(10, 0, 5).toISOString(), end: at(11, 0, 5).toISOString(), timezone: 'UTC' });
    let c = await connect(ken, 'google');
    ok('Google sign-in comes back to the calendar', c.back?.pathname === '/calendar' && c.back.searchParams.get('connected') === gEmail, c.back?.toString() ?? c.start);
    let g: any;
    ok('the Google calendar syncs', !!(g = await waitFor(async () => (await ken.get('/calendar/accounts')).data.items.find((a: any) => a.kind === 'google' && a.last_sync_at))) && !g.last_error, g);
    ok('its events came in', (await titles(ken)).includes(`Gus at Google ${stamp}`));
    ok('Ken\'s events went to Google', (await titles(gus)).includes(`Ida later ${stamp}`));

    const series = { subject: `Mo standup ${stamp}`, start: { dateTime: at(9, 0, 6).toISOString().slice(0, 19), timeZone: 'UTC' }, end: { dateTime: at(9, 15, 6).toISOString().slice(0, 19), timeZone: 'UTC' }, isAllDay: false,
      recurrence: { pattern: { type: 'daily', interval: 1 }, range: { type: 'numbered', startDate: at(0, 0, 6).toISOString().slice(0, 10), numberOfOccurrences: 3 } } };
    graph.set('ms-a', { id: 'ms-a', changeKey: ck(), type: 'singleInstance', iCalUId: 'ical-a', subject: `Mo lunch ${stamp}`, start: { dateTime: at(12, 0, 6).toISOString().slice(0, 19), timeZone: 'UTC' }, end: { dateTime: at(13, 0, 6).toISOString().slice(0, 19), timeZone: 'UTC' }, isAllDay: false });
    graph.set('ms-s', { id: 'ms-s', changeKey: ck(), type: 'seriesMaster', iCalUId: 'ical-s', ...series });
    graph.set('ms-x', { id: 'ms-x', changeKey: ck(), type: 'exception', seriesMasterId: 'ms-s', originalStart: at(9, 0, 7).toISOString(), subject: `Mo standup late ${stamp}`, start: { dateTime: at(11, 0, 7).toISOString().slice(0, 19), timeZone: 'UTC' }, end: { dateTime: at(11, 15, 7).toISOString().slice(0, 19), timeZone: 'UTC' }, isAllDay: false });
    c = await connect(gus, 'microsoft');
    ok('Microsoft sign-in comes back to the calendar', c.back?.searchParams.get('connected') === mEmail, c.back?.toString() ?? c.start);
    let m: any;
    ok('the Microsoft calendar syncs', !!(m = await waitFor(async () => (await gus.get('/calendar/accounts')).data.items.find((a: any) => a.kind === 'microsoft' && a.last_sync_at))) && !m.last_error && m.calendar_name === 'Calendar', m);
    const gt = await titles(gus);
    ok('its events came in, the series with its changed occurrence', gt.includes(`Mo lunch ${stamp}`) && gt.filter((t: string) => t === `Mo standup ${stamp}`).length === 2 && gt.includes(`Mo standup late ${stamp}`), gt);
    const pushed = [...graph.values()].find((e) => e.subject === `Gus at Google ${stamp}`);
    ok('Gus\'s own events went to Microsoft, as Graph events', !!pushed && pushed.start.timeZone === 'UTC' && !pushed.attendees, [...graph.values()].map((e) => e.subject));
    // Changes both ways.
    graph.get('ms-a').subject = `Mo lunch moved ${stamp}`;
    graph.get('ms-a').changeKey = ck();
    const mine2 = (await gus.get(`/calendar?${range()}`)).data.items.find((x: any) => x.title === `Gus at Google ${stamp}`);
    await gus.patch(`/calendar/events/${mine2.id}`, { title: `Gus renamed ${stamp}` });
    graphCalls = [];
    r = await gus.post(`/calendar/accounts/${m.id}/sync`);
    ok('a sync with Microsoft runs cleanly', !r.data.last_error, r.data);
    ok('a change at Microsoft comes in', (await titles(gus)).includes(`Mo lunch moved ${stamp}`));
    ok('a change here is sent with PATCH and the version', pushed.subject === `Gus renamed ${stamp}` && graphCalls.some((x) => x.startsWith('PATCH')), graphCalls);
    graph.delete('ms-a');
    await gus.post(`/calendar/accounts/${m.id}/sync`);
    ok('deleted at Microsoft: deleted here', !(await titles(gus)).includes(`Mo lunch moved ${stamp}`));
    await gus.del(`/calendar/events/${mine2.id}`);
    await gus.post(`/calendar/accounts/${m.id}/sync`);
    ok('deleted here: deleted at Microsoft', ![...graph.values()].some((e) => e.subject === `Gus renamed ${stamp}`));
  }
} finally {
  mock.close();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
