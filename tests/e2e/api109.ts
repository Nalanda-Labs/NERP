// Deal health and silent clients.
//
// Health: the pipeline's won deals spent 2 days in "Proposal" and had 2 contacts. A deal 6 days
// in Proposal with one contact, nothing happening for 20 days, its close date 3 days past, our
// email unanswered for 7 days and nothing planned scores 0 (poor), with each reason; a fresh,
// well-covered deal scores 100 (healthy).
//
// Silent clients: a company that ordered every 30 days but not for 80 days is silent (a
// check-in task for its responsible person, once); one that ordered 20 days ago isn't.
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
const before = (await admin.get('/crm/insights/settings')).data;
const ago = (days: number) => new Date(Date.now() - days * 86400e3).toISOString();
const day = (offset: number) => new Date(Date.now() + offset * 86400e3).toISOString().slice(0, 10);

try {
  const pipe = (await admin.post('/crm/deal/categories', { name: `Health ${s}` })).data.categories.find((c: any) => c.name === `Health ${s}`);
  const proposal = pipe.stages.find((x: any) => x.semantics === 'process' && x.sort > pipe.stages[0].sort) ?? pipe.stages[1];
  const won = pipe.stages.find((x: any) => x.semantics === 'success');
  const contact = async (name: string) => (await admin.post('/crm/contact/items', { first_name: name, last_name: `H${s}` })).data.id;
  const [c1, c2] = [await contact('Ann'), await contact('Bob')];

  // Won deals: 2 days in Proposal, 2 contacts each.
  for (let k = 0; k < 3; k++) {
    const d = (await admin.post('/crm/deal/items', { title: `Won ${k} ${s}`, category_id: pipe.id, stage_id: proposal.id, contact_ids: [c1, c2] })).data;
    await admin.patch(`/crm/deal/items/${d.id}`, { stage_id: won.id });
    await db.query('DELETE FROM crm_stage_history WHERE item_id = $1', [d.id]);
    await db.query("INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics, entered_at) VALUES ($1, $2, $3, 'process', $4), ($1, $2, $5, 'success', $6)",
      [d.id, pipe.id, proposal.id, ago(30 + k), won.id, ago(28 + k)]);
  }

  // A deal in trouble.
  const bad = (await admin.post('/crm/deal/items', { title: `Stalled ${s}`, category_id: pipe.id, stage_id: proposal.id, contact_ids: [c1], close_date: day(-3) })).data;
  await db.query('DELETE FROM crm_stage_history WHERE item_id = $1', [bad.id]);
  await db.query("INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics, entered_at) VALUES ($1, $2, $3, 'process', $4)", [bad.id, pipe.id, proposal.id, ago(6)]);
  await db.query('UPDATE crm_items SET last_activity_at = $2 WHERE id = $1', [bad.id, ago(20)]);
  const email = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, sent_at) VALUES ('out', 'sent', $1, gen_random_uuid(), $2) RETURNING id", [`<h-${s}@nerp>`, ago(7)])).rows[0].id;
  await db.query('INSERT INTO crm_email_links (email_id, item_id) VALUES ($1, $2)', [email, bad.id]);

  // A deal in good shape.
  const good = (await admin.post('/crm/deal/items', { title: `Fresh ${s}`, category_id: pipe.id, stage_id: proposal.id, contact_ids: [c1, c2], close_date: day(20) })).data;
  await admin.post('/crm/activities', { item_id: good.id, kind: 'todo', subject: 'Send the proposal', deadline: new Date(Date.now() + 2 * 86400e3).toISOString() });

  let r = await admin.get(`/crm/items/${bad.id}/health`);
  const kinds = (r.data.reasons ?? []).map((x: any) => x.kind).sort();
  ok('a deal in trouble scores 0 (poor)', r.status === 200 && r.data.score === 0 && r.data.grade === 'poor', r.data);
  ok('…with every reason', kinds.join(',') === 'contact,dates,next_step,pace,people,replies', kinds);
  const text = (k: string) => r.data.reasons.find((x: any) => x.kind === k)?.text ?? '';
  ok('…compared with won deals', /won deals usually move on in 2/.test(text('pace')) && /won deals usually have 2/.test(text('people')) && /unanswered/.test(text('replies')), r.data.reasons);
  r = await admin.get(`/crm/items/${good.id}/health`);
  ok('a deal in good shape scores 100 (healthy)', r.data.score === 100 && r.data.grade === 'healthy' && r.data.reasons.length === 0, r.data);
  r = await admin.get(`/crm/deal/health?category_id=${pipe.id}`);
  ok('the pipeline\'s open deals, least healthy first', r.status === 200 && r.data.items[0].id === bad.id && r.data.items.at(-1).id === good.id && r.data.items.length === 2 && r.data.counts.poor === 1, r.data.items?.map((x: any) => [x.title, x.score]));
  ok('…by grade', (await admin.get(`/crm/deal/health?category_id=${pipe.id}&grade=healthy`)).data.items.map((x: any) => x.id).join() === good.id);
  const wonOne = (await db.query('SELECT id FROM crm_items WHERE title = $1', [`Won 0 ${s}`])).rows[0].id;
  ok('won deals aren\'t judged', (await admin.get(`/crm/items/${wonOne}/health`)).data.score === null);
  ok('only deals have health', (await admin.get(`/crm/items/${c1}/health`)).status === 400);

  // ---- next steps that worked: the won deals held a meeting a day into Proposal; two lost
  // deals passed through it without one ----
  const lost = pipe.stages.find((x: any) => x.semantics === 'failure');
  const wonIds = (await db.query('SELECT id FROM crm_items WHERE title LIKE $1', [`Won % ${s}`])).rows.map((x) => x.id);
  for (const [k, id] of wonIds.entries()) {
    await db.query("INSERT INTO crm_activities (item_id, kind, subject, responsible_id, created_by, is_completed, created_at) VALUES ($1, 'meeting', 'Demo', $2, $2, TRUE, $3)", [id, me.id, ago(29 + k)]);
  }
  for (let k = 0; k < 2; k++) {
    const d = (await admin.post('/crm/deal/items', { title: `Lost ${k} ${s}`, category_id: pipe.id, stage_id: proposal.id })).data;
    await admin.patch(`/crm/deal/items/${d.id}`, { stage_id: lost.id });
    await db.query('DELETE FROM crm_stage_history WHERE item_id = $1', [d.id]);
    await db.query("INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics, entered_at) VALUES ($1, $2, $3, 'process', $4), ($1, $2, $5, 'failure', $6)",
      [d.id, pipe.id, proposal.id, ago(25), lost.id, ago(23)]);
  }
  r = await admin.get(`/crm/items/${bad.id}/next-steps`);
  const meet = r.data.items?.find((x: any) => x.step === 'meeting');
  ok('next steps that worked: a meeting (won 100% with it, 0% without), a day in', r.status === 200 && meet && meet.won_with === 100 && meet.won_without === 0 && meet.deals_with === 3 && meet.usual_day === 1 && meet.done === false && r.data.based_on === 5, r.data);
  ok('…steps that didn\'t make a difference aren\'t suggested', !r.data.items.some((x: any) => x.step === 'call'));
  await admin.post('/crm/activities', { item_id: bad.id, kind: 'meeting', subject: 'Meet', starts_at: new Date(Date.now() + 86400e3).toISOString() });
  ok('…and once taken, it shows as done', (await admin.get(`/crm/items/${bad.id}/next-steps`)).data.items.find((x: any) => x.step === 'meeting')?.done === true);

  // ---- silent clients ----
  const company = async (name: string) => (await admin.post('/crm/company/items', { company_name: `${name} ${s}`, assigned_to: me.id })).data.id;
  const quiet = await company('Quiet'), busy = await company('Busy');
  const orders = async (client: string, daysAgo: number[]) => {
    for (const d of daysAgo) {
      const x = (await admin.post('/crm/deal/items', { title: `Order ${d} ${s}`, category_id: pipe.id, stage_id: proposal.id, company_id: client })).data;
      await admin.patch(`/crm/deal/items/${x.id}`, { stage_id: won.id });
      await db.query("UPDATE crm_stage_history SET entered_at = $2 WHERE item_id = $1 AND semantics = 'success'", [x.id, ago(d)]);
    }
  };
  await orders(quiet, [170, 140, 110, 80]);
  await orders(busy, [80, 50, 20]);
  r = await admin.get('/crm/silent-clients');
  const q = r.data.items?.find((x: any) => x.id === quiet);
  ok('a client who stopped ordering', q && q.usual_gap_days === 30 && q.days_since === 80 && q.orders === 4 && q.link === `/crm/company/${quiet}`, q);
  ok('…one still in rhythm isn\'t', !r.data.items.some((x: any) => x.id === busy));
  ok('turning check-in tasks on is for CRM administrators', (await (async () => { const c = new Client(); await admin.post('/users', { email: `nina.${s}@acme.test`, password: 'password123', first_name: 'Nina', last_name: `H${s}` }); await c.post('/auth/login', { email: `nina.${s}@acme.test`, password: 'password123' }); return c.put('/crm/insights/settings', { silent_tasks: true }); })()).status === 403);
  await admin.put('/crm/insights/settings', { silent_tasks: true });
  const runJob = () => db.query(`INSERT INTO jobs (kind, payload) VALUES ('crm.silent_clients', '{}')`);
  await runJob();
  const task = await (async () => { for (let i = 0; i < 40; i++) { const t = (await db.query('SELECT t.id, t.title, t.responsible_id FROM tasks t JOIN task_crm_links l ON l.task_id = t.id WHERE l.item_id = $1', [quiet])).rows; if (t.length) return t; await new Promise((res) => setTimeout(res, 500)); } return []; })();
  ok('a check-in task for its responsible person, linked to it', task.length === 1 && task[0].title === `Check in with Quiet ${s}` && task[0].responsible_id === me.id, task);
  await runJob();
  await new Promise((res) => setTimeout(res, 3000));
  ok('…once', (await db.query('SELECT COUNT(*)::int AS n FROM task_crm_links WHERE item_id = $1', [quiet])).rows[0].n === 1);
  ok('the list shows the task', (await admin.get('/crm/silent-clients')).data.items.find((x: any) => x.id === quiet)?.task_id === task[0]?.id);
} finally {
  await admin.put('/crm/insights/settings', { silent_tasks: before.silent_tasks ?? false }).catch(() => {});
  await db.query("DELETE FROM tasks WHERE title LIKE $1", [`Check in with % ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_emails WHERE message_id = $1", [`<h-${s}@nerp>`]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title LIKE $1 OR last_name LIKE $1 OR company_name LIKE $1", [`%${s}`]).catch((e) => console.log('cleanup:', e.message));
  await db.query("DELETE FROM crm_categories WHERE name = $1", [`Health ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
