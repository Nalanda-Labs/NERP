// Proactive CRM: each person's open deals and leads that need attention, with why — an overdue
// activity, nothing planned, a client's email waiting, a close date passed, a stage's time limit
// exceeded — and the morning summary (notification, and email if set).
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
const before = (await admin.get('/crm/insights/settings')).data;
const hoursAgo = (h: number) => new Date(Date.now() - h * 3600e3).toISOString();
const day = (n: number) => new Date(Date.now() + n * 86400e3).toISOString().slice(0, 10);

try {
  await admin.post('/users', { email: `jo.${s}@acme.test`, password: 'password123', first_name: 'Jo', last_name: `A${s}` });
  const jo = new Client();
  const joUser = (await jo.post('/auth/login', { email: `jo.${s}@acme.test`, password: 'password123' })).data.user;
  await admin.put('/crm/insights/settings', { reply_hours: 24, attention_digest: true, attention_email: false });
  const pipe = (await admin.post('/crm/deal/categories', { name: `Attention ${s}` })).data.categories.find((c: any) => c.name === `Attention ${s}`);
  const stage = pipe.stages.find((x: any) => x.semantics === 'process');
  const deal = async (title: string, extra: any = {}) => (await admin.post('/crm/deal/items', { title: `${title} ${s}`, category_id: pipe.id, stage_id: stage.id, assigned_to: joUser.id, ...extra })).data;
  const plan = (item: string, deadlineHours: number) => db.query("INSERT INTO crm_activities (item_id, kind, subject, responsible_id, created_by, deadline) VALUES ($1, 'call', 'Call', $2, $2, $3)", [item, joUser.id, hoursAgo(-deadlineHours)]);
  const overdue = await deal('Overdue'); await plan(overdue.id, -5);
  const idle = await deal('Idle');
  const waiting = await deal('Waiting'); await plan(waiting.id, 24);
  const mail = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, sent_at) VALUES ('in', 'received', $1, gen_random_uuid(), 'Any news?', 'c@example.com', $2) RETURNING id", [`<at-${s}@x>`, hoursAgo(30)])).rows[0].id;
  await db.query('INSERT INTO crm_email_links (email_id, item_id) VALUES ($1, $2)', [mail, waiting.id]);
  const late = await deal('Late', { close_date: day(-3) }); await plan(late.id, 24);
  const slow = await deal('Slow'); await plan(slow.id, 24);
  await db.query('UPDATE crm_stages SET sla_minutes = 60 WHERE id = $1', [stage.id]);
  await db.query('UPDATE crm_items SET stage_changed_at = $2 WHERE id = $1', [slow.id, hoursAgo(3)]);
  await db.query('UPDATE crm_items SET stage_changed_at = now() WHERE id = ANY($1)', [[overdue.id, idle.id, waiting.id, late.id]]);
  const fine = await deal('Fine'); await plan(fine.id, 24);
  await db.query('UPDATE crm_items SET stage_changed_at = now() WHERE id = $1', [fine.id]);
  const lead = (await admin.post('/crm/lead/items', { title: `Lead ${s}`, assigned_to: joUser.id })).data;

  let r = await jo.get('/crm/attention');
  const by = (t: string) => r.data.items.find((x: any) => x.title === t);
  const kinds = (t: string) => (by(t)?.reasons ?? []).map((x: any) => x.kind).join();
  ok('an overdue activity', kinds(`Overdue ${s}`) === 'overdue', by(`Overdue ${s}`));
  ok('nothing planned', kinds(`Idle ${s}`) === 'nothing_planned');
  ok('a client\'s email waiting over a day', kinds(`Waiting ${s}`) === 'unanswered' && /30 hours/.test(by(`Waiting ${s}`).reasons[0].text), by(`Waiting ${s}`));
  ok('a close date passed', kinds(`Late ${s}`) === 'close_date' && /3 days ago/.test(by(`Late ${s}`).reasons[0].text));
  ok('over the stage\'s time limit', kinds(`Slow ${s}`) === 'stage_time' && /hours over/.test(by(`Slow ${s}`).reasons[0].text), by(`Slow ${s}`));
  ok('leads too', kinds(`Lead ${s}`) === 'nothing_planned');
  ok('not the deal that is fine', !by(`Fine ${s}`));
  ok('the most pressing first', r.data.items[0].title === `Waiting ${s}`, r.data.items.map((x: any) => x.title));
  ok('counts per reason', r.data.counts.nothing_planned === 2 && r.data.counts.unanswered === 1, r.data.counts);
  await admin.put('/crm/insights/settings', { reply_hours: 48 });
  ok('the waiting time is a setting', !(await jo.get('/crm/attention')).data.items.some((x: any) => x.title === `Waiting ${s}`));
  await admin.put('/crm/insights/settings', { reply_hours: 24 });

  // ---- whose list, and the morning summary ----
  const me = (await admin.get('/auth/me')).data;
  ok('one\'s own list; others\' for heads and CRM administrators', (await jo.get(`/crm/attention?user=${(me.user ?? me).id}`)).status === 403 && (await admin.get(`/crm/attention?user=${joUser.id}`)).data.items.length === 6);
  ok('summaries are sent by CRM administrators', (await jo.post('/crm/attention/send-now', {})).status === 403);
  await admin.put('/crm/insights/settings', { attention_email: true });
  r = await admin.post('/crm/attention/send-now', {});
  const note = (await db.query("SELECT title, body, link FROM notifications WHERE user_id = $1 AND kind = 'crm.attention' ORDER BY created_at DESC LIMIT 1", [joUser.id])).rows[0];
  ok('a notification sums it up', r.status === 200 && note?.title === '6 deals and leads need attention' && /1 waiting for your answer/.test(note.body) && note.link === '/crm/insights?tab=attention', note);
  const em = (await db.query("SELECT payload FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1 ORDER BY created_at DESC LIMIT 1", [`jo.${s}@acme.test`])).rows[0]?.payload;
  ok('…and an email, when set', em && /need attention/.test(em.subject) && em.text.includes(`Waiting ${s}`), em?.subject);
  ok('settings are for CRM administrators', (await jo.put('/crm/insights/settings', { attention_digest: false })).status === 403);
  ok('the waiting time is checked', (await admin.put('/crm/insights/settings', { reply_hours: 0 })).status === 400);
} finally {
  await admin.put('/crm/insights/settings', { reply_hours: before.reply_hours ?? 24, attention_digest: before.attention_digest ?? true, attention_email: before.attention_email ?? false }).catch(() => {});
  await db.query("DELETE FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [`jo.${s}@acme.test`]).catch(() => {});
  await db.query('DELETE FROM crm_emails WHERE message_id = $1', [`<at-${s}@x>`]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title LIKE $1", [`% ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_categories WHERE name = $1", [`Attention ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
