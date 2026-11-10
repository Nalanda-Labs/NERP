// Gmail / Microsoft 365 OAuth mailboxes, against a mock OAuth provider on :8099.
import { execSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { cleanupRun, requireOAuthMock } from './cleanup.ts';
// Skip before creating anything when the server has no mock OAuth provider.
await requireOAuthMock();
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 600)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'Doc' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const sam = await mk('sam'), ivy = await mk('ivy');
import http from 'node:http';
const waitFor = async (fn, ms = 30000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(400); } return v; };
import { createHash } from 'node:crypto';
import { Client } from './lib.ts';
const b64u = (b) => Buffer.from(b).toString('base64url');
const idToken = (claims) => `${b64u('{"alg":"none"}')}.${b64u(JSON.stringify(claims))}.sig`;
// Mock provider: /authorize consents at once; /token checks PKCE and hands out tokens.
const codes = new Map(); const seen: any[] = [];
const mock = http.createServer(async (req, res) => {
  const u = new URL(req.url!, 'http://127.0.0.1:8099');
  if (u.pathname.endsWith('/authorize')) {
    const code = 'code-' + Math.random().toString(36).slice(2);
    codes.set(code, { challenge: u.searchParams.get('code_challenge'), provider: u.pathname.split('/')[1], redirect: u.searchParams.get('redirect_uri'), scope: u.searchParams.get('scope') });
    res.writeHead(302, { location: `${u.searchParams.get('redirect_uri')}?code=${code}&state=${u.searchParams.get('state')}` }); return res.end();
  }
  if (u.pathname.endsWith('/token')) {
    let body = ''; for await (const c of req) body += c;
    const f = new URLSearchParams(body); seen.push(Object.fromEntries(f));
    const json = (s, o) => { res.writeHead(s, { 'content-type': 'application/json' }); res.end(JSON.stringify(o)); };
    if (f.get('client_secret') !== 'shh') return json(401, { error: 'invalid_client' });
    if (f.get('grant_type') === 'refresh_token') return f.get('refresh_token')!.startsWith('rt-') ? json(200, { access_token: 'at-refreshed', expires_in: 3600, refresh_token: 'rt-rotated' }) : json(400, { error: 'invalid_grant' });
    const c = codes.get(f.get('code')); codes.delete(f.get('code'));
    if (!c) return json(400, { error: 'invalid_grant', error_description: 'unknown code' });
    if (createHash('sha256').update(f.get('code_verifier')!).digest('base64url') !== c.challenge) return json(400, { error: 'invalid_grant', error_description: 'PKCE mismatch' });
    if (f.get('redirect_uri') !== c.redirect) return json(400, { error: 'invalid_grant', error_description: 'redirect mismatch' });
    const email = c.provider === 'google' ? { email: `ann.${stamp}@gmail.example`, name: 'Ann G' } : { preferred_username: `Ann.${stamp}@Contoso.example`, name: 'Ann M' };
    return json(200, { access_token: 'at-1', refresh_token: 'rt-1', expires_in: 30, id_token: idToken(email) });
  }
  res.writeHead(404); res.end();
});
await new Promise<void>((r) => mock.listen(8099, '127.0.0.1', () => r()));

// Follows the browser's round trip: start → provider → our callback → back to the app.
async function connect(client: any, kind: string, body?: any) {
  const s = await client.post(`/crm/mailboxes/oauth/${kind}/start`, body);
  if (s.status !== 200) return { start: s };
  const a = await fetch(s.data.url, { redirect: 'manual' });
  const cb = new URL(a.headers.get('location')!);
  const back = await fetch(`http://localhost:8080${cb.pathname}${cb.search}`, { redirect: 'manual' });
  return { start: s, url: s.data.url, callback: cb, back: new URL(back.headers.get('location')!), backStatus: back.status };
}

let r: any = await sam.get('/crm/mailboxes/oauth/providers');
ok('both providers configured', r.data.google && r.data.microsoft && r.data.redirect_uri.endsWith('/api/crm/mailboxes/oauth/callback'), r.data);
for (const m of (await sam.get('/crm/mailboxes')).data.items) if (m.is_owner) await sam.del(`/crm/mailboxes/${m.id}`);

let g: any = await connect(sam, 'google');
const gu = new URL(g.url);
ok('authorize url: PKCE, offline, Gmail scope', gu.pathname === '/google/authorize' && gu.searchParams.get('code_challenge_method') === 'S256' && gu.searchParams.get('access_type') === 'offline' && gu.searchParams.get('prompt') === 'consent' && gu.searchParams.get('scope')!.includes('https://mail.google.com/'), g.url);
ok('callback redirects back with success', g.backStatus === 303 && g.back.pathname === '/crm/mailboxes' && g.back.searchParams.get('connected') === `ann.${stamp}@gmail.example`, [g.backStatus, g.back?.href]);
r = await sam.get('/crm/mailboxes');
let gm = r.data.items.find((m) => m.auth_kind === 'google');
ok('gmail mailbox created', gm && gm.email === `ann.${stamp}@gmail.example` && gm.imap_host === 'imap.gmail.com' && gm.smtp_host === 'smtp.gmail.com' && gm.display_name === 'Ann G', r.data.items);
ok('token secrets never returned', !JSON.stringify(r.data).includes('rt-1') && !JSON.stringify(r.data).includes('at-1'));
const replay = await fetch(`http://localhost:8080${g.callback.pathname}${g.callback.search}`, { redirect: 'manual' });
ok('state is single-use', new URL(replay.headers.get('location')!).searchParams.get('error')?.includes('already used'));
const denied = await fetch(`http://localhost:8080/api/crm/mailboxes/oauth/callback?error=access_denied&error_description=${encodeURIComponent('The user said no')}`, { redirect: 'manual' });
ok('provider error shown', new URL(denied.headers.get('location')!).searchParams.get('error') === 'The user said no');

// The first access token expires in 30 s, so the first sync refreshes it.
await waitFor(async () => seen.some((x) => x.grant_type === 'refresh_token'));
ok('access token refreshed with the refresh token', seen.some((x) => x.grant_type === 'refresh_token' && x.refresh_token === 'rt-1'), seen.map((x) => x.grant_type));
gm = await waitFor(async () => (await sam.get('/crm/mailboxes')).data.items.find((m) => m.auth_kind === 'google' && m.last_sync_at), 60000);
ok('sync attempted with XOAUTH2 (fake token refused by the real server)', gm?.last_error && !/cannot be read/.test(gm.last_error), gm?.last_error);

let m = await connect(sam, 'microsoft');
const mu = new URL(m.url);
ok('microsoft scopes', mu.pathname === '/microsoft/authorize' && mu.searchParams.get('scope')!.includes('IMAP.AccessAsUser.All') && mu.searchParams.get('scope')!.includes('SMTP.Send') && mu.searchParams.get('scope')!.includes('offline_access'), m.url);
r = await sam.get('/crm/mailboxes');
const mm = r.data.items.find((x) => x.auth_kind === 'microsoft');
ok('microsoft mailbox from preferred_username, lower-cased', mm?.email === `ann.${stamp}@contoso.example` && mm.imap_host === 'outlook.office365.com' && mm.smtp_port === 587, mm);

// Reconnecting updates the same mailbox.
const before = (await sam.get('/crm/mailboxes')).data.items.length;
g = await connect(sam, 'google', { mailbox_id: gm.id });
ok('reconnect keeps one mailbox', g.back.searchParams.get('connected') && (await sam.get('/crm/mailboxes')).data.items.length === before);
ok('cannot reconnect someone else\'s mailbox', (await ivy.post('/crm/mailboxes/oauth/google/start', { mailbox_id: gm.id })).status === 404);
ok('unknown provider', (await sam.post('/crm/mailboxes/oauth/yahoo/start', {})).status === 400);

// Options can change; servers can't.
r = await sam.patch(`/crm/mailboxes/${gm.id}`, { create_leads: true, sync_days: 30, imap_host: 'evil.example' });
ok('oauth mailbox: options only', r.status === 200 && r.data.create_leads && r.data.sync_days === 30 && r.data.imap_host === 'imap.gmail.com', r.data);

for (const x of (await sam.get('/crm/mailboxes')).data.items) await sam.del(`/crm/mailboxes/${x.id}`);
mock.close();
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
