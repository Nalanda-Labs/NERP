// The server calling out (SSRF) and other server-side abuse, against a second NERP started
// here with the production defaults (no private-network exceptions, Secure cookies) on
// 127.0.0.1:8091, sharing the database: BI datasets from URLs, calendar accounts and
// mailboxes pointed at internal addresses in every spelling. Also: a meeting room raced by
// calendar events, fields people may not set on themselves (mass assignment), and the
// company-wide menu default.
import { spawn } from 'node:child_process';
import { admin, person, raw, ok, finish, stamp } from './lib.ts';

const STRICT = 'http://127.0.0.1:8091';
const backend = new URL('../../backend/', import.meta.url).pathname;
const server = spawn('./target/debug/nerp', [], {
  cwd: backend,
  env: { ...process.env, BIND_ADDR: '127.0.0.1:8091', PUBLIC_URL: STRICT, CRM_WEBHOOK_ALLOW_PRIVATE: 'false', MAIL_ALLOW_PRIVATE: 'false', COOKIE_SECURE: 'true', TRUST_PROXY: 'false', WORKER_ENABLED: 'false', STATIC_DIR: '',
    // The stored files' key (as tests/e2e/tools/restart-server.sh sets it): without it the server rightly refuses to start.
    FILE_ENCRYPTION_KEY: process.env.FILE_ENCRYPTION_KEY ?? 'nerp-test-file-key-0123456789abcdef', LIVEKIT_URL: '', COLLABORA_URL: '' },
  stdio: 'ignore',
});
let up = false;
for (let i = 0; i < 90 && !up; i++) {
  up = await fetch(`${STRICT}/api/health`).then((r) => r.ok, () => false);
  if (!up) await new Promise((r) => setTimeout(r, 1000));
}
ok('a second server with the production defaults is up', up);
const strict = async (method: string, path: string, token: string, body?: unknown) => {
  const r = await fetch(`${STRICT}/api${path}`, { method, headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' }, body: body ? JSON.stringify(body) : undefined });
  return { status: r.status, text: await r.text() };
};

try {
  // ---- cookies ----
  const login = await fetch(`${STRICT}/api/auth/login`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: 'admin@acme.test', password: 'supersecret1' }) });
  const cookie = login.headers.get('set-cookie') ?? '';
  ok('in production the refresh cookie is Secure, HttpOnly and SameSite=Strict', /;\s*Secure/i.test(cookie) && /HttpOnly/i.test(cookie) && /SameSite=Strict/i.test(cookie), cookie);
  const token = (await login.json()).access_token as string;

  // ---- internal addresses in every spelling ----
  const INTERNAL = [
    'http://127.0.0.1:8080/api/health', 'http://localhost:8080/api/health', 'http://localhost.:8080/api/health', 'http://127.1:8080/api/health',
    'http://2130706433:8080/api/health', 'http://0x7f000001:8080/api/health', 'http://0177.0.0.1:8080/api/health', 'http://0.0.0.0:8080/api/health',
    'http://[::1]:8080/api/health', 'http://[::ffff:127.0.0.1]:8080/api/health', 'http://[0:0:0:0:0:ffff:7f00:1]:8080/api/health',
    'http://169.254.169.254/latest/meta-data/', 'http://10.0.0.1/', 'http://192.168.1.1/', 'http://172.16.0.1/', 'http://100.64.0.1/', 'http://[fd00::1]/',
    'http://127.0.0.1:6379/', 'http://127.0.0.1:5434/', 'file:///etc/passwd', 'gopher://127.0.0.1:6379/_INFO', 'ftp://127.0.0.1/',
  ];
  const reached: string[] = [];
  for (const url of INTERNAL) {
    const r = await strict('POST', '/bi/datasets', token, { name: `Pen ssrf ${stamp}`, kind: 'url', url });
    // Refused for the right reason: an internal address, or not a web address at all.
    if (r.status !== 400 || !/private network|internal network|must start with http|full URL/i.test(r.text)) reached.push(`BI ${url} → ${r.status} ${r.text.slice(0, 100)}`);
  }
  ok('BI datasets can\'t fetch internal addresses', reached.length === 0, reached);
  const cal: string[] = [];
  for (const url of ['http://127.0.0.1:8080/dav/', 'http://[::1]:8080/dav/', 'http://169.254.169.254/', 'http://2130706433:8080/dav/']) {
    const r = await strict('POST', '/calendar/accounts/caldav/discover', token, { url, username: 'admin@acme.test', password: 'x' });
    if (r.status === 200) cal.push(`${url} → ${r.status} ${r.text.slice(0, 100)}`);
  }
  ok('calendar accounts can\'t reach internal addresses', cal.length === 0, cal);
  const mail: string[] = [];
  for (const host of ['127.0.0.1', 'localhost', '::1', '10.1.2.3', '169.254.169.254', '2130706433']) {
    const r = await strict('POST', '/crm/mailboxes/test', token, { email: 'sales@nerp.test', password: 'nerptest', imap_host: host, imap_port: 143, imap_security: 'none', smtp_host: host, smtp_port: 1025, smtp_security: 'none', smtp_auth: false });
    if (r.status === 200 && /"ok"\s*:\s*true/.test(r.text)) mail.push(`${host} → ${r.text.slice(0, 120)}`);
  }
  ok('mailboxes can\'t use internal mail servers', mail.length === 0, mail);
  const leaked = INTERNAL.length ? (await strict('POST', '/bi/datasets', token, { name: `Pen ssrf ${stamp}`, kind: 'url', url: 'http://127.0.0.1:5434/' })).text : '';
  ok('the refusal doesn\'t say what answers inside', !/postgres|redis|SSH|HTTP\/1\.1 200|connection refused/i.test(leaked), leaked.slice(0, 200));
} finally {
  server.kill('SIGTERM');
}

// ---------------------------------------------------------------------------
// A meeting room booked by calendar events at the same moment
// ---------------------------------------------------------------------------
const room = (await admin.post('/booking/resources', { name: `Pen cal room ${stamp}`, kind: 'room', capacity: 1, timezone: 'UTC', min_notice_minutes: 0 })).data;
const people = await Promise.all(['Ola', 'Pia', 'Quin', 'Rae', 'Sam'].map((n) => person(n)));
const t0 = new Date(Date.now() + 12 * 86400e3);
t0.setUTCHours(9, 0, 0, 0);
const made = await Promise.all(people.map((p, i) => p.post('/calendar/events', { title: `Pen room clash ${i} ${stamp}`, timezone: 'UTC', start: t0.toISOString(), end: new Date(+t0 + 3600e3).toISOString(), room_id: room.id })));
const later = await admin.post('/calendar/events', { title: `Pen room later ${stamp}`, timezone: 'UTC', start: t0.toISOString(), end: new Date(+t0 + 3600e3).toISOString(), room_id: room.id });
ok('(one more, afterwards, is refused)', later.status === 409 || later.status === 400, [later.status, later.data]);
ok('a room for one, taken by 5 events at once, is taken once', made.filter((r: any) => r.status === 200).length === 1, made.map((r: any) => `${r.status} ${r.data?.error?.message ?? ''}`));
await admin.del(`/booking/resources/${room.id}`);

// ---------------------------------------------------------------------------
// Fields people may not set on themselves
// ---------------------------------------------------------------------------
const mo = await person('Mo');
const sneaky = { is_guest: false, is_active: true, invite_pending: false, join_pending: false, auth_source: 'ldap', email: `boss.${stamp}@acme.test`, roles: ['admin'], role_ids: ['00000000-0000-4000-8000-000000000000'], permissions: ['*'], is_admin: true, totp_secret: 'AAAA', password_hash: 'x', department_ids: [], manager_id: admin.id };
for (const [m, p] of [['PATCH', '/profile'], ['PUT', '/profile'], ['PATCH', '/me'], ['PATCH', `/users/${mo.id}`], ['PUT', '/me/ui']] as const) await mo.req(m, p, sneaky);
const me = (await mo.get('/auth/me')).data;
const user = me.user ?? me;
ok('nobody gives themselves roles, permissions or another email', !JSON.stringify(user.permissions ?? []).includes('"*"') && user.email === mo.email && !(user.roles ?? []).some((r: any) => (r.code ?? r) === 'admin'), user);
ok('…and they can still sign in with their own password', (await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: mo.email, password: 'password123' }) })).status === 200);
ok('only administrators set everyone\'s menu', (await mo.put('/settings/ui-default', { hidden: ['crm'], pinned: [] })).status === 403);

await finish();
