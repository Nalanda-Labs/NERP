// Signing in with other accounts (Google, Microsoft, Apple, Facebook, X): existing employees
// only, matched by an email address the provider verified or linked from their profile;
// Apple's signed client secret; unlinking; turning a provider off. Every provider is a
// stand-in on :9479 (the dev server runs with NERP_TEST_SIGNIN_BASE pointing there).
import http from 'node:http';
import crypto from 'node:crypto';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

const B = 'http://localhost:8080/api';
const BASE = 'http://127.0.0.1:9479';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Soc${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  c.email = email;
  return c;
};
const [eve, hal, ola] = await Promise.all(['Eve', 'Hal', 'Ola'].map(user));

// ---- the stand-in providers ----
const b64 = (o: object) => Buffer.from(JSON.stringify(o)).toString('base64url');
const jwt = (claims: object) => `${b64({ alg: 'none' })}.${b64(claims)}.x`;
const apple = crypto.generateKeyPairSync('ec', { namedCurve: 'P-256' });
const applePem = apple.privateKey.export({ type: 'pkcs8', format: 'pem' }).toString();
// What the next token request for a code returns (filled in once the nonce is known).
const codes = new Map<string, () => object>();
const seen: { path: string; body: string; auth?: string }[] = [];
const MS_PERSONAL = '9188040d-6c67-4c5b-b112-36a304b66dad';
const mock = http.createServer(async (req, res) => {
  let body = '';
  for await (const c of req) body += c;
  const url = new URL(req.url!, BASE);
  seen.push({ path: url.pathname, body, auth: req.headers.authorization });
  const send = (code: number, o: object) => { res.writeHead(code, { 'content-type': 'application/json' }); res.end(JSON.stringify(o)); };
  const form = new URLSearchParams(body);
  const code = form.get('code') ?? url.searchParams.get('code') ?? '';
  const answer = codes.get(code);
  if (url.pathname === '/accounts.google.com/.well-known/openid-configuration')
    return send(200, { issuer: `${BASE}/accounts.google.com`, authorization_endpoint: `${BASE}/accounts.google.com/auth`, token_endpoint: `${BASE}/accounts.google.com/token` });
  if (url.pathname === '/accounts.google.com/token' || url.pathname === '/login.microsoftonline.com/common/oauth2/v2.0/token') {
    if (form.get('client_secret') !== 'shh' || !form.get('code_verifier')) return send(401, { error: 'invalid_client' });
    return answer ? send(200, { access_token: 'at', id_token: jwt(answer()) }) : send(400, { error: 'invalid_grant' });
  }
  if (url.pathname === '/appleid.apple.com/auth/token') {
    // The client secret must be an ES256 JWT signed with the team's key.
    const [h, p, sig] = (form.get('client_secret') ?? '').split('.');
    const good = !!sig && crypto.verify('sha256', Buffer.from(`${h}.${p}`), { key: apple.publicKey, dsaEncoding: 'ieee-p1363' }, Buffer.from(sig, 'base64url'));
    const claims = good ? JSON.parse(Buffer.from(p, 'base64url').toString()) : {};
    const header = good ? JSON.parse(Buffer.from(h, 'base64url').toString()) : {};
    if (!good || claims.iss !== 'TEAM123' || claims.sub !== 'com.acme.nerp' || header.kid !== 'KEY456' || claims.aud !== 'https://appleid.apple.com') return send(401, { error: 'invalid_client' });
    return answer ? send(200, { access_token: 'at', id_token: jwt(answer()) }) : send(400, { error: 'invalid_grant' });
  }
  if (url.pathname === '/graph.facebook.com/v23.0/oauth/access_token')
    return url.searchParams.get('client_secret') === 'shh' && answer ? send(200, { access_token: `fb-${code}` }) : send(400, { error: { message: 'bad code' } });
  if (url.pathname === '/graph.facebook.com/v23.0/me') {
    const c = codes.get((req.headers.authorization ?? '').replace('Bearer fb-', ''));
    return c ? send(200, c()) : send(401, { error: { message: 'bad token' } });
  }
  if (url.pathname === '/api.x.com/2/oauth2/token') {
    const basic = Buffer.from((req.headers.authorization ?? '').replace('Basic ', ''), 'base64').toString();
    return basic === 'x-client:shh' && form.get('code_verifier') && answer ? send(200, { access_token: `x-${code}` }) : send(401, { error: 'unauthorized_client' });
  }
  if (url.pathname === '/api.x.com/2/users/me') {
    const c = codes.get((req.headers.authorization ?? '').replace('Bearer x-', ''));
    return c ? send(200, { data: c() }) : send(401, {});
  }
  send(404, { error: 'not here' });
});
await new Promise<void>((r) => mock.listen(9479, '127.0.0.1', r));

// The browser's round trip: start (or link), the provider answers with a code, the callback.
const goTo = async (startUrl: string, claims: (nonce: string) => object, how: 'get' | 'post' = 'get') => {
  const loc = new URL(startUrl);
  const st = loc.searchParams.get('state')!;
  const nonce = loc.searchParams.get('nonce') ?? '';
  const code = `c${crypto.randomUUID().slice(0, 8)}`;
  codes.set(code, () => claims(nonce));
  const r = how === 'post'
    ? await fetch(`${B}/auth/providers/callback`, { method: 'POST', redirect: 'manual', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({ code, state: st }).toString() })
    : await fetch(`${B}/auth/providers/callback?${new URLSearchParams({ code, state: st })}`, { redirect: 'manual' });
  return { status: r.status, location: r.headers.get('location') ?? '', cookie: r.headers.get('set-cookie')?.split(';')[0] ?? '', params: loc.searchParams };
};
const start = async (id: string) => {
  const r = await fetch(`${B}/auth/providers/${id}/start`, { redirect: 'manual' });
  return r.headers.get('location') ?? '';
};
const whoIs = async (cookie: string) => {
  const r = await fetch(`${B}/auth/refresh`, { method: 'POST', headers: { cookie } });
  return (await r.json())?.user?.email;
};
const loginError = (location: string) => new URL(location, 'http://x').searchParams.get('sso_error') ?? '';

try {
  // ---- setting up ----
  ok('only administrators set providers up', (await eve.get('/directory/providers')).status === 403);
  let r = await admin.post('/directory/providers', { kind: 'google', client_id: 'g-client' });
  ok('a secret is needed to turn one on', r.status === 400, r.data);
  r = await admin.post('/directory/providers', { kind: 'myspace', client_id: 'x', secret: 'y' });
  ok('known kinds only', r.status === 400);
  const google = (await admin.post('/directory/providers', { kind: 'google', client_id: 'g-client', secret: 'shh', label: `Google ${stamp}` })).data;
  ok('Google', google.label === `Google ${stamp}` && google.has_secret && !('secret' in google), google);
  const ms = (await admin.post('/directory/providers', { kind: 'microsoft', client_id: 'm-client', secret: 'shh', label: `Microsoft ${stamp}` })).data;
  ok('Apple needs its ids', (await admin.post('/directory/providers', { kind: 'apple', client_id: 'com.acme.nerp', secret: applePem })).status === 400);
  ok('and a real key', (await admin.post('/directory/providers', { kind: 'apple', client_id: 'com.acme.nerp', secret: 'not a key', team_id: 'TEAM123', key_id: 'KEY456' })).status === 400);
  const appleP = (await admin.post('/directory/providers', { kind: 'apple', client_id: 'com.acme.nerp', secret: applePem, team_id: 'TEAM123', key_id: 'KEY456', label: `Apple ${stamp}` })).data;
  const fb = (await admin.post('/directory/providers', { kind: 'facebook', client_id: 'fb-client', secret: 'shh', label: `Facebook ${stamp}` })).data;
  const x = (await admin.post('/directory/providers', { kind: 'x', client_id: 'x-client', secret: 'shh', label: `X ${stamp}` })).data;
  r = await new Client().get('/auth/providers');
  ok('the login page lists them', [google, ms, appleP, fb, x].every((p) => r.data.items.some((i: any) => i.id === p.id && i.label === p.label)), r.data);
  r = await admin.get('/directory/providers');
  ok('with the address to register at each provider', r.data.redirect_uri === 'http://localhost:8080/api/auth/providers/callback');

  // ---- Google: a verified email finds the employee ----
  let loc = await start(google.id);
  ok('off to Google, with PKCE and a nonce', loc.startsWith(`${BASE}/accounts.google.com/auth?`) && new URL(loc).searchParams.get('code_challenge_method') === 'S256' && !!new URL(loc).searchParams.get('nonce'), loc);
  let res = await goTo(loc, (nonce) => ({ iss: `${BASE}/accounts.google.com`, aud: 'g-client', sub: `g-${stamp}-eve`, email: eve.email, email_verified: true, nonce, exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('signed in as Eve', res.location === '/' && (await whoIs(res.cookie)) === eve.email, res);
  res = await goTo(await start(google.id), (nonce) => ({ iss: `${BASE}/accounts.google.com`, aud: 'g-client', sub: `g-${stamp}-eve`, email: 'changed@gmail.com', email_verified: false, nonce, exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('next time the account itself counts, whatever its email', res.location === '/' && (await whoIs(res.cookie)) === eve.email, res);
  res = await goTo(await start(google.id), (nonce) => ({ iss: `${BASE}/accounts.google.com`, aud: 'g-client', sub: `g-${stamp}-x`, email: hal.email, email_verified: false, nonce, exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('an unverified email finds no one', /not verified/.test(loginError(res.location)), res);
  res = await goTo(await start(google.id), (nonce) => ({ iss: `${BASE}/accounts.google.com`, aud: 'g-client', sub: `g-${stamp}-y`, email: `nobody.${stamp}@acme.test`, email_verified: true, nonce, exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('no account is made for strangers', /no employee account/.test(loginError(res.location)), res);
  res = await goTo(await start(google.id), (nonce) => ({ iss: `${BASE}/accounts.google.com`, aud: 'someone-else', sub: `g-${stamp}-eve`, email: eve.email, email_verified: true, nonce, exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('tokens for other applications are refused', /different application/.test(loginError(res.location)), res);
  res = await goTo(await start(google.id), () => ({ iss: `${BASE}/accounts.google.com`, aud: 'g-client', sub: `g-${stamp}-eve`, email: eve.email, email_verified: true, nonce: 'wrong', exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('and tokens from other sign-ins', /does not belong/.test(loginError(res.location)), res);
  const used = new URL(await start(google.id)).searchParams.get('state')!;
  await fetch(`${B}/auth/providers/callback?code=zzz&state=${used}`, { redirect: 'manual' });
  const again = await fetch(`${B}/auth/providers/callback?code=zzz&state=${used}`, { redirect: 'manual' });
  ok('a sign-in is used once', /already used/.test(loginError(again.headers.get('location') ?? '')));

  // ---- Microsoft: work accounts must prove the email's domain ----
  res = await goTo(await start(ms.id), (nonce) => ({ iss: 'https://login.microsoftonline.com/1111-tenant/v2.0', tid: '1111-tenant', aud: 'm-client', sub: `m-${stamp}-1`, email: hal.email, nonce, exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('a work account’s unproven email finds no one', /not verified/.test(loginError(res.location)), res);
  res = await goTo(await start(ms.id), (nonce) => ({ iss: `https://login.microsoftonline.com/${MS_PERSONAL}/v2.0`, tid: MS_PERSONAL, aud: 'm-client', sub: `m-${stamp}-2`, email: hal.email, nonce, exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('a personal account’s does', res.location === '/' && (await whoIs(res.cookie)) === hal.email, res);
  res = await goTo(await start(ms.id), (nonce) => ({ iss: 'https://login.microsoftonline.com/other/v2.0', tid: '1111-tenant', aud: 'm-client', sub: `m-${stamp}-3`, email: hal.email, xms_edov: true, nonce, exp: Math.floor(Date.now() / 1000) + 300 }));
  ok('the issuer must match the tenant', /different issuer/.test(loginError(res.location)), res);

  // ---- Apple: a signed client secret, the answer posted back ----
  loc = await start(appleP.id);
  ok('off to Apple, answering by form post', new URL(loc).searchParams.get('response_mode') === 'form_post', loc);
  res = await goTo(loc, (nonce) => ({ iss: `${BASE}/appleid.apple.com`, aud: 'com.acme.nerp', sub: `a-${stamp}`, email: ola.email, email_verified: 'true', nonce, exp: Math.floor(Date.now() / 1000) + 300 }), 'post');
  ok('signed in with Apple', res.location === '/' && (await whoIs(res.cookie)) === ola.email, res);

  // ---- Facebook ----
  res = await goTo(await start(fb.id), () => ({ id: `fb-${stamp}`, name: 'Eve', email: eve.email }));
  ok('signed in with Facebook', res.location === '/' && (await whoIs(res.cookie)) === eve.email, res);

  // ---- X: no email shared, so linked from the profile first ----
  loc = await start(x.id);
  res = await goTo(loc, () => ({ id: `x-${stamp}`, name: 'Hal', username: 'hal' }));
  ok('without an email it asks to link from the profile', /link the account from your profile/.test(loginError(res.location)), res);
  r = await hal.post(`/auth/providers/${x.id}/link`);
  res = await goTo(r.data.url, () => ({ id: `x-${stamp}`, name: 'Hal', username: 'hal' }));
  ok('linked from the profile', res.location === '/profile?linked=1', res);
  res = await goTo(await start(x.id), () => ({ id: `x-${stamp}`, name: 'Hal', username: 'hal' }));
  ok('then signs in with X', res.location === '/' && (await whoIs(res.cookie)) === hal.email, res);
  r = await eve.post(`/auth/providers/${x.id}/link`);
  res = await goTo(r.data.url, () => ({ id: `x-${stamp}`, name: 'Hal', username: 'hal' }));
  ok('nobody else can link the same account', /already linked to someone else/.test(new URL(res.location, 'http://x').searchParams.get('link_error') ?? ''), res);
  r = await hal.get('/auth/identities');
  ok('Hal sees the accounts linked', r.data.items.some((i: any) => i.provider_id === x.id) && r.data.items.some((i: any) => i.provider_id === ms.id), r.data);
  r = await hal.del(`/auth/identities/${x.id}`);
  ok('and unlinks one', !r.data.items.some((i: any) => i.provider_id === x.id));
  res = await goTo(await start(x.id), () => ({ id: `x-${stamp}`, name: 'Hal', username: 'hal' }));
  ok('which then no longer signs him in', /link the account/.test(loginError(res.location)), res);
  ok('X got PKCE and the client’s own credentials', seen.some((s) => s.path === '/api.x.com/2/oauth2/token' && s.auth?.startsWith('Basic ')));

  // ---- off, and deactivated people ----
  await admin.put(`/directory/providers/${fb.id}`, { enabled: false });
  ok('a provider turned off is not offered', !(await new Client().get('/auth/providers')).data.items.some((i: any) => i.id === fb.id));
  ok('nor can it be used', /not available/.test(loginError(await (async () => (await fetch(`${B}/auth/providers/${fb.id}/start`, { redirect: 'manual' })).headers.get('location') ?? '')())));
  await admin.post(`/users/${ola.id}/deactivate`).catch(() => {});
  await admin.patch(`/users/${ola.id}`, { is_active: false }).catch(() => {});
  res = await goTo(await start(appleP.id), (nonce) => ({ iss: `${BASE}/appleid.apple.com`, aud: 'com.acme.nerp', sub: `a-${stamp}`, email: ola.email, email_verified: true, nonce, exp: Math.floor(Date.now() / 1000) + 300 }), 'post');
  ok('deactivated people cannot sign in', /deactivated/.test(loginError(res.location)), res);
  r = await admin.get('/directory/providers');
  ok('administrators see how many are linked', r.data.items.find((p: any) => p.id === google.id)?.linked === 1, r.data.items);
} finally {
  for (const p of (await admin.get('/directory/providers')).data.items ?? []) if (String(p.label).includes(stamp)) await admin.del(`/directory/providers/${p.id}`);
  mock.close();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
