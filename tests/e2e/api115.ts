// Retention rules: per kind of data, how long it is kept and what happens after; a run
// applies them and logs what it did. Every rule here keeps 3000 days and the test's own data
// is dated nine years back, so nothing else in the database is old enough to be touched.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
const me = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user;
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const before = (await db.query('SELECT * FROM retention_rules')).rows;
const OLD = new Date(Date.now() - 9 * 365 * 86400e3).toISOString();
const made: Record<string, string> = {};
const one = async (sql: string, p: unknown[]) => (await db.query(sql, p)).rows[0];

try {
  let r = await admin.get('/admin/retention');
  ok('the rules, all off at first', r.status === 200 && r.data.rules.length >= 12 && r.data.rules.every((x: any) => typeof x.due_now === 'number' && x.keep_days >= x.min_days), r.data.rules?.map((x: any) => [x.key, x.enabled]));
  ok('…and what is always kept', r.data.always_kept.some((x: string) => /audit trail/.test(x)));

  // ---- old data of every kind, and some that must stay ----
  const lead = (await admin.post('/crm/lead/items', { title: `Old lead ${s}` })).data;
  await db.query("UPDATE crm_items SET stage_semantics = 'failure', is_closed = TRUE, closed_at = $2, updated_at = $2 WHERE id = $1", [lead.id, OLD]);
  const gone = (await admin.post('/crm/contact/items', { first_name: 'Gone', last_name: `R${s}` })).data;
  made.gone = gone.id;
  const busy = (await admin.post('/crm/contact/items', { first_name: 'Busy', last_name: `R${s}` })).data;
  await admin.post('/crm/deal/items', { title: `Open deal ${s}`, contact_ids: [busy.id] });
  const fresh = (await admin.post('/crm/contact/items', { first_name: 'Fresh', last_name: `R${s}` })).data;
  await db.query('UPDATE crm_items SET updated_at = $2, last_activity_at = $2 WHERE id = ANY($1)', [[gone.id, busy.id], OLD]);
  const fd = new FormData(); fd.append('file', new Blob([`old attachment ${s}`], { type: 'text/plain' }), `att ${s}.txt`);
  const fileId = (await admin.req('POST', '/files', fd) as any).data.id;
  const mail = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, sent_at) VALUES ('in', 'received', $1, gen_random_uuid(), $2, 'x@example.com', $3) RETURNING id", [`<r-${s}@x>`, `Old mail ${s}`, OLD])).rows[0].id;
  await db.query('INSERT INTO crm_email_files (email_id, file_id) VALUES ($1, $2)', [mail, fileId]);
  made.channel = (await db.query("INSERT INTO chat_channels (name, key) VALUES ($1, $2) RETURNING id", [`Ret ${s}`, `r${s}`])).rows[0].id;
  const chat = (await db.query("INSERT INTO chat_sessions (channel_id, token_hash, started_at, closed_at, status) VALUES ($1, $2, $3, $3, 'closed') RETURNING id", [made.channel, `rh${s}`, OLD])).rows[0].id;
  const ticket = (await db.query("INSERT INTO support_tickets (title, channel, requester_name, requester_email, closed_at) VALUES ($1, 'email', 'Tess', 'tess@example.com', $2) RETURNING id", [`Old ticket ${s}`, OLD])).rows[0].id;
  made.ticket = ticket;
  made.site = (await db.query("INSERT INTO track_sites (name, key) VALUES ($1, $2) RETURNING id", [`Ret site ${s}`, `rk${s}`])).rows[0].id;
  const vis = (await db.query('INSERT INTO track_visitors (id) VALUES (gen_random_uuid()) RETURNING id')).rows[0].id;
  made.visitor = vis;
  const visit = (await db.query("INSERT INTO track_visits (visitor_id, site_id, channel, source, landing, started_at, last_at) VALUES ($1, $2, 'direct', '', '/', $3, $3) RETURNING id", [vis, made.site, OLD])).rows[0].id;
  const newVisit = (await db.query("INSERT INTO track_visits (visitor_id, site_id, channel, source, landing) VALUES ($1, $2, 'direct', '', '/') RETURNING id", [vis, made.site])).rows[0].id;
  made.form = (await db.query("INSERT INTO crm_webforms (key, name, entity_type) VALUES ($1, $2, 3) RETURNING id", [`rf${s}`, `Ret form ${s}`])).rows[0].id;
  const sub = (await db.query("INSERT INTO crm_webform_submissions (form_id, data, created_at) VALUES ($1, '{}', $2) RETURNING id", [made.form, OLD])).rows[0].id;
  const newSub = (await db.query("INSERT INTO crm_webform_submissions (form_id, data) VALUES ($1, '{}') RETURNING id", [made.form])).rows[0].id;
  made.campaign = (await db.query("INSERT INTO mkt_campaigns (name, slug) VALUES ($1, $2) RETURNING id", [`Ret ${s}`, `ret-${s}`])).rows[0].id;
  const rcp = (await db.query("INSERT INTO mkt_recipients (campaign_id, email, token, status, sent_at) VALUES ($1, 'm@example.com', $2, 'sent', $3) RETURNING id", [made.campaign, `rt${s}`, OLD])).rows[0].id;
  const booking = (await db.query("INSERT INTO bookings (starts_at, ends_at, client_name, client_email) VALUES ($1, $1::timestamptz + interval '1 hour', 'Bea', 'bea@example.com') RETURNING id", [OLD])).rows[0].id;
  made.booking = booking;
  const deal = (await admin.post('/crm/deal/items', { title: `Order deal ${s}` })).data;
  await db.query("INSERT INTO shop_orders (item_id, token, name, email, address, created_at) VALUES ($1, $2, 'Otto', 'otto@example.com', '2 Old Road', $3)", [deal.id, `ro${s}`, OLD]);
  const pr = (await db.query("INSERT INTO privacy_requests (kind, email, source, status, decided_at) VALUES ('access', $1, 'staff', 'done', $2) RETURNING id", [`old.${s}@example.com`, OLD])).rows[0].id;

  // ---- turning rules on ----
  const set = (key: string, action: string, keep_days = 3000, enabled = true) => admin.put(`/admin/retention/${key}`, { enabled, keep_days, action });
  ok('fewer days than the rule allows is refused', (await set('emails', 'delete', 10)).status === 400);
  ok('…and an action it doesn\'t offer', (await set('emails', 'anonymise')).status === 400);
  const rules: [string, string][] = [['lost_leads', 'delete'], ['inactive_contacts', 'anonymise'], ['emails', 'delete'], ['live_chats', 'delete'], ['tickets', 'anonymise'], ['site_visits', 'delete'], ['form_submissions', 'delete'], ['marketing', 'delete'], ['bookings', 'anonymise'], ['orders', 'anonymise'], ['privacy_requests', 'delete']];
  const dues: Record<string, number> = {};
  for (const [k, a] of rules) { const x = await set(k, a); dues[k] = x.data.due_now; }
  ok('each rule says how much it would affect now', Object.values(dues).every((n) => n >= 1), dues);

  r = await admin.post('/admin/retention/run', {});
  const res = Object.fromEntries((r.data.results ?? []).map((x: any) => [x.key, x]));
  ok('a run applies every rule that is on', r.status === 200 && rules.every(([k]) => res[k]?.affected >= 1 && !res[k]?.error), r.data.results);
  ok('lost leads go to the recycle bin', (await one('SELECT deleted_at FROM crm_items WHERE id = $1', [lead.id])).deleted_at);
  const g = await one('SELECT first_name, deleted_at FROM crm_items WHERE id = $1', [gone.id]);
  ok('an inactive contact is anonymised', g.first_name === 'Erased' && g.deleted_at, g);
  ok('…not one with an open deal, nor a recent one', (await one('SELECT first_name FROM crm_items WHERE id = $1', [busy.id])).first_name === 'Busy' && (await one('SELECT first_name FROM crm_items WHERE id = $1', [fresh.id])).first_name === 'Fresh');
  ok('old emails go, with their attachments', (await db.query('SELECT 1 FROM crm_emails WHERE id = $1', [mail])).rowCount === 0 && (await db.query('SELECT 1 FROM files WHERE id = $1', [fileId])).rowCount === 0);
  ok('old live chats go', (await db.query('SELECT 1 FROM chat_sessions WHERE id = $1', [chat])).rowCount === 0);
  const t = await one('SELECT requester_name, requester_email, title FROM support_tickets WHERE id = $1', [ticket]);
  ok('old tickets are anonymised', t.requester_name === 'Erased' && t.requester_email === null && t.title === `Old ticket ${s}`, t);
  ok('old visits go, new ones stay', (await db.query('SELECT 1 FROM track_visits WHERE id = $1', [visit])).rowCount === 0 && (await db.query('SELECT 1 FROM track_visits WHERE id = $1', [newVisit])).rowCount === 1);
  ok('old form submissions go, new ones stay', (await db.query('SELECT 1 FROM crm_webform_submissions WHERE id = $1', [sub])).rowCount === 0 && (await db.query('SELECT 1 FROM crm_webform_submissions WHERE id = $1', [newSub])).rowCount === 1);
  ok('old campaign recipients go', (await db.query('SELECT 1 FROM mkt_recipients WHERE id = $1', [rcp])).rowCount === 0);
  const b = await one('SELECT client_name, client_email FROM bookings WHERE id = $1', [booking]);
  ok('old bookings keep their time without who booked', b.client_name === 'Erased' && b.client_email === null, b);
  const o = await one('SELECT name, email, address FROM shop_orders WHERE item_id = $1', [deal.id]);
  ok('old orders keep their amounts without the contact details', o.name === 'Erased' && o.email === 'erased@invalid' && o.address === '', o);
  ok('old answered data requests go', (await db.query('SELECT 1 FROM privacy_requests WHERE id = $1', [pr])).rowCount === 0);

  r = await admin.post('/admin/retention/run', {});
  ok('the next run has nothing left to do', (r.data.results ?? []).filter((x: any) => rules.some(([k]) => k === x.key)).every((x: any) => x.affected === 0), r.data.results);
  r = await admin.get('/admin/retention');
  ok('the log shows what each run did, and who ran it', r.data.log.some((l: any) => l.key === 'emails' && l.affected >= 1 && l.by) && r.data.rules.find((x: any) => x.key === 'emails').last_run, r.data.log?.slice(0, 3));
  ok('…and the audit log too', (await db.query("SELECT 1 FROM audit_log WHERE action = 'retention.applied' AND entity_id = 'emails' AND created_at > now() - interval '5 minutes'")).rowCount! >= 1);
  ok('changing a rule is audited', (await db.query("SELECT 1 FROM audit_log WHERE action = 'retention.rule' AND entity_id = 'bookings' AND created_at > now() - interval '5 minutes'")).rowCount! >= 1);
  await set('emails', 'delete', 3000, false);
  ok('a rule turned off doesn\'t run', !((await admin.post('/admin/retention/run', {})).data.results ?? []).some((x: any) => x.key === 'emails'));

  const u = new Client();
  await admin.post('/users', { email: `rex.${s}@acme.test`, password: 'password123', first_name: 'Rex', last_name: `R${s}` });
  await u.post('/auth/login', { email: `rex.${s}@acme.test`, password: 'password123' });
  ok('retention is for those who handle personal data', (await u.get('/admin/retention')).status === 403 && (await u.post('/admin/retention/run', {})).status === 403);
} finally {
  await db.query('DELETE FROM retention_rules').catch(() => {});
  for (const x of before) await db.query('INSERT INTO retention_rules (key, enabled, keep_days, action, updated_by, updated_at) VALUES ($1, $2, $3, $4, $5, $6)', [x.key, x.enabled, x.keep_days, x.action, x.updated_by, x.updated_at]).catch(() => {});
  if (made.ticket) await db.query('DELETE FROM support_tickets WHERE id = $1', [made.ticket]).catch(() => {});
  if (made.booking) await db.query('DELETE FROM bookings WHERE id = $1', [made.booking]).catch(() => {});
  await db.query('DELETE FROM shop_orders WHERE token = $1', [`ro${s}`]).catch(() => {});
  if (made.channel) await db.query('DELETE FROM chat_channels WHERE id = $1', [made.channel]).catch(() => {});
  if (made.campaign) await db.query('DELETE FROM mkt_campaigns WHERE id = $1', [made.campaign]).catch(() => {});
  if (made.visitor) await db.query('DELETE FROM track_visitors WHERE id = $1', [made.visitor]).catch(() => {});
  if (made.site) await db.query('DELETE FROM track_sites WHERE id = $1', [made.site]).catch(() => {});
  if (made.form) await db.query('DELETE FROM crm_webforms WHERE id = $1', [made.form]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title LIKE $1 OR last_name = $2 OR id = $3", [`% ${s}`, `R${s}`, made.gone ?? null]).catch((e) => console.log('cleanup', e.message));
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
