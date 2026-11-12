// Duplicate prevention while typing: a new contact, lead or company is matched against the
// CRM's records of all three types (same email or phone, same or similar name, same business
// under another legal form), mail senders not in the CRM yet and chat visitors; creating it
// with adopt_history attaches their earlier emails and chats.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const made: Record<string, string> = {};
// A name nobody else has, with a typo-able spelling.
const last = `Quillfeather${s}`;
const q = (code: string, p: Record<string, string>) => admin.get(`/crm/${code}/match?` + new URLSearchParams(p));

try {
  const lead = (await admin.post('/crm/lead/items', { title: `Enquiry ${s}`, first_name: 'Jonathan', last_name: last, company_name: `Brightwater Kitchens ${s} Ltd`, multifields: [{ kind: 'phone', value_type: 'work', value: '+1 (555) 013-4242' }] })).data;
  const co = (await admin.post('/crm/company/items', { company_name: `Brightwater Kitchens ${s} Limited` })).data;
  const ct = (await admin.post('/crm/contact/items', { first_name: 'Mara', last_name: last, multifields: [{ kind: 'email', value_type: 'work', value: `mara.${s}@example.com` }] })).data;

  let r = await q('contact', { phones: '555 013 4242' });
  ok('a contact typed with a lead\'s phone finds the lead', r.status === 200 && r.data.records.some((x: any) => x.id === lead.id && x.type === 'lead' && x.reasons.includes('phone') && x.same_type === false), r.data);
  r = await q('contact', { name: `Jonathon ${last}` });
  ok('a similar name finds it too (a typo)', r.data.records.some((x: any) => x.id === lead.id && x.reasons.includes('similar_name')), r.data.records);
  r = await q('lead', { emails: `MARA.${s}@example.com` });
  ok('a lead typed with a contact\'s email finds the contact', r.data.records.some((x: any) => x.id === ct.id && x.reasons.includes('email') && x.link === `/crm/contact/${ct.id}`), r.data.records);
  r = await q('company', { company: `brightwater kitchens ${s} LLC` });
  const kinds = Object.fromEntries(r.data.records.map((x: any) => [x.id, x.reasons.join()]));
  ok('a company under another legal form finds the company and the lead\'s company', kinds[co.id] === 'company' && kinds[lead.id] === 'company', r.data.records);
  r = await q('contact', { name: `Zebedee ${s}` });
  ok('an unrelated name finds nothing', r.data.records.length === 0, r.data.records);
  r = await q('contact', { phones: '555 013 4242', exclude: lead.id });
  ok('the record being edited isn\'t its own duplicate', !r.data.records.some((x: any) => x.id === lead.id));

  // ---- mail senders and chat visitors not in the CRM ----
  const who = `nell.${s}@example.com`;
  const m1 = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, sent_at) VALUES ('in', 'received', $1, gen_random_uuid(), $2, $3, now() - interval '3 days') RETURNING id", [`<n1-${s}@x>`, `Quote for a kitchen ${s}`, who.toUpperCase()])).rows[0].id;
  const m2 = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, to_addrs, sent_at) VALUES ('out', 'sent', $1, gen_random_uuid(), 'Re: quote', 'sales@acme.test', $2, now() - interval '2 days') RETURNING id", [`<n2-${s}@x>`, JSON.stringify([who])])).rows[0].id;
  made.channel = (await db.query("INSERT INTO chat_channels (name, key) VALUES ($1, $2) RETURNING id", [`Dup ${s}`, `d${s}`])).rows[0].id;
  const chat = (await db.query("INSERT INTO chat_sessions (channel_id, token_hash, visitor_name, visitor_email) VALUES ($1, $2, 'Nell', $3) RETURNING id", [made.channel, `dh${s}`, who])).rows[0].id;
  r = await q('contact', { emails: who });
  ok('someone who emailed but isn\'t in the CRM', r.data.senders.count === 1 && r.data.senders.last_subject === `Quote for a kitchen ${s}` && r.data.records.length === 0, r.data);
  ok('…and chatted on the site', r.data.chats.count === 1);
  const nell = (await admin.post('/crm/contact/items', { first_name: 'Nell', last_name: last, multifields: [{ kind: 'email', value_type: 'work', value: who }], adopt_history: true })).data;
  ok('created with their history', nell.id && (await db.query('SELECT COUNT(*)::int AS n FROM crm_email_links WHERE item_id = $1 AND email_id = ANY($2)', [nell.id, [m1, m2]])).rows[0].n === 2, nell);
  ok('…the chat too', (await db.query('SELECT item_id FROM chat_sessions WHERE id = $1', [chat])).rows[0].item_id === nell.id);
  r = await q('contact', { emails: who });
  ok('now they are a record, with nothing left over', r.data.records.some((x: any) => x.id === nell.id) && r.data.senders.count === 0 && r.data.chats.count === 0, r.data);
  const plain = (await admin.post('/crm/contact/items', { first_name: 'Ora', last_name: last, multifields: [{ kind: 'email', value_type: 'work', value: `ora.${s}@example.com` }] })).data;
  const m3 = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, sent_at) VALUES ('in', 'received', $1, gen_random_uuid(), 'Hi', $2, now()) RETURNING id", [`<o-${s}@x>`, `ora2.${s}@example.com`])).rows[0].id;
  ok('without adopt_history nothing is attached', (await db.query('SELECT 1 FROM crm_email_links WHERE item_id = $1', [plain.id])).rowCount === 0 && m3);

  // ---- access ----
  const empRole = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
  const perms = (await admin.get('/crm/permissions')).data;
  made.original = JSON.stringify(Object.entries(perms.matrix[empRole.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope }))));
  made.role = empRole.id;
  await admin.put(`/crm/permissions/${empRole.id}`, { entries: JSON.parse(made.original).map((e: any) => (e.entity_type === 1 && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
  const u = new Client();
  await admin.post('/users', { email: `ivy.${s}@acme.test`, password: 'password123', first_name: 'Ivy', last_name: `M${s}` });
  await u.post('/auth/login', { email: `ivy.${s}@acme.test`, password: 'password123' });
  r = await u.get(`/crm/contact/match?phones=${encodeURIComponent('555 013 4242')}`);
  ok('a record the person may not see is only counted', r.status === 200 && !r.data.records.some((x: any) => x.id === lead.id) && r.data.hidden_count === 1, r.data);
} finally {
  if (made.original) await admin.put(`/crm/permissions/${made.role}`, { entries: JSON.parse(made.original) }).catch(() => {});
  await db.query('DELETE FROM crm_emails WHERE message_id LIKE $1', [`%-${s}@x>`]).catch(() => {});
  if (made.channel) await db.query('DELETE FROM chat_channels WHERE id = $1', [made.channel]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE last_name = $1 OR title = $2 OR company_name LIKE $3", [last, `Enquiry ${s}`, `Brightwater Kitchens ${s}%`]).catch((e) => console.log('cleanup', e.message));
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
