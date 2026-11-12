// Personal data requests: finding a person across modules, the report, reviewed erasure, and
// the public form confirmed by email.
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
const email = `dora.${s}@example.com`;
const phone = `+44 20 7${String(Date.now()).slice(-7)}`;
const made: Record<string, string> = {};

try {
  // Dora, in many modules.
  const dora = (await admin.post('/crm/contact/items', { first_name: 'Dora', last_name: `D${s}`, multifields: [{ kind: 'email', value_type: 'work', value: email }, { kind: 'phone', value_type: 'mobile', value: phone }] })).data;
  made.dora = dora.id;
  const deal = (await admin.post('/crm/deal/items', { title: `Dora's roof ${s}`, contact_ids: [dora.id] })).data;
  const fd = new FormData(); fd.append('file', new Blob([`passport scan ${s}`], { type: 'text/plain' }), `passport ${s}.txt`);
  const fileId = (await admin.req('POST', '/files', fd) as any).data.id;
  await db.query("INSERT INTO crm_timeline (item_id, kind, author_id, body, file_ids) VALUES ($1, 'comment', $2, $3, $4)", [dora.id, me.id, `Dora prefers mornings ${s}`, [fileId]]);
  const mail = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, sent_at) VALUES ('in', 'received', $1, gen_random_uuid(), $2, $3, now()) RETURNING id", [`<d-${s}@x>`, `About my roof ${s}`, email.toUpperCase()])).rows[0].id;
  await db.query('INSERT INTO crm_email_links (email_id, item_id) VALUES ($1, $2)', [mail, dora.id]);
  made.channel = (await db.query("INSERT INTO chat_channels (name, key) VALUES ($1, $2) RETURNING id", [`Chan ${s}`, `c${s}`])).rows[0].id;
  const chat = (await db.query("INSERT INTO chat_sessions (channel_id, token_hash, visitor_name, visitor_email) VALUES ($1, $2, 'Dora', $3) RETURNING id", [made.channel, `h${s}`, email])).rows[0].id;
  await db.query("INSERT INTO chat_messages (session_id, author, body) VALUES ($1, 'visitor', $2)", [chat, `Hello from Dora ${s}`]);
  const ticket = (await db.query("INSERT INTO support_tickets (title, channel, requester_name, requester_email) VALUES ($1, 'email', 'Dora', $2) RETURNING id", [`Leak ${s}`, email])).rows[0].id;
  made.ticket = ticket;
  await db.query("INSERT INTO support_messages (ticket_id, kind, by_support, body, author_email) VALUES ($1, 'message', FALSE, $2, $3)", [ticket, `It leaks ${s}`, email]);
  const booking = (await db.query("INSERT INTO bookings (starts_at, ends_at, client_name, client_phone) VALUES (now() + interval '2 days', now() + interval '2 days 1 hour', 'Dora', $1) RETURNING id", [phone.replace(/\s/g, '')])).rows[0].id;
  made.booking = booking;
  await db.query("INSERT INTO shop_orders (item_id, token, name, email, address) VALUES ($1, $2, 'Dora', $3, '1 Roof Lane')", [deal.id, `o${s}`, email]);
  made.campaign = (await db.query("INSERT INTO mkt_campaigns (name, slug) VALUES ($1, $2) RETURNING id", [`News ${s}`, `news-${s}`])).rows[0].id;
  await db.query("INSERT INTO mkt_recipients (campaign_id, email, token, status, sent_at) VALUES ($1, $2, $3, 'sent', now())", [made.campaign, email, `t${s}`]);
  made.form = (await db.query("INSERT INTO crm_webforms (key, name, entity_type) VALUES ($1, $2, 3) RETURNING id", [`f${s}`, `Quote ${s}`])).rows[0].id;
  await db.query("INSERT INTO crm_webform_submissions (form_id, data) VALUES ($1, $2)", [made.form, { email, message: 'roof' }]);
  made.event = (await db.query("INSERT INTO cal_events (owner_id, created_by, title, starts_at, ends_at, timezone) VALUES ($1, $1, $2, now() + interval '1 day', now() + interval '1 day 1 hour', 'UTC') RETURNING id", [me.id, `Site visit ${s}`])).rows[0].id;
  await db.query("INSERT INTO cal_email_guests (event_id, email, name) VALUES ($1, $2, 'Dora')", [made.event, email]);
  const sreq = (await db.query("INSERT INTO sign_requests (item_id, title, original_sha256, status) VALUES ($1, $2, 'x', 'completed') RETURNING id", [deal.id, `Contract ${s}`])).rows[0].id;
  await db.query("INSERT INTO sign_signers (request_id, name, email, token_hash, status, signed_at) VALUES ($1, 'Dora', $2, $3, 'signed', now())", [sreq, email, `sh${s}`]);
  // Someone else entirely.
  const olga = (await admin.post('/crm/contact/items', { first_name: 'Olga', last_name: `D${s}`, multifields: [{ kind: 'email', value_type: 'work', value: `olga.${s}@example.com` }] })).data;

  // ---- access ----
  let r = await admin.post('/privacy/requests', { kind: 'access', email, phone, name: 'Dora' });
  ok('a request by staff is open and due in 30 days', r.status === 200 && r.data.status === 'open' && Math.round((new Date(r.data.due_at).getTime() - Date.now()) / 86400e3) === 30, r.data);
  const access = r.data.id;
  r = await admin.get(`/privacy/requests/${access}`);
  const count = (k: string) => r.data.sources.find((x: any) => x.key === k)?.count;
  ok('found in every module', ['crm', 'emails', 'chats', 'tickets', 'bookings', 'orders', 'marketing', 'forms', 'calendar', 'signatures'].every((k) => count(k) === 1), r.data.sources.map((x: any) => [x.key, x.count]));
  ok('…the booking by phone alone, the email regardless of case', count('bookings') === 1 && count('emails') === 1);
  ok('…and nobody else', !JSON.stringify(r.data).includes(`olga.${s}`));
  const rep = await admin.req('GET', `/privacy/requests/${access}/report`, undefined, true) as Response;
  const body = await rep.json();
  const txt = JSON.stringify(body);
  ok('the report has the data itself', rep.status === 200 && /attachment/.test(rep.headers.get('content-disposition') ?? '') && txt.includes(`Dora prefers mornings ${s}`) && txt.includes(`Hello from Dora ${s}`) && txt.includes(`It leaks ${s}`) && txt.includes(phone) && body.about.email === email, Object.keys(body));
  ok('…only the places that hold something', body.sources.every((x: any) => x.count > 0));
  r = await admin.patch(`/privacy/requests/${access}`, { status: 'done', note: 'Report emailed' });
  ok('answered', r.data.status === 'done' && r.data.decided_by === me.id);

  // ---- erasure ----
  const er = (await admin.post('/privacy/requests', { kind: 'erasure', email, phone })).data.id;
  ok('an erasure request can\'t just be marked done', (await admin.patch(`/privacy/requests/${er}`, { status: 'done' })).status === 400);
  ok('erasing needs the email typed again', (await admin.post(`/privacy/requests/${er}/erase`, { actions: {}, confirm: 'nope' })).status === 400);
  ok('…and only offered actions', (await admin.post(`/privacy/requests/${er}/erase`, { actions: { books: 'delete' }, confirm: email })).status === 400);
  r = await admin.post(`/privacy/requests/${er}/erase`, { actions: { tickets: 'anonymise' }, confirm: email, note: 'Signed contract kept as evidence' });
  ok('erased as chosen', r.status === 200 && r.data.status === 'done' && r.data.outcome?.crm?.count === 1 && r.data.outcome?.emails?.count === 1 && r.data.outcome?.tickets?.action === 'anonymise' && r.data.outcome?.signatures?.action === 'keep', r.data);
  const one = async (sql: string, p: unknown[]) => (await db.query(sql, p)).rows[0];
  const c = await one('SELECT first_name, last_name, deleted_at, (SELECT COUNT(*)::int FROM crm_multifields WHERE item_id = $1) AS mf, (SELECT COUNT(*)::int FROM crm_timeline WHERE item_id = $1 AND kind = \'comment\') AS notes FROM crm_items WHERE id = $1', [dora.id]);
  ok('her CRM record is anonymised and out of the lists', c.first_name === 'Erased' && c.last_name === null && c.deleted_at && c.mf === 0 && c.notes === 0, c);
  ok('…the file on a note is gone', (await db.query('SELECT 1 FROM files WHERE id = $1', [fileId])).rowCount === 0);
  ok('…the deal stays', (await db.query('SELECT 1 FROM crm_items WHERE id = $1 AND deleted_at IS NULL', [deal.id])).rowCount === 1);
  ok('emails, chats, marketing, forms and invitations deleted',
    (await db.query('SELECT 1 FROM crm_emails WHERE id = $1', [mail])).rowCount === 0 && (await db.query('SELECT 1 FROM chat_sessions WHERE id = $1', [chat])).rowCount === 0
    && (await db.query('SELECT 1 FROM mkt_recipients WHERE campaign_id = $1', [made.campaign])).rowCount === 0 && (await db.query('SELECT 1 FROM crm_webform_submissions WHERE form_id = $1', [made.form])).rowCount === 0
    && (await db.query('SELECT 1 FROM cal_email_guests WHERE event_id = $1', [made.event])).rowCount === 0);
  const t = await one("SELECT t.requester_email, t.title, (SELECT body FROM support_messages WHERE ticket_id = t.id LIMIT 1) AS body FROM support_tickets t WHERE t.id = $1", [ticket]);
  ok('the ticket is kept without her', t && t.requester_email === null && t.title === 'Erased' && t.body === 'Erased', t);
  const b = await one('SELECT client_name, client_phone FROM bookings WHERE id = $1', [booking]);
  ok('the booking keeps its time without her', b.client_name === 'Erased' && b.client_phone === null, b);
  const o = await one('SELECT name, email, address FROM shop_orders WHERE item_id = $1', [deal.id]);
  ok('the order keeps its amounts without her', o.name === 'Erased' && o.email === 'erased@invalid' && o.address === '', o);
  ok('the signature is kept, as chosen', (await one('SELECT email FROM sign_signers WHERE request_id = $1', [sreq])).email === email);
  ok('she is on the opt-out list', (await db.query('SELECT 1 FROM mkt_optouts WHERE lower(email) = $1', [email])).rowCount === 1);
  ok('Olga is untouched', (await one('SELECT first_name FROM crm_items WHERE id = $1', [olga.id])).first_name === 'Olga');
  ok('afterwards there is no report of erased data', (await admin.req('GET', `/privacy/requests/${er}/report`, undefined, true) as Response).status === 400);
  ok('…and it can\'t be run twice', (await admin.post(`/privacy/requests/${er}/erase`, { actions: {}, confirm: email })).status === 400);
  ok('erasing is in the audit log', (await db.query("SELECT 1 FROM audit_log WHERE action = 'privacy.erase' AND entity_id = $1", [er])).rowCount === 1);

  // ---- the public form ----
  const anon = new Client();
  const pub = `pat.${s}@example.com`;
  r = await anon.post('/public/privacy-request', { kind: 'erasure', email: pub, name: 'Pat' });
  ok('anyone can ask', r.status === 200 && r.data.ok === true);
  const req = (await db.query("SELECT id, status FROM privacy_requests WHERE email = $1", [pub])).rows[0];
  ok('…it waits for confirmation', req?.status === 'unverified');
  const job = (await db.query("SELECT payload FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1 ORDER BY created_at DESC LIMIT 1", [pub])).rows[0]?.payload;
  const token = /privacy-request\/confirm\?token=([A-Za-z0-9_-]+)/.exec(job?.text ?? '')?.[1];
  ok('…with a confirmation email', !!token, job?.text);
  ok('staff can\'t act on it before', (await admin.post(`/privacy/requests/${req.id}/erase`, { actions: {}, confirm: pub })).status === 400);
  r = await anon.post('/public/privacy-request/verify', { token });
  ok('confirmed from the email', r.status === 200 && r.data.kind === 'erasure');
  const v = (await db.query('SELECT status, due_at FROM privacy_requests WHERE id = $1', [req.id])).rows[0];
  ok('…now open and due', v.status === 'open' && v.due_at);
  ok('…the link works once', (await anon.post('/public/privacy-request/verify', { token })).status === 404);
  ok('those who handle requests are told', (await db.query("SELECT 1 FROM notifications n JOIN users u ON u.id = n.user_id WHERE u.email = 'admin@acme.test' AND n.kind = 'privacy.request' AND n.link = $1", [`/privacy/${req.id}`])).rowCount === 1);
  for (let i = 0; i < 2; i++) await anon.post('/public/privacy-request', { kind: 'access', email: pub });
  ok('the same address can\'t ask endlessly', (await anon.post('/public/privacy-request', { kind: 'access', email: pub })).status === 429);
  ok('a wrong token tells nothing', (await anon.post('/public/privacy-request/verify', { token: 'nope' })).status === 404);

  const u = new Client();
  await admin.post('/users', { email: `kim.${s}@acme.test`, password: 'password123', first_name: 'Kim', last_name: `D${s}` });
  await u.post('/auth/login', { email: `kim.${s}@acme.test`, password: 'password123' });
  ok('requests are for those who handle them', (await u.get('/privacy/requests')).status === 403 && (await u.get(`/privacy/requests/${access}/report`)).status === 403);
} finally {
  await db.query('DELETE FROM privacy_requests WHERE email IN ($1, $2)', [email, `pat.${s}@example.com`]).catch(() => {});
  await db.query('DELETE FROM mkt_optouts WHERE lower(email) = $1', [email]).catch(() => {});
  await db.query("DELETE FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [`pat.${s}@example.com`]).catch(() => {});
  await db.query("DELETE FROM sign_requests WHERE title = $1", [`Contract ${s}`]).catch(() => {});
  if (made.ticket) await db.query('DELETE FROM support_tickets WHERE id = $1', [made.ticket]).catch(() => {});
  if (made.booking) await db.query('DELETE FROM bookings WHERE id = $1', [made.booking]).catch(() => {});
  await db.query('DELETE FROM shop_orders WHERE token = $1', [`o${s}`]).catch(() => {});
  if (made.channel) await db.query('DELETE FROM chat_channels WHERE id = $1', [made.channel]).catch(() => {});
  if (made.campaign) await db.query('DELETE FROM mkt_campaigns WHERE id = $1', [made.campaign]).catch(() => {});
  if (made.event) await db.query('DELETE FROM cal_events WHERE id = $1', [made.event]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE last_name = $1 OR title = $2 OR id = $3", [`D${s}`, `Dora's roof ${s}`, made.dora ?? null]).catch((e) => console.log('cleanup', e.message));
  if (made.form) await db.query('DELETE FROM crm_webforms WHERE id = $1', [made.form]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
