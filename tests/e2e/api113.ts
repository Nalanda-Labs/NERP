// One client, everything: a company's history across modules — its contacts and deals,
// comments, activities, emails, bookings, tickets, web forms, marketing emails, site visits,
// signatures and tasks — newest first, by kind, page by page.
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
const ago = (h: number) => new Date(Date.now() - h * 3600e3).toISOString();
const made = { form: '', campaign: '', site: '', ticket: '', task: '' };
let original: any[] | null = null, roleId = '';

try {
  const co = (await admin.post('/crm/company/items', { company_name: `Everything Co ${s}` })).data;
  const ann = (await admin.post('/crm/contact/items', { first_name: 'Ann', last_name: `E${s}`, company_id: co.id })).data;
  const other = (await admin.post('/crm/contact/items', { first_name: 'Olaf', last_name: `E${s}` })).data;
  const deal = (await admin.post('/crm/deal/items', { title: `Everything deal ${s}`, company_id: co.id, contact_ids: [ann.id] })).data;
  await db.query("UPDATE crm_items SET created_at = $2 WHERE id = $1", [deal.id, ago(100)]);
  await db.query("UPDATE crm_stage_history SET entered_at = $2 WHERE item_id = $1", [deal.id, ago(100)]);
  // …and a stage it moved to later.
  await db.query("INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics, entered_at) SELECT id, category_id, stage_id, 'process', $2 FROM crm_items WHERE id = $1", [deal.id, ago(11.5)]);

  // History in many modules, an hour apart (newest: the comment).
  await db.query("INSERT INTO crm_timeline (item_id, kind, author_id, body, created_at) VALUES ($1, 'comment', $2, $3, $4)", [deal.id, me.id, `Spoke about the roof ${s}`, ago(1)]);
  await db.query("INSERT INTO crm_activities (item_id, kind, subject, responsible_id, created_by, is_completed, completed_at, created_at) VALUES ($1, 'call', $2, $3, $3, TRUE, $4, $4)", [ann.id, `Called Ann ${s}`, me.id, ago(2)]);
  const email = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, sent_at) VALUES ('in', 'received', $1, gen_random_uuid(), $2, 'ann@example.com', $3) RETURNING id", [`<e-${s}@x>`, `Quote please ${s}`, ago(3)])).rows[0].id;
  await db.query('INSERT INTO crm_email_links (email_id, item_id) VALUES ($1, $2), ($1, $3)', [email, deal.id, ann.id]);
  await db.query("INSERT INTO bookings (starts_at, ends_at, client_name, contact_id, status) VALUES ($1, $1::timestamptz + interval '1 hour', 'Ann', $2, 'confirmed')", [ago(4), ann.id]);
  made.ticket = (await db.query("INSERT INTO support_tickets (title, channel, contact_id, created_at) VALUES ($1, 'email', $2, $3) RETURNING id", [`Leaking roof ${s}`, ann.id, ago(5)])).rows[0].id;
  made.form = (await db.query("INSERT INTO crm_webforms (key, name, entity_type) VALUES ($1, $2, 3) RETURNING id", [`f${s}`, `Contact us ${s}`])).rows[0].id;
  await db.query("INSERT INTO crm_webform_submissions (form_id, contact_id, data, created_at) VALUES ($1, $2, '{}', $3)", [made.form, ann.id, ago(6)]);
  made.campaign = (await db.query("INSERT INTO mkt_campaigns (name, slug) VALUES ($1, $2) RETURNING id", [`Spring ${s}`, `spring-${s}`])).rows[0].id;
  await db.query("INSERT INTO mkt_recipients (campaign_id, item_id, email, token, status, sent_at, opened_at) VALUES ($1, $2, 'ann@example.com', $3, 'sent', $4, $4)", [made.campaign, ann.id, `t${s}`, ago(7)]);
  made.site = (await db.query("INSERT INTO track_sites (name, key) VALUES ($1, $2) RETURNING id", [`Site ${s}`, `k${s}`])).rows[0].id;
  const vis = (await db.query('INSERT INTO track_visitors (id, item_id) VALUES (gen_random_uuid(), $1) RETURNING id', [ann.id])).rows[0].id;
  await db.query("INSERT INTO track_visits (visitor_id, site_id, channel, source, landing, pages, started_at, last_at) VALUES ($1, $2, 'search', 'google', '/roofs', 3, $3, $3)", [vis, made.site, ago(8)]);
  await db.query("INSERT INTO sign_requests (item_id, title, original_sha256, status, created_at, completed_at) VALUES ($1, $2, 'x', 'completed', $3, $4)", [deal.id, `Contract ${s}`, ago(10), ago(9)]);
  made.task = (await db.query("INSERT INTO tasks (title, created_by, responsible_id, status, created_at) VALUES ($1, $2, $2, 'new', $3) RETURNING id", [`Measure the roof ${s}`, me.id, ago(11)])).rows[0].id;
  await db.query('INSERT INTO task_crm_links (task_id, item_id) VALUES ($1, $2)', [made.task, deal.id]);
  // Someone else's history: not the client's.
  await db.query("INSERT INTO crm_timeline (item_id, kind, author_id, body, created_at) VALUES ($1, 'comment', $2, $3, $4)", [other.id, me.id, `Not theirs ${s}`, ago(0.5)]);

  let r = await admin.get(`/crm/items/${co.id}/everything`);
  const mine = (r.data.items ?? []).filter((e: any) => e.at > ago(12));
  const kinds = mine.map((e: any) => e.kind);
  ok('the company\'s history across modules', r.status === 200 && ['comment', 'activity', 'email', 'booking', 'ticket', 'form', 'marketing', 'visit', 'document', 'task', 'record'].every((k) => kinds.includes(k)), kinds);
  ok('newest first', mine[0]?.kind === 'comment' && mine.every((e: any, i: number) => i === 0 || e.at <= mine[i - 1].at), mine.map((e: any) => [e.kind, e.at]));
  ok('…through its contacts and deals, with where it happened', mine.find((e: any) => e.kind === 'activity')?.record?.id === ann.id && mine.find((e: any) => e.kind === 'comment')?.link === `/crm/deal/${deal.id}`, mine.slice(0, 2));
  ok('an email linked to two of its records shows once', mine.filter((e: any) => e.kind === 'email').length === 1 && /Quote please/.test(mine.find((e: any) => e.kind === 'email').title));
  ok('signing shows sent and signed', mine.filter((e: any) => e.kind === 'document').map((e: any) => e.title).join('|') === `Signed: Contract ${s}|Sent for signature: Contract ${s}`, mine.filter((e: any) => e.kind === 'document'));
  ok('the ticket links to the help desk', mine.find((e: any) => e.kind === 'ticket')?.link === `/support/tickets/${made.ticket}`);
  ok('other people\'s history isn\'t in it', !JSON.stringify(r.data.items).includes(`Not theirs ${s}`));
  ok('the kinds to filter by', r.data.kinds.some((k: any) => k.kind === 'ticket') && r.data.kinds.some((k: any) => k.kind === 'money'));

  r = await admin.get(`/crm/items/${co.id}/everything?kinds=email,ticket`);
  ok('filtered by kind', r.data.items.length >= 2 && r.data.items.every((e: any) => ['email', 'ticket'].includes(e.kind)), r.data.items.map((e: any) => e.kind));

  // Page by page.
  const p1 = (await admin.get(`/crm/items/${co.id}/everything?limit=3`)).data;
  const p2 = (await admin.get(`/crm/items/${co.id}/everything?limit=3&before=${encodeURIComponent(p1.next_before)}`)).data;
  ok('page by page', p1.items.length === 3 && p1.next_before && p2.items.length === 3 && p2.items[0].at < p1.items[2].at, [p1.items.map((e: any) => e.at), p2.items.map((e: any) => e.at)]);

  r = await admin.get(`/crm/items/${ann.id}/everything`);
  ok('a contact\'s own history', r.data.items.some((e: any) => e.kind === 'ticket') && r.data.items.some((e: any) => e.kind === 'comment'), r.data.items.map((e: any) => e.kind));
  ok('only contacts and companies', (await admin.get(`/crm/items/${deal.id}/everything`)).status === 400);
  ok('only for those signed in', (await new Client().get(`/crm/items/${co.id}/everything`)).status === 401);

  // Someone without help desk or accounting rights doesn't see those kinds.
  const u = new Client();
  await admin.post('/users', { email: `ed.${s}@acme.test`, password: 'password123', first_name: 'Ed', last_name: `E${s}` });
  await u.post('/auth/login', { email: `ed.${s}@acme.test`, password: 'password123' });
  r = await u.get(`/crm/items/${co.id}/everything`);
  ok('help desk and the books only for those who work there', r.status === 200 && !r.data.items.some((e: any) => e.kind === 'ticket' || e.kind === 'money') && !r.data.kinds.some((k: any) => k.kind === 'ticket'), [r.status, r.data.kinds?.map((k: any) => k.kind)]);
  // With employees seeing only their own deals, Ed still sees the company and Ann's call,
  // but not what happened on a deal that isn't his.
  const empRole = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
  const perms = (await admin.get('/crm/permissions')).data;
  original = Object.entries(perms.matrix[empRole.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
  roleId = empRole.id;
  await admin.put(`/crm/permissions/${roleId}`, { entries: original.map((e: any) => (e.entity_type === 2 && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
  r = await u.get(`/crm/items/${co.id}/everything`);
  const seen = JSON.stringify(r.data.items);
  ok('a deal they may not see stays out of the history', r.status === 200 && seen.includes(`Called Ann ${s}`) && !seen.includes(`Spoke about the roof ${s}`) && !seen.includes(`Contract ${s}`) && !seen.includes(`Measure the roof ${s}`), r.data.items?.map((e: any) => e.title));
} finally {
  if (original) await admin.put(`/crm/permissions/${roleId}`, { entries: original }).catch(() => {});
  await db.query('DELETE FROM tasks WHERE id = $1', [made.task || null]).catch(() => {});
  await db.query('DELETE FROM support_tickets WHERE id = $1', [made.ticket || null]).catch(() => {});
  await db.query('DELETE FROM crm_emails WHERE message_id = $1', [`<e-${s}@x>`]).catch(() => {});
  await db.query("DELETE FROM bookings WHERE client_name = 'Ann' AND contact_id IN (SELECT id FROM crm_items WHERE last_name = $1)", [`E${s}`]).catch(() => {});
  await db.query('DELETE FROM mkt_campaigns WHERE id = $1', [made.campaign || null]).catch(() => {});
  await db.query('DELETE FROM track_sites WHERE id = $1', [made.site || null]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title = $1 OR last_name = $2 OR company_name = $3", [`Everything deal ${s}`, `E${s}`, `Everything Co ${s}`]).catch((e) => console.log('cleanup', e.message));
  await db.query('DELETE FROM crm_webforms WHERE id = $1', [made.form || null]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
