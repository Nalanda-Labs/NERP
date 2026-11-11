// Tokens other than the sign-in token: Collabora's file tokens (WOPI), calendar-app passwords
// (CalDAV), real-time tickets, short download links, API keys, and the second sign-in step
// (two-factor codes). Each must work only for what it was made for, for as long as its
// owner may still use it.
import { createHmac } from 'node:crypto';
import { admin, person, raw, ok, finish, resetLimits, stamp, BASE, API } from './lib.ts';
import { Client } from '../e2e/lib.ts';

const ann = await person('Ann');
const bob = await person('Bob');
const upload = async (c: Client, name: string, text: string) => { const f = new FormData(); f.append('file', new Blob([text]), name); return (await c.post('/files', f)).data.id; };
const toDrive = async (c: Client, name: string, text: string) => (await c.post('/drive/files', { file_id: await upload(c, name, text), name })).data;

// ---------------------------------------------------------------------------
// WOPI (Collabora): a token is for one person, one file, while they may use it
// ---------------------------------------------------------------------------
const office = (await ann.get('/office/status')).data;
if (!office.editing) console.log('skip WOPI: the server has no COLLABORA_URL');
else {
  const doc = await toDrive(ann, `wopi-${stamp}.rtf`, '{\\rtf1 Ann only}');
  const other = await toDrive(ann, `wopi-other-${stamp}.rtf`, '{\\rtf1 Ann other}');
  const wopi = (id: string, token: string, init: { method?: string; headers?: Record<string, string>; body?: string } = {}) =>
    raw(init.method ?? 'GET', `/wopi/files/${id}${init.method === 'POST' && init.headers?.['X-WOPI-Override'] === 'PUT' ? '/contents' : ''}?access_token=${encodeURIComponent(token)}`, init);
  const mine = (await ann.post(`/drive/nodes/${doc.id}/office`, {})).data.token as string;
  ok('the editor token reads its own file', (await wopi(doc.id, mine)).status === 200);
  ok('…but not another file', (await wopi(other.id, mine)).status === 401 && (await raw('GET', `/wopi/files/${other.id}/contents?access_token=${mine}`)).status === 401);
  const [n, u, sid, exp, sig] = mine.split('.');
  ok('changing whose token it is breaks it', (await wopi(doc.id, `${n}.${bob.id}.${sid}.${exp}.${sig}`)).status === 401);
  ok('stretching its expiry breaks it', (await wopi(doc.id, `${n}.${u}.${sid}.${Number(exp) + 86400 * 365}.${sig}`)).status === 401);
  ok('a token for another file, re-pointed, breaks', (await wopi(other.id, `${other.id}.${u}.${sid}.${exp}.${sig}`)).status === 401);
  ok('it is not a sign-in token', (await raw('GET', '/auth/me', { headers: { authorization: `Bearer ${mine}` } })).status === 401);

  // Bob may edit, then loses access.
  await ann.put(`/drive/nodes/${doc.id}/shares`, [{ user_id: bob.id, level: 'edit' }]);
  const bobs = (await bob.post(`/drive/nodes/${doc.id}/office`, {})).data.token as string;
  ok('a colleague with edit access gets a working token', (await wopi(doc.id, bobs)).json?.UserCanWrite === true);
  await ann.put(`/drive/nodes/${doc.id}/shares`, [{ user_id: bob.id, level: 'view' }]);
  const info = (await wopi(doc.id, bobs)).json;
  ok('when Ann makes it view-only, his token can no longer write', info?.UserCanWrite === false, info);
  const put = await wopi(doc.id, bobs, { method: 'POST', headers: { 'X-WOPI-Override': 'PUT' }, body: '{\\rtf1 Bob was here}' });
  ok('…and his saves are refused', put.status === 403 || put.status === 401, put.status);
  await ann.put(`/drive/nodes/${doc.id}/shares`, []);
  ok('when she stops sharing, his token reads nothing', (await wopi(doc.id, bobs)).status === 401 && (await raw('GET', `/wopi/files/${doc.id}/contents?access_token=${bobs}`)).status === 401);

  // The token dies with the person's sign-in.
  const cy = await person('Cy');
  await ann.put(`/drive/nodes/${doc.id}/shares`, [{ user_id: cy.id, level: 'edit' }]);
  const cys = (await cy.post(`/drive/nodes/${doc.id}/office`, {})).data.token as string;
  // The password is changed on another device: every other sign-in ends, with its tokens.
  const cyPhone = new Client();
  await cyPhone.post('/auth/login', { email: cy.email, password: 'password123' });
  ok('the editor token works before', (await wopi(doc.id, cys)).status === 200);
  await cyPhone.post('/auth/change-password', { current_password: 'password123', new_password: 'password456' });
  ok('after a password change elsewhere, the editor token stops working', (await wopi(doc.id, cys)).status === 401);
  const dee = await person('Dee');
  await ann.put(`/drive/nodes/${doc.id}/shares`, [{ user_id: dee.id, level: 'edit' }]);
  const dees = (await dee.post(`/drive/nodes/${doc.id}/office`, {})).data.token as string;
  await dee.post('/auth/logout');
  ok('after signing out, the editor token stops working', (await wopi(doc.id, dees)).status === 401);
  const eve = await person('Eve');
  await ann.put(`/drive/nodes/${doc.id}/shares`, [{ user_id: eve.id, level: 'edit' }]);
  const eves = (await eve.post(`/drive/nodes/${doc.id}/office`, {})).data.token as string;
  await admin.patch(`/users/${eve.id}`, { is_active: false });
  ok('a deactivated person\'s editor token stops working', (await wopi(doc.id, eves)).status === 401);
  ok('Ann\'s file was never overwritten', (await (await ann.req('GET', `/drive/nodes/${doc.id}/download`, undefined, true) as Response).text()) === '{\\rtf1 Ann only}');
}

// ---------------------------------------------------------------------------
// CalDAV: an app password opens one's own calendar only
// ---------------------------------------------------------------------------
const DAV = BASE.replace(/\/api$/, '').replace(/\/$/, '');
const dav = (method: string, path: string, email: string, pw: string, body?: string, extra: Record<string, string> = {}) =>
  fetch(`${DAV}${path}`, { method, headers: { authorization: 'Basic ' + Buffer.from(`${email}:${pw}`).toString('base64'), 'content-type': 'application/xml', depth: '1', ...extra }, body });
const start = new Date(Date.now() + 2 * 86400e3);
const secret = (await ann.post('/calendar/events', { title: `Ann secret ${stamp}`, timezone: 'UTC', start: start.toISOString(), end: new Date(+start + 3600e3).toISOString(), is_private: true })).data;
const bobPw = (await bob.post('/auth/app-passwords', { name: `Bob cal ${stamp}` })).data;
const PROPFIND = '<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:getetag/></d:prop></d:propfind>';
let res = await dav('PROPFIND', `/dav/calendars/${bob.id}/default/`, bob.email, bobPw.password, PROPFIND);
ok('Bob\'s app password opens his calendar', res.status === 207, res.status);
const annCal = await dav('PROPFIND', `/dav/calendars/${ann.id}/default/`, bob.email, bobPw.password, PROPFIND);
ok('…but not Ann\'s', [403, 404].includes(annCal.status) && !(await annCal.text()).includes(secret.id), annCal.status);
res = await dav('GET', `/dav/calendars/${bob.id}/default/${secret.id}.ics`, bob.email, bobPw.password);
ok('Ann\'s event can\'t be fetched through his own calendar', res.status === 404 && !(await res.text()).includes('Ann secret'));
res = await dav('GET', `/dav/calendars/${ann.id}/default/${secret.id}.ics`, bob.email, bobPw.password);
ok('…or through hers', [403, 404].includes(res.status));
res = await dav('PUT', `/dav/calendars/${ann.id}/default/planted-${stamp}.ics`, bob.email, bobPw.password,
  `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:planted-${stamp}\r\nDTSTART:20270101T100000Z\r\nDTEND:20270101T110000Z\r\nSUMMARY:Planted ${stamp}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`, { 'content-type': 'text/calendar' });
ok('Bob can\'t put an event into Ann\'s calendar', [403, 404].includes(res.status) && !JSON.stringify((await ann.get(`/calendar/events?from=2026-12-31T00:00:00Z&to=2027-01-02T00:00:00Z`)).data).includes(`Planted ${stamp}`), res.status);
res = await dav('DELETE', `/dav/calendars/${bob.id}/default/${secret.id}.ics`, bob.email, bobPw.password);
ok('…or delete hers', [403, 404].includes(res.status) && (await ann.get(`/calendar/events/${secret.id}`)).status === 200);
ok('the account password is not a calendar password', (await dav('PROPFIND', `/dav/calendars/${bob.id}/default/`, bob.email, 'password123', PROPFIND)).status === 401);
ok('an app password is not a sign-in password', (await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: bob.email, password: bobPw.password }) })).status === 401);
ok('…nor an API token', (await raw('GET', '/auth/me', { headers: { authorization: `Bearer ${bobPw.password}` } })).status === 401);
ok('Ann\'s email with Bob\'s app password fails', (await dav('PROPFIND', `/dav/calendars/${ann.id}/default/`, ann.email, bobPw.password, PROPFIND)).status === 401);
await bob.del(`/auth/app-passwords/${bobPw.id}`);
ok('a revoked app password stops at once', (await dav('PROPFIND', `/dav/calendars/${bob.id}/default/`, bob.email, bobPw.password, PROPFIND)).status === 401);
let slowed = false;
for (let i = 0; i < 40 && !slowed; i++) if ((await dav('PROPFIND', '/dav/', ann.email, `guess-${i}`, PROPFIND)).status === 429) slowed = true;
ok('guessing app passwords is slowed', slowed);
resetLimits('dav:fail:*');

// ---------------------------------------------------------------------------
// Real-time: a ticket is used once and carries only its owner's events
// ---------------------------------------------------------------------------
const WS = API.replace(/^http/, 'ws');
const connect = (ticket: string) => new Promise<{ ws: WebSocket | null; events: any[] }>((resolve) => {
  const ws = new WebSocket(`${WS}/realtime/ws?ticket=${encodeURIComponent(ticket)}`);
  const events: any[] = [];
  ws.onmessage = (m) => events.push(JSON.parse(String(m.data)));
  ws.onopen = () => setTimeout(() => resolve({ ws, events }), 300);
  ws.onerror = () => resolve({ ws: null, events });
});
const t1 = (await bob.post('/realtime/ticket')).data.ticket;
const first = await connect(t1);
ok('a ticket opens the live connection', !!first.ws);
const again = await connect(t1);
ok('the same ticket can\'t be used twice', !again.ws || again.events.every((e) => e.type !== 'hello'));
ok('a made-up ticket opens nothing', !(await connect('made-up-ticket-0000000000')).ws);
const annChat = (await ann.post('/im/chats', { title: `Ann and Cy ${stamp}`, members: [admin.id ?? (await admin.get('/auth/me')).data.id] })).data;
await ann.post(`/im/chats/${annChat.id}/messages`, { body: `whisper ${stamp}` });
await new Promise((r) => setTimeout(r, 800));
ok('Bob\'s connection carries none of Ann\'s private messages', !JSON.stringify(first.events).includes(`whisper ${stamp}`), first.events.slice(0, 5));
first.ws?.close();
again.ws?.close();

// ---------------------------------------------------------------------------
// Short download links: one file, one person, not a sign-in
// ---------------------------------------------------------------------------
const annFile = await upload(ann, `ann-${stamp}.txt`, 'ann file');
const bobFile = await upload(bob, `bob-${stamp}.txt`, 'bob file');
let r = await bob.post('/download-links', { path: `/api/files/${annFile}/content` });
const linkForAnnFile = r.data?.url;
ok('a link to someone else\'s file is refused or opens nothing', !linkForAnnFile || (await raw('GET', linkForAnnFile.replace(/^\/api/, ''))).status >= 400);
r = await bob.post('/download-links', { path: `/api/files/${bobFile}/content` });
const dl = new URL(r.data.url, 'http://x').searchParams.get('dl')!;
ok('his own link downloads his file', (await raw('GET', `/files/${bobFile}/content?dl=${dl}`)).text === 'bob file');
ok('…but the same token doesn\'t open Ann\'s', (await raw('GET', `/files/${annFile}/content?dl=${dl}`)).status >= 400);
ok('…nor other routes', (await raw('GET', `/auth/me?dl=${dl}`)).status === 401 && (await raw('GET', `/tasks?dl=${dl}`)).status === 401);
ok('links are only for downloads', (await bob.post('/download-links', { path: '/api/users' })).status === 400 && (await bob.post('/download-links', { path: `/api/files/${bobFile}/content/../../users` })).status === 400);

// ---------------------------------------------------------------------------
// API keys: only their scopes, never the account
// ---------------------------------------------------------------------------
const kim = await person('Kim', ['api.keys']);
const crmKey = (await kim.post('/api-keys', { name: `crm ${stamp}`, scopes: ['crm:read'] })).data;
const key = crmKey.secret ?? crmKey.key ?? crmKey.token;
const withKey = (method: string, path: string, body?: unknown) => raw(method, path, { headers: { authorization: `Bearer ${key}`, ...(body ? { 'content-type': 'application/json' } : {}) }, body: body ? JSON.stringify(body) : undefined });
ok('a CRM read key reads the CRM', (await withKey('GET', '/crm/deal/items')).status === 200, crmKey);
const outside: string[] = [];
for (const [m, p, b] of [['GET', '/users'], ['GET', '/tasks'], ['GET', '/settings'], ['GET', '/auth/me'], ['GET', '/api-keys'], ['POST', '/api-keys', { name: 'x', scopes: ['crm'] }],
  ['POST', '/auth/change-password', { current_password: 'password123', new_password: 'pwned12345' }], ['POST', '/crm/deal/items', { title: 'x' }], ['GET', '/im/chats'], ['GET', '/drive'],
  ['GET', '//users'], ['GET', '/crm/../users'], ['GET', '/%63rm/../users'], ['GET', '/CRM/deal/items'], ['POST', '/realtime/ticket']] as [string, string, unknown?][]) {
  const x = await withKey(m, p, b);
  if (x.status < 400) outside.push(`${m} ${p} → ${x.status}`);
}
ok('it reaches nothing outside its scope', outside.length === 0, outside);
await admin.patch(`/users/${kim.id}`, { is_active: false });
ok('a key stops when its owner is deactivated', (await withKey('GET', '/crm/deal/items')).status === 401);
await admin.patch(`/users/${kim.id}`, { is_active: true });

// ---------------------------------------------------------------------------
// Two-factor sign-in
// ---------------------------------------------------------------------------
const b32 = (s: string) => { const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'; let bits = ''; for (const c of s.replace(/=+$/, '').toUpperCase()) bits += A.indexOf(c).toString(2).padStart(5, '0'); return Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2))); };
const totp = (secret: string, at = Date.now()) => {
  const c = Buffer.alloc(8); c.writeBigUInt64BE(BigInt(Math.floor(at / 30000)));
  const h = createHmac('sha1', b32(secret)).update(c).digest(); const o = h[h.length - 1] & 15;
  return String(((h.readUInt32BE(o) & 0x7fffffff) % 1e6)).padStart(6, '0');
};
const tf = await person('Tia');
const setup = (await tf.post('/auth/2fa/setup', { password: 'password123' })).data;
const enabled = (await tf.post('/auth/2fa/enable', { code: totp(setup.secret) })).data;
ok('two-factor sign-in is on', Array.isArray(enabled.recovery_codes), enabled);
await new Promise((r) => setTimeout(r, 30_500 - (Date.now() % 30_000)));
const signIn = async () => (await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: tf.email, password: 'password123' }) })).json;
const step2 = (challenge: string, code: string) => raw('POST', '/auth/login/2fa', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ challenge, code }) });
let ch = await signIn();
ok('the password alone gives no token', ch.two_factor_required && !ch.access_token, ch);
const code = totp(setup.secret);
ok('the right code signs in', (await step2(ch.challenge, code)).status === 200);
ch = await signIn();
ok('the same code can\'t be used twice', (await step2(ch.challenge, code)).status === 401);
ch = await signIn();
const rc = enabled.recovery_codes[0];
ok('a recovery code signs in once', (await step2(ch.challenge, rc)).status === 200);
ch = await signIn();
ok('…and not twice', (await step2(ch.challenge, rc)).status === 401);
ok('a challenge is not a token', (await raw('GET', '/auth/me', { headers: { authorization: `Bearer ${ch.challenge}` } })).status === 401);
// Guessing codes: each sign-in allows a few tries, but starting over must not give endless
// tries. After many wrong codes over fresh sign-ins, the account must make the attacker wait.
let tries = 0;
for (let i = 0; i < 12; i++) {
  const c = await signIn();
  if (!c?.challenge) break;
  for (let j = 0; j < 5; j++) { tries++; await step2(c.challenge, String(100000 + i * 5 + j)); }
}
await new Promise((r) => setTimeout(r, 30_500 - (Date.now() % 30_000)));
const after = await signIn();
const goodNow = after?.challenge ? (await step2(after.challenge, totp(setup.secret))).status : 429;
ok(`${tries} wrong codes over fresh sign-ins lock the second step for a while`, goodNow !== 200, { tries, goodNow, after });
const rescue = await signIn();
ok('…while the person can still sign in with a recovery code', rescue?.challenge && (await step2(rescue.challenge, enabled.recovery_codes[1])).status === 200);
resetLimits('auth:fail*');
resetLimits('auth:2fa*');

await finish();
