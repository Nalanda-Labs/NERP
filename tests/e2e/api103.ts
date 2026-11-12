// SAML 2.0 single sign-on against a stand-in identity provider (mock-saml-idp.ts): setting it
// up from its metadata (a URL; metadata without a signing certificate refused), NERP's own
// metadata, a signed request, signing in (an existing employee by email; a new one only when
// allowed; the allowed domains), and every answer that must be refused: unsigned, signed by
// another key, changed after signing, SHA-1, a forged assertion wrapped around the genuine one,
// for another audience, expired, used twice, for another request, or not asked for.
import { BASE, Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { IDP, IDP_ENTITY, startMockIdp, type Person } from './mock-saml-idp.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const before = (await admin.get('/directory')).data.saml?.config ?? {};
const idp = await startMockIdp();
const annEmail = `ann.${s}@acme.test`;
const ann = (await admin.post('/users', { email: annEmail, password: 'password123', first_name: 'Ann', last_name: `Saml${s}` })).data;

/** One sign-in: NERP's start, the identity provider, the post back. */
async function signIn(p: Person, opts: { reuse?: { SAMLResponse: string; RelayState: string }; noRelay?: boolean } = {}) {
  idp.next(p);
  let form = opts.reuse;
  if (!form) {
    const start = await fetch(`${BASE}/api/auth/saml/start`, { redirect: 'manual' });
    const to = start.headers.get('location') ?? '';
    if (!to.startsWith(`${IDP}/sso`)) return { status: start.status, location: to, cookie: null as string | null, form: null };
    form = await (await fetch(to, { headers: { accept: 'application/json' } })).json();
  }
  const body = new URLSearchParams({ SAMLResponse: form!.SAMLResponse, ...(opts.noRelay ? {} : { RelayState: form!.RelayState }) });
  const r = await fetch(`${BASE}/api/auth/saml/acs`, { method: 'POST', body, redirect: 'manual', headers: { 'content-type': 'application/x-www-form-urlencoded', origin: IDP } });
  const cookie = (r.headers.getSetCookie?.() ?? []).find((c) => c.startsWith('nerp_rt=')) ?? null;
  return { status: r.status, location: r.headers.get('location') ?? '', cookie, form };
}
const refused = (r: { location: string; cookie: string | null }, why?: RegExp) => !r.cookie && r.location.startsWith('/login?sso_error=') && (!why || why.test(decodeURIComponent(r.location.replace(/\+/g, ' '))));
const whoIs = async (cookie: string) => {
  const r = await fetch(`${BASE}/api/auth/refresh`, { method: 'POST', headers: { cookie: cookie.split(';')[0] } });
  const t = (await r.json()).access_token;
  return (await (await fetch(`${BASE}/api/auth/me`, { headers: { authorization: `Bearer ${t}` } })).json());
};

try {
  // ---- setting it up ----
  ok('metadata without a signing certificate is refused', (await admin.put('/directory/saml', { enabled: true, metadata_url: `${IDP}/metadata-nocert` })).status === 400);
  ok('turning it on needs metadata', (await admin.put('/directory/saml', { enabled: true })).status === 400);
  let r = await admin.put('/directory/saml', { enabled: true, metadata_url: `${IDP}/metadata`, button_label: `Company login ${s}`, allowed_domains: ['acme.test'], auto_create: false });
  ok('set up from the identity provider\'s metadata address', r.status === 200 && r.data.idp?.ok === true && r.data.idp.entity_id === IDP_ENTITY && r.data.config.metadata_xml.includes('IDPSSODescriptor'), r.data);
  ok('…NERP\'s addresses to give the identity provider', r.data.entity_id === `${BASE}/api/auth/saml/metadata` && r.data.acs_url === `${BASE}/api/auth/saml/acs` && r.data.config.sp_certificate.includes('BEGIN CERTIFICATE'), r.data);
  ok('only for people who manage settings', (await (await (async () => { const c = new Client(); await c.post('/auth/login', { email: annEmail, password: 'password123' }); return c; })()).put('/directory/saml', { enabled: false })).status === 403);
  const md = await fetch(`${BASE}/api/auth/saml/metadata`);
  const mdText = await md.text();
  ok('NERP\'s metadata: its entity, where answers go, its certificate', md.status === 200 && mdText.includes(`entityID="${BASE}/api/auth/saml/metadata"`) && mdText.includes(`Location="${BASE}/api/auth/saml/acs"`) && /X509Certificate>[A-Za-z0-9+/=\s]{200,}</.test(mdText), mdText.slice(0, 400));
  const info = (await (await fetch(`${BASE}/api/auth/sso`)).json()).saml;
  ok('the login page offers it', info?.enabled === true && info.label === `Company login ${s}`, info);

  // ---- signing in ----
  let a = await signIn({ email: annEmail, first: 'Ann', last: 'Saml' });
  const req = idp.requests.at(-1)!;
  ok('NERP\'s request: signed, from NERP\'s entity, answers to its address', req.signed && req.issuer === `${BASE}/api/auth/saml/metadata` && req.acs === `${BASE}/api/auth/saml/acs`, req);
  ok('an employee signs in (matched by email)', a.status === 303 && a.location === '/' && !!a.cookie, a);
  const me = a.cookie ? await whoIs(a.cookie) : null;
  ok('…as themselves', me?.id === ann.id || me?.user?.id === ann.id, me);
  ok('the same answer twice: refused', refused(await signIn({ email: annEmail }, { reuse: a.form! }), /already used|took too long/));
  ok('someone without an account (no automatic accounts)', refused(await signIn({ email: `new.${s}@acme.test` }), /no employee account/));
  ok('another email domain', refused(await signIn({ email: `x.${s}@elsewhere.example` }), /cannot sign in here/));

  // ---- answers that must be refused ----
  for (const [mode, label] of [
    ['unsigned', 'unsigned'], ['wrong-key', 'signed by another key'], ['tampered', 'changed after signing'], ['sha1', 'signed with SHA-1'],
    ['wrong-audience', 'for another service'], ['expired', 'expired'], ['other-request', 'answering another request'],
  ] as const) {
    ok(`refused: ${label}`, refused(await signIn({ email: annEmail, mode, attacker: `mallory.${s}@acme.test` }), /could not be verified/));
  }
  const wrapped = await signIn({ email: annEmail, mode: 'wrapped', attacker: `admin@acme.test` });
  const wrappedAs = wrapped.cookie ? await whoIs(wrapped.cookie) : null;
  ok('a forged assertion wrapped around the genuine one never signs in as the forged person', refused(wrapped) || (wrappedAs && (wrappedAs.id ?? wrappedAs.user?.id) === ann.id), [wrapped.location, wrappedAs?.email ?? wrappedAs?.user?.email]);
  ok('not asked for (sign-in started at the identity provider): refused', refused(await signIn({ email: annEmail }, { noRelay: true }), /not started here/));

  // ---- accounts made on first sign-in ----
  await admin.put('/directory/saml', { enabled: true, metadata_url: `${IDP}/metadata`, button_label: `Company login ${s}`, allowed_domains: ['acme.test'], auto_create: true });
  a = await signIn({ email: `newbie.${s}@acme.test`, nameId: `emp-${s}`, first: 'Nia', last: `New${s}` });
  const newbie = a.cookie ? await whoIs(a.cookie) : null;
  const u = (await admin.get(`/users?q=${encodeURIComponent(`newbie.${s}`)}`)).data.items?.[0];
  ok('a new employee, named from the attributes', a.location === '/' && u?.first_name === 'Nia' && u?.last_name === `New${s}` && !!newbie, [a.location, u]);
  a = await signIn({ email: `newbie.${s}@acme.test`, nameId: `emp-${s}` });
  ok('…signs in again as the same person', a.location === '/' && (await admin.get(`/users?q=${encodeURIComponent(`newbie.${s}`)}`)).data.items.length === 1);

  // ---- turned off ----
  await admin.put('/directory/saml', { enabled: false, metadata_url: `${IDP}/metadata` });
  ok('turned off: no button, no sign-in', (await (await fetch(`${BASE}/api/auth/sso`)).json()).saml?.enabled === false && (await fetch(`${BASE}/api/auth/saml/start`, { redirect: 'manual' })).headers.get('location')?.startsWith('/login?sso_error='));
} finally {
  await admin.put('/directory/saml', { enabled: false, metadata_url: before.metadata_url ?? '', metadata_xml: before.metadata_xml ?? '', button_label: before.button_label ?? '', allowed_domains: before.allowed_domains ?? [], auto_create: before.auto_create ?? false, email_attribute: before.email_attribute ?? '' }).catch(() => {});
  await idp.close();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
