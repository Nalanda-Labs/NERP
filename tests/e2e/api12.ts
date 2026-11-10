// Web forms: builder API, public form, spam protection, records, matching, UTM, auto-reply.
import { execSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
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
const waitFor = async (fn, ms = 20000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(300); } return v; };
const pub = async (method: string, path: string, body?: any, headers: Record<string, string> = {}) => { const r = await fetch(`http://localhost:8080/api${path}`, { method, headers: { 'content-type': 'application/json', ...headers }, body: body ? JSON.stringify(body) : undefined }); return { status: r.status, data: await r.json().catch(() => null) }; };
// Each simulated visitor comes from its own address, as browsers on the internet do.
let visitor = 0; const from = () => ({ 'x-forwarded-for': `203.0.113.${++visitor % 250}` });

// ---- building a lead form ----
let r: any = await admin.get('/crm/webforms/targets?entity_type=1');
ok('lead targets', r.data.items.some((x) => x.code === 'email') && r.data.items.some((x) => x.code === 'first_name'), r.data);
ok('employees cannot build forms', (await sam.post('/crm/webforms', { name: 'x', fields: [{ label: 'a', kind: 'text' }] })).status === 403);
ok('bad mapping rejected', (await admin.post('/crm/webforms', { name: 'x', fields: [{ label: 'a', kind: 'text', maps_to: 'contact.email' }] })).status === 400);
ok('select needs options', (await admin.post('/crm/webforms', { name: 'x', fields: [{ label: 'a', kind: 'select' }] })).status === 400);
ok('contacts are not a form target', (await admin.post('/crm/webforms', { name: 'x', entity_type: 3, fields: [{ label: 'a', kind: 'text' }] })).status === 400);
r = await admin.post('/crm/webforms', {
  name: `Contact us ${stamp}`, entity_type: 1, responsible: [sam.id, ivy.id],
  fields: [
    { label: 'First name', kind: 'text', required: true, maps_to: 'first_name' },
    { label: 'Last name', kind: 'text', maps_to: 'last_name' },
    { label: 'Email', kind: 'email', required: true, maps_to: 'email' },
    { label: 'Phone', kind: 'phone', maps_to: 'phone' },
    { label: 'Team size', kind: 'select', options: ['1-10', '11-50', '50+'] },
    { label: 'Message', kind: 'textarea', maps_to: 'comments' },
  ],
  settings: { title: 'Talk to sales', button: 'Send', success_message: 'Thanks, we will call you!', consent_text: 'I agree to be contacted.', color: '#0b57d0' },
});
ok('create form', r.status === 200 && r.data.key.length >= 12 && r.data.fields.map((f) => f.key).join() === 'f1,f2,f3,f4,f5,f6', r.data);
const form = r.data;

// ---- the public side ----
r = await pub('GET', `/public/forms/${form.key}`);
ok('public definition (no internals)', r.status === 200 && r.data.fields.length === 6 && r.data.token && !('responsible' in r.data) && !('id' in r.data) && r.data.settings.title === 'Talk to sales', r.data);
let token = r.data.token;
const vals = (n) => ({ f1: `Pat${n}`, f2: `Lee${stamp}`, f3: `pat${n}.${stamp}@example.com`, f4: '+1 555 0100', f5: '11-50', f6: 'Need 20 seats' });
r = await pub('POST', `/public/forms/${form.key}/submit`, { values: vals(1), token, consent: true }, from());
ok('too-fast submission silently ignored', r.status === 200 && !(await admin.get(`/crm/webforms/${form.id}`)).data.submissions, r.data);
await sleep(2100);
ok('consent required', (await pub('POST', `/public/forms/${form.key}/submit`, { values: vals(1), token }, from())).status === 400);
ok('required field', (await pub('POST', `/public/forms/${form.key}/submit`, { values: { ...vals(1), f1: '' }, token, consent: true }, from())).data.error.message.includes('First name'));
ok('invalid email', (await pub('POST', `/public/forms/${form.key}/submit`, { values: { ...vals(1), f3: 'nope' }, token, consent: true }, from())).status === 400);
ok('forged token', (await pub('POST', `/public/forms/${form.key}/submit`, { values: vals(1), token: '1.abc', consent: true }, from())).status === 400);
r = await pub('POST', `/public/forms/${form.key}/submit`, { values: vals(1), token, consent: true, website: 'http://spam' }, from());
ok('honeypot: fake success, nothing stored', r.status === 200 && !(await admin.get(`/crm/webforms/${form.id}`)).data.submissions);
r = await pub('POST', `/public/forms/${form.key}/submit`, { values: vals(1), token, consent: true, utm: { source: 'google', medium: 'cpc', campaign: 'spring' }, page_url: 'https://acme.example/pricing?utm_source=google', referrer: 'https://www.google.com/' }, from());
ok('submission accepted', r.status === 200 && r.data.message === 'Thanks, we will call you!', r.data);
await sleep(300);
r = await admin.get(`/crm/webforms/${form.id}/submissions`);
const s1 = r.data.items[0];
ok('submission logged with record', s1?.item_id && s1.entity_type === 1 && s1.utm.source === 'google' && s1.data.length === 6, r.data);
const lead = (await admin.get(`/crm/lead/items/${s1.item_id}`)).data;
ok('lead fields mapped', lead.first_name === 'Pat1' && lead.last_name === `Lee${stamp}` && lead.source === 'web_form' && lead.source_details.startsWith('https://acme.example/pricing') && lead.multifields.some((m) => m.kind === 'email' && m.value === `pat1.${stamp}@example.com`), lead);
ok('unmapped answers in the comment', lead.comments.includes('Need 20 seats') && lead.comments.includes('Team size: 11-50'), lead.comments);
ok('UTM on the record', lead.utm?.source === 'google' && lead.utm.campaign === 'spring', lead.utm);
ok('title from form + name', lead.title === `Contact us ${stamp}: Pat1 Lee${stamp}`, lead.title);
r = await admin.get(`/crm/items/${lead.id}/timeline`);
ok('timeline entry with answers', r.data.items.some((e) => e.kind === 'webform' && e.data.values.length === 6));
await pub('POST', `/public/forms/${form.key}/submit`, { values: vals(2), token, consent: true }, from());
await sleep(300);
const items = (await admin.get(`/crm/webforms/${form.id}/submissions`)).data.items;
const owners = await Promise.all(items.map(async (s) => (await admin.get(`/crm/lead/items/${s.item_id}`)).data.assigned_to));
ok('responsible in turn', new Set(owners).size === 2 && owners.every((o) => [sam.id, ivy.id].includes(o)), owners);
ok('notified', (await waitFor(async () => [...(await sam.get('/notifications')).data.items, ...(await ivy.get('/notifications')).data.items].filter((n) => n.kind === 'crm.webform').length >= 2)));

// ---- rate limit per visitor ----
let limited = false;
for (let i = 0; i < 12; i++) { const x = await pub('POST', `/public/forms/${form.key}/submit`, { values: vals(100 + i), token, consent: true }, { 'x-forwarded-for': '198.51.100.7' }); if (x.status === 429) limited = true; }
ok('rate limited per visitor (10 per 10 min)', limited);

// ---- deal form: contact matching, company, auto-reply ----
for (const m of (await admin.get('/crm/mailboxes')).data.items) await admin.del(`/crm/mailboxes/${m.id}`);
const mb = (await admin.post('/crm/mailboxes', { email: 'sales@nerp.test', password: 'nerptest', imap_host: '127.0.0.1', imap_port: 143, imap_security: 'none', smtp_host: '127.0.0.1', smtp_port: 1025, smtp_security: 'starttls', smtp_auth: false, allow_invalid_certs: true })).data;
const existing = (await sam.post('/crm/contact/items', { first_name: 'Known', last_name: `Buyer${stamp}`, multifields: [{ kind: 'email', value: `known.${stamp}@example.com` }] })).data.id;
const cat = (await admin.get('/crm/config')).data.types.find((t) => t.code === 'deal').categories.find((c) => c.is_default);
r = await admin.post('/crm/webforms', {
  name: `Quote request ${stamp}`, entity_type: 2, category_id: cat.id, stage_id: cat.stages[1].id,
  fields: [
    { label: 'Name', kind: 'text', required: true, maps_to: 'contact.first_name' },
    { label: 'Email', kind: 'email', required: true, maps_to: 'contact.email' },
    { label: 'Company', kind: 'text', maps_to: 'company.name' },
    { label: 'Budget', kind: 'number', maps_to: 'opportunity' },
  ],
  reply_mailbox: mb.id, reply_subject: `We got your request, {{f1}} ${stamp}`, reply_body: 'Hello {{f1}},\nthanks for contacting us via {{form}}.',
});
ok('deal form', r.status === 200, r.data);
const dform = r.data;
token = (await pub('GET', `/public/forms/${dform.key}`)).data.token;
await sleep(2100);
r = await pub('POST', `/public/forms/${dform.key}/submit`, { values: { f1: 'Known', f2: `KNOWN.${stamp}@example.com`, f3: `Newco ${stamp}`, f4: '2500' }, token }, from());
ok('deal submission', r.status === 200, r.data);
await sleep(300);
const ds = (await admin.get(`/crm/webforms/${dform.id}/submissions`)).data.items[0];
const deal = (await admin.get(`/crm/deal/items/${ds.item_id}`)).data;
ok('existing contact matched by email', ds.contact_id === existing && deal.contacts.some((c) => c.id === existing), [ds, deal.contacts]);
ok('company created and linked', deal.company_title === `Newco ${stamp}`, deal.company_title);
ok('pipeline, stage, amount', deal.category_id === cat.id && deal.stage_id === cat.stages[1].id && +deal.opportunity === 2500, deal);
const mail = await waitFor(async () => (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent(`subject:"We got your request, Known ${stamp}"`)}`)).json()).messages?.[0]);
ok('auto-reply sent from the mailbox', mail?.From.Address === 'sales@nerp.test' && mail.To[0].Address === `known.${stamp}@example.com`, mail);
ok('auto-reply on the deal', (await waitFor(async () => (await admin.get(`/crm/items/${deal.id}/emails`)).data.items.some((e) => e.direction === 'out' && e.status === 'sent'))));

// ---- inactive / deleted ----
await admin.patch(`/crm/webforms/${form.id}`, { is_active: false });
ok('inactive form is gone publicly', (await pub('GET', `/public/forms/${form.key}`)).status === 404);
r = await admin.get('/crm/webforms');
ok('list with counts', r.data.items.find((x) => x.id === form.id)?.submissions >= 2 && r.data.public_base.endsWith('/f/'));
ok('delete form keeps records', (await admin.del(`/crm/webforms/${form.id}`)).status === 200 && (await admin.get(`/crm/lead/items/${lead.id}`)).status === 200);
await admin.del(`/crm/webforms/${dform.id}`);
await admin.del(`/crm/mailboxes/${mb.id}`);

console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
