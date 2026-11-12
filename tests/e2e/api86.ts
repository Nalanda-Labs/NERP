// Sales intelligence: the tracking script's reports (visits by channel and source, pages,
// sites' host names), the journey of a visitor who sent a web form, offline ad links (/go/…)
// with click counts and QR codes, spend, and the traffic analysis with first-touch results.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const sam = new Client();
const samUser = (await admin.post('/users', { email: `sam.${stamp}@acme.test`, password: 'password123', first_name: 'Sam', last_name: `Tr${stamp}` })).data;
await sam.post('/auth/login', { email: `sam.${stamp}@acme.test`, password: 'password123' });
let ipn = 0;
const ip = () => ({ 'x-forwarded-for': `198.51.100.${(++ipn % 250) + 1}` });
const host = `shop-${stamp}.example`;
let siteId = '', linkId = '', formId = '';
const day = new Date().toISOString().slice(0, 10);

try {
  ok('only marketers set tracking up', (await sam.post('/tracking/sites', { name: 'x' })).status === 403);
  ok('host names are checked', (await admin.post('/tracking/sites', { name: 'x', domains: ['not a host'] })).status === 400);
  let r = await admin.post('/tracking/sites', { name: `Shop ${stamp}`, domains: [`https://www.${host}/`] });
  ok('a website with its tracking code', r.status === 200 && r.data.domains[0] === host && r.data.snippet.includes(`data-site="${r.data.key}"`) && r.data.snippet.includes('/track.js'), r.data);
  siteId = r.data.id;
  const key = r.data.key;
  const hit = async (v: string, u: string, ref = '', t = 'Page') => {
    const res = await fetch(`http://localhost:8080/api/public/track/${key}`, { method: 'POST', headers: { 'content-type': 'text/plain', ...ip() }, body: JSON.stringify({ v, u, t, r: ref }) });
    return res.status;
  };
  const a = crypto.randomUUID(), b = crypto.randomUUID(), c = crypto.randomUUID();
  ok('reports are answered 204, whatever they are', (await hit(a, `https://${host}/`, 'https://www.google.com/search?q=widgets', 'Home')) === 204 && (await hit('nope', 'x')) === 204);
  await hit(a, `https://${host}/pricing`, `https://${host}/`, 'Pricing');
  await hit(a, `https://${host}/?utm_source=newsletter&utm_medium=email&utm_campaign=nov-${stamp}`, '', 'Home');
  await hit(b, `https://www.${host}/?gclid=xyz`, 'https://www.google.com/', 'Home');
  await hit(c, `https://elsewhere-${stamp}.example/`, '', 'Not ours');
  const journeyOf = async (v: string) => (await admin.get(`/tracking/report?from=${day}&to=${day}&site_id=${siteId}`)).data;
  let rep = await journeyOf(a);
  const row = (k: string) => rep.rows.find((x: any) => x.key === k);
  ok('a search visit and an email visit for the first visitor', row('search')?.visits === 1 && row('email')?.visits === 1, rep.rows);
  ok('a paid visit from a click id', row('paid')?.visits === 1);
  ok('pages of other host names are dropped', rep.total.visits === 3 && rep.total.visitors === 2, rep.total);
  rep = (await admin.get(`/tracking/report?from=${day}&to=${day}&site_id=${siteId}&by=source`)).data;
  ok('by source', rep.rows.some((x: any) => x.key === 'google.com') && rep.rows.some((x: any) => x.key === 'google ads') && rep.rows.some((x: any) => x.key === 'newsletter'), rep.rows);
  rep = (await admin.get(`/tracking/report?from=${day}&to=${day}&site_id=${siteId}&by=campaign`)).data;
  ok('by campaign', rep.by === 'campaign' && rep.rows.some((x: any) => x.key === `nov-${stamp}` && x.visits === 1) && rep.rows.some((x: any) => x.key === '(none)' && x.visits === 2), rep.rows);
  rep = (await admin.get(`/tracking/report?from=${day}&to=${day}&site_id=${siteId}&by=landing`)).data;
  ok('by landing page (without the query)', rep.rows.some((x: any) => x.key === '/' && x.visits === 3), rep.rows);
  ok('only known groupings', (await admin.get(`/tracking/report?by=nope`)).status === 400);

  // ---- a web form sent by the first visitor ----
  const form = (await admin.post('/crm/webforms', { name: `Track form ${stamp}`, entity_type: 1, responsible: [samUser.id], fields: [{ label: 'Name', kind: 'text', required: true, maps_to: 'first_name' }, { label: 'Email', kind: 'email', required: true, maps_to: 'email' }] })).data;
  formId = form.id;
  const def = await fetch(`http://localhost:8080/api/public/forms/${form.key}`).then((x) => x.json());
  await sleep(3200);
  r = await fetch(`http://localhost:8080/api/public/forms/${form.key}/submit`, { method: 'POST', headers: { 'content-type': 'application/json', ...ip() }, body: JSON.stringify({ values: { f1: `Visitor ${stamp}`, f2: `visitor.${stamp}@example.com` }, token: def.token, visitor_id: a }) }).then(async (x) => ({ status: x.status, data: await x.json() }));
  ok('the visitor sends a form', r.status === 200, r.data);
  const lead = (await admin.get(`/crm/lead/items?q=${encodeURIComponent(`visitor.${stamp}`)}`)).data.items[0];
  const contact = (await admin.get(`/crm/contact/items?q=${encodeURIComponent(`visitor.${stamp}`)}`)).data.items[0];
  const rec = contact ?? lead;
  const j = (await admin.get(`/crm/items/${rec.id}/journey`)).data;
  ok('the record shows the visits that led to it', j.visits.length === 2 && j.first_touch.channel === 'search' && j.visits[0].pages === 2 && j.visits[0].page_list.map((p: any) => p.title).join() === 'Home,Pricing', j);
  ok('…and its own milestones', j.events.some((e: any) => e.kind === 'created'));
  ok('people who can\'t read the record don\'t see its journey', (await new Client().get(`/crm/items/${rec.id}/journey`)).status === 401);
  rep = await journeyOf(a);
  ok('the analysis counts the record for the first touch', rep.rows.find((x: any) => x.key === 'search')?.records === 1, rep.rows);

  // ---- an offline ad ----
  ok('ad links need a real address', (await admin.post('/tracking/links', { name: 'x', target_url: 'javascript:alert(1)', source: 'poster' })).status === 400);
  r = await admin.post('/tracking/links', { name: `Station poster ${stamp}`, target_url: `https://${host}/spring?x=1`, source: `poster-${stamp}`, campaign: 'spring' });
  ok('an ad link', r.status === 200 && r.data.short_url.includes(`/go/${r.data.code}`), r.data);
  linkId = r.data.id;
  const go = await fetch(`http://localhost:8080/go/${r.data.code}`, { redirect: 'manual', headers: ip() });
  const loc = go.headers.get('location') ?? '';
  ok('/go/… redirects with UTM tags and the ad\'s code', go.status === 303 || go.status === 302 || go.status === 307 ? /utm_source=poster-/.test(loc) && /utm_medium=offline/.test(loc) && /utm_campaign=spring/.test(loc) && loc.includes(`nerp_ad=${r.data.code}`) && loc.includes('x=1') : false, `${go.status} ${loc}`);
  // Again from the same address (forwarded addresses count only behind a trusted proxy).
  await fetch(`http://localhost:8080/go/${r.data.code}`, { redirect: 'manual' });
  await fetch(`http://localhost:8080/go/${r.data.code}`, { redirect: 'manual' });
  const d = crypto.randomUUID();
  await hit(d, loc, '', 'Spring');
  const l = (await admin.get('/tracking/links')).data.items.find((x: any) => x.id === linkId);
  ok('clicks count once an hour per address; visits through it are counted', l.clicks === 1 && l.visits === 1, l);
  rep = await journeyOf(d);
  ok('…as offline visits', rep.rows.find((x: any) => x.key === 'offline')?.visits === 1, rep.rows);
  const qr = await admin.req('GET', `/tracking/links/${linkId}/qr.svg`, undefined, true);
  ok('a QR code of the short link', qr.status === 200 && (await qr.text()).startsWith('<?xml') && qr.headers.get('content-type') === 'image/svg+xml');
  await admin.patch(`/tracking/links/${linkId}`, { is_active: false });
  ok('a switched-off link leads nowhere', (await fetch(`http://localhost:8080/go/${l.code}`, { redirect: 'manual' })).status === 404);

  // ---- spend and ROI ----
  const month = day.slice(0, 7);
  ok('spend can\'t be negative', (await admin.put('/tracking/spend', { source: 'search', month, amount: '-5' })).status === 400);
  r = await admin.put('/tracking/spend', { source: `poster-${stamp}`, month, amount: '200' });
  ok('spend for a source and month', r.status === 200, r.data);
  rep = (await admin.get(`/tracking/report?from=${day}&to=${day}&site_id=${siteId}&by=source`)).data;
  const poster = rep.rows.find((x: any) => x.key === `poster-${stamp}`);
  ok('the analysis puts spend beside it, with ROI', Number(poster?.spend) === 200 && Number(poster.roi) === -100, poster);
  ok('spend is listed by year', (await admin.get(`/tracking/spend?year=${month.slice(0, 4)}`)).data.items.some((x: any) => x.source === `poster-${stamp}`));
  await admin.put('/tracking/spend', { source: `poster-${stamp}`, month, amount: '' });
  ok('…and removed', !(await admin.get(`/tracking/spend?year=${month.slice(0, 4)}`)).data.items.some((x: any) => x.source === `poster-${stamp}`));
} finally {
  if (linkId) await admin.del(`/tracking/links/${linkId}`);
  if (siteId) await admin.del(`/tracking/sites/${siteId}`);
  if (formId) await admin.del(`/crm/webforms/${formId}`);
  for (const t of ['lead', 'contact']) for (const x of (await admin.get(`/crm/${t}/items?q=${encodeURIComponent(`visitor.${stamp}`)}`)).data.items ?? []) await admin.del(`/crm/${t}/items/${x.id}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
