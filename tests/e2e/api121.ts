// Automation preview: how often a robot or trigger would have run in the past days, with its
// conditions (checked on the records as they are now) and its event settings.
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
const ago = (days: number) => new Date(Date.now() - days * 86400e3).toISOString();

try {
  const pipe = (await admin.post('/crm/deal/categories', { name: `Preview ${s}` })).data.categories.find((c: any) => c.name === `Preview ${s}`);
  const proposal = pipe.stages.find((x: any) => x.semantics === 'process' && x.sort > pipe.stages[0].sort) ?? pipe.stages[1];
  const deal = async (title: string, amount: string, enteredDaysAgo: number) => {
    const d = (await admin.post('/crm/deal/items', { title: `${title} ${s}`, category_id: pipe.id, stage_id: proposal.id, opportunity: amount, is_manual_amount: true })).data;
    await db.query('DELETE FROM crm_stage_history WHERE item_id = $1', [d.id]);
    await db.query("INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics, entered_at) VALUES ($1, $2, $3, 'process', $4)", [d.id, pipe.id, proposal.id, ago(enteredDaysAgo)]);
    return d;
  };
  const [d1, d2, d3, d4] = [await deal('Small', '100', 5), await deal('Medium', '300', 20), await deal('Old', '200', 50), await deal('Big', '5000', 10)];
  const run = (b: any) => admin.post('/crm/automation/preview', { category_id: pipe.id, ...b });

  let r = await run({ kind: 'robot', stage_id: proposal.id, days: 30 });
  ok('a robot: every entry into its stage in the period', r.status === 200 && r.data.would_run === 3 && r.data.records === 3 && r.data.considered === 3, r.data);
  ok('…with examples and weeks', r.data.sample.length === 3 && r.data.sample.every((x: any) => /^\/crm\/deal\//.test(x.link)) && r.data.per_week.reduce((a: number, b: number) => a + b, 0) === 3, r.data);
  r = await run({ kind: 'robot', stage_id: proposal.id, days: 90 });
  ok('a longer period counts the older one too', r.data.would_run === 4, r.data);
  r = await run({ kind: 'robot', stage_id: proposal.id, days: 30, conditions: [{ field: 'opportunity', op: 'gte', value: '1000' }] });
  ok('conditions leave the others out', r.data.would_run === 1 && r.data.sample[0].id === d4.id && r.data.filtered_out === 2 && /as they are now/.test(r.data.note), r.data);

  // Triggers: two calls and a meeting done on the pipeline's deals, one call long ago.
  const act = (item: string, kind: string, daysAgo: number) =>
    db.query("INSERT INTO crm_activities (item_id, kind, subject, responsible_id, created_by, is_completed, completed_at) VALUES ($1, $2, $3, $4, $4, TRUE, $5)", [item, kind, `${kind} ${s}`, me.id, ago(daysAgo)]);
  await act(d1.id, 'call', 2); await act(d2.id, 'call', 3); await act(d4.id, 'meeting', 4); await act(d3.id, 'call', 60);
  r = await run({ kind: 'trigger', event: 'activity_done', params: { kind: 'call' }, days: 30 });
  ok('a trigger: its events in the period, with its settings', r.status === 200 && r.data.would_run === 2 && r.data.records === 2, r.data);
  r = await run({ kind: 'trigger', event: 'activity_done', params: {}, days: 30 });
  ok('…without the setting, every activity done', r.data.would_run === 3, r.data);
  await db.query("INSERT INTO crm_timeline (item_id, kind, author_id, body, created_at) VALUES ($1, 'comment', $2, 'x', $3)", [d2.id, me.id, ago(1)]);
  r = await run({ kind: 'trigger', event: 'comment_added', days: 30 });
  ok('comments count', r.data.would_run >= 1 && r.data.sample.some((x: any) => x.id === d2.id), r.data);
  r = await run({ kind: 'trigger', event: 'webhook' });
  ok('webhooks aren\'t recorded, and it says so', r.data.unknown === true);
  ok('other pipelines\' records don\'t count', (await admin.post('/crm/automation/preview', { category_id: (await admin.get('/crm/config')).data.types.find((t: any) => t.code === 'deal').categories.find((c: any) => c.is_default).id, kind: 'robot', stage_id: proposal.id })).data.would_run === 0);
  ok('kind is robot or trigger', (await run({ kind: 'nope' })).status === 400);

  const u = new Client();
  await admin.post('/users', { email: `pv.${s}@acme.test`, password: 'password123', first_name: 'Pv', last_name: `A${s}` });
  await u.post('/auth/login', { email: `pv.${s}@acme.test`, password: 'password123' });
  ok('previews are for those who edit automation', (await u.post('/crm/automation/preview', { category_id: pipe.id, kind: 'robot', stage_id: proposal.id })).status === 403);
} finally {
  await db.query("DELETE FROM crm_items WHERE title LIKE $1", [`% ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_categories WHERE name = $1", [`Preview ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
