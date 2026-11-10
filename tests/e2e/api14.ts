import { execSync } from 'node:child_process';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 500)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'Crm' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const sam = await mk("sam"), ivy = await mk("ivy");
// CRM analytics: funnel, figures, managers, sources, targets, permissions.
const cfg = (await admin.get('/crm/config')).data;
const base = cfg.base_currency;
let r: any = await admin.post('/crm/deal/categories', { name: `Analytics ${stamp}` });
const cat = r.data.categories.find((c) => c.name === `Analytics ${stamp}`);
const open = cat.stages.filter((s) => s.semantics === 'process');
const won = cat.stages.find((s) => s.semantics === 'success');
const lost = cat.stages.find((s) => s.semantics === 'failure');
ok('pipeline has open, won and lost stages', open.length >= 3 && won && lost, cat.stages.map((s) => s.name));
const deal = async (who, amount, stages) => {
  const d = (await who.post('/crm/deal/items', { title: `A ${stamp} ${amount}`, category_id: cat.id, currency: base, opportunity: String(amount), assigned_to: who.id, source: amount === 300 ? 'web_form' : 'website' })).data;
  for (const s of stages) { const x = await who.patch(`/crm/deal/items/${d.id}`, { stage_id: s.id }); if (x.status !== 200) console.log('move failed', x.data); }
  return d.id;
};
const ids: any[] = [];
ids.push(await deal(sam, 1000, [open[1], open[2], won]));
ids.push(await deal(sam, 500, [open[1], lost]));
ids.push(await deal(sam, 200, []));
ids.push(await deal(ivy, 300, [won]));

const q = (extra = '') => admin.get(`/crm/analytics/deal?category_id=${cat.id}${extra}`);
r = await q();
ok('report', r.status === 200 && r.data.currency === base && r.data.period.bucket === 'day', r.data);
const k = r.data.kpi.current;
ok('won count and amount', k.won_count === 2 && +k.won_amount === 1300, k);
ok('lost and new', k.lost_count === 1 && k.new_count === 4 && +k.new_amount === 2000, k);
ok('open now', r.data.kpi.open_count === 1 && +r.data.kpi.open_amount === 200, r.data.kpi);
ok('cycle time measured', typeof k.avg_cycle_days === 'number' && k.avg_cycle_days >= 0 && k.avg_cycle_days < 1, k);
ok('previous period empty', r.data.kpi.previous.won_count === 0 && r.data.kpi.previous.new_count === 0);
const f = Object.fromEntries(r.data.funnel.map((s) => [s.stage_id, s]));
ok('funnel: everyone entered the first stage', f[open[0].id].reached === 4, r.data.funnel);
ok('funnel: 3 reached the second (won deal skipping stages counts)', f[open[1].id].reached === 3, r.data.funnel.map((s) => [s.name, s.reached]));
ok('funnel: 2 reached the third and later open stages', open.slice(2).every((s) => f[s.id].reached === 2), r.data.funnel.map((s) => [s.name, s.reached]));
ok('funnel: won 2, lost 1, current counts', f[won.id].reached === 2 && f[lost.id].reached === 1 && f[open[0].id].current === 1 && +f[won.id].reached_amount === 1300, r.data.funnel);
ok('funnel order: open stages, then won, then lost', r.data.funnel.at(-1).semantics === 'failure' && r.data.funnel[open.length].semantics === 'success');
const sum = (k) => r.data.dynamics.reduce((a, p) => a + Number(p[k]), 0);
ok('dynamics add up', sum('new_count') === 4 && sum('won_count') === 2 && sum('won_amount') === 1300, r.data.dynamics.filter((p) => p.new_count));
ok('dynamics cover each day of the period', r.data.dynamics.length === r.data.period.days);
const m = Object.fromEntries(r.data.managers.map((x) => [x.user_id, x]));
ok('managers: sam', m[sam.id]?.won_count === 1 && +m[sam.id].won_amount === 1000 && m[sam.id].lost_count === 1 && m[sam.id].open_count === 1 && m[sam.id].new_count === 3, m[sam.id]);
ok('managers: ivy', m[ivy.id]?.won_count === 1 && +m[ivy.id].won_amount === 300, m[ivy.id]);
ok('managers ranked by won amount', r.data.managers[0].user_id === sam.id);
const src = Object.fromEntries(r.data.sources.map((s) => [s.source, s]));
ok('sources', src.website?.created === 3 && src.website.won_count === 1 && src.web_form?.created === 1 && +src.web_form.won_amount === 300, r.data.sources);
ok('lead conversion offered on deals', r.data.lead_conversion && typeof r.data.lead_conversion.created === 'number');
ok('admin can edit targets', r.data.can_edit_targets === true);

r = await q(`&users=${sam.id}`);
ok('manager filter', r.data.kpi.current.won_count === 1 && r.data.managers.length === 1, r.data.kpi.current);
r = await q('&from=2020-01-01&to=2020-01-31');
ok('other period is empty, funnel still lists stages', r.data.kpi.current.new_count === 0 && r.data.funnel.length === cat.stages.length && r.data.funnel.every((s) => s.reached === 0));
r = await admin.get('/crm/analytics/deal?from=2026-01-01&to=2026-12-31');
ok('a year is bucketed by month', r.data.period.bucket === 'month' && r.data.dynamics.length === 12, r.data.period);
r = await admin.get('/crm/analytics/deal?from=2026-01-01&to=2026-03-31');
ok('a quarter is bucketed by week', r.data.period.bucket === 'week', r.data.period);

// Validation
ok('end before start', (await admin.get('/crm/analytics/deal?from=2026-02-01&to=2026-01-01')).status === 400);
ok('too long', (await admin.get('/crm/analytics/deal?from=2020-01-01&to=2026-01-01')).status === 400);
ok('bad users', (await admin.get('/crm/analytics/deal?users=nope')).status === 400);
ok('pipeline of another type', (await admin.get(`/crm/analytics/lead?category_id=${cat.id}`)).status === 400);
ok('types without stages', (await admin.get('/crm/analytics/contact')).status === 400);
ok('unknown type', (await admin.get('/crm/analytics/nope')).status === 404);

// Targets
const month = new Date().toISOString().slice(0, 7);
ok('non-admin cannot set targets', (await sam.put('/crm/analytics/deal/targets', { month, team_target: '1', users: [] })).status === 403);
ok('non-admin cannot read target editor', (await sam.get('/crm/analytics/deal/targets')).status === 403);
const before = (await admin.get(`/crm/analytics/deal/targets?month=${month}`)).data;
ok('target editor lists people', before.users.some((u) => u.user_id === sam.id));
ok('negative target rejected', (await admin.put('/crm/analytics/deal/targets', { month, team_target: '-5', users: [] })).status === 400);
ok('bad month rejected', (await admin.put('/crm/analytics/deal/targets', { month: '2026-13', users: [] })).status === 400);
r = await admin.put('/crm/analytics/deal/targets', { month, team_target: '5000', users: [{ user_id: sam.id, target: '2000' }, { user_id: ivy.id, target: null }] });
ok('save targets', r.status === 200, r.data);
r = await q();
ok('team target progress', +r.data.targets.team_target === 5000 && +r.data.targets.team_won === 1300, r.data.targets);
const samRow = r.data.managers.find((x) => x.user_id === sam.id);
ok('manager target progress', +samRow.target === 2000 && +samRow.target_won === 1000, samRow);
ok('no target for ivy', r.data.managers.find((x) => x.user_id === ivy.id).target === null);
const after = (await admin.get(`/crm/analytics/deal/targets?month=${month}`)).data;
ok('targets read back', +after.team_target === 5000 && +after.users.find((u) => u.user_id === sam.id).target === 2000);

// Non-admin view and scopes
r = await sam.get(`/crm/analytics/deal?category_id=${cat.id}`);
ok('employee sees analytics (read=all by default)', r.status === 200 && r.data.kpi.current.won_count === 2 && r.data.can_edit_targets === false, r.data.kpi?.current);
const empRole = (await admin.get('/roles')).data.find((x) => x.code === 'employee');
const perms = (await admin.get('/crm/permissions')).data;
const original = Object.entries(perms.matrix[empRole.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
try {
  await admin.put(`/crm/permissions/${empRole.id}`, { entries: original.map((e) => (e.entity_type === 2 && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
  r = await ivy.get(`/crm/analytics/deal?category_id=${cat.id}`);
  ok('read=own: only own deals count', r.data.kpi.current.won_count === 1 && +r.data.kpi.current.won_amount === 300 && r.data.kpi.current.new_count === 1 && r.data.managers.length === 1, r.data.kpi.current);
  ok('read=own: funnel too', r.data.funnel.find((s) => s.stage_id === open[0].id).reached === 1);
  ok('read=own: team target progress only counts visible deals', +r.data.targets.team_won === 300, r.data.targets);
  await admin.put(`/crm/permissions/${empRole.id}`, { entries: original.map((e) => (e.entity_type === 2 && e.action === 'read' ? { ...e, scope: 'none' } : e)) });
  ok('read=none: forbidden', (await ivy.get('/crm/analytics/deal')).status === 403);
} finally {
  await admin.put(`/crm/permissions/${empRole.id}`, { entries: original });
}

// Stage history survives a stage being deleted: records moved off it enter the new stage.
const extra = (await admin.post(`/crm/categories/${cat.id}/stages`, { name: `Temp ${stamp}`, semantics: 'process' }));
if (extra.status === 200) {
  const tmp = extra.data.categories.find((c) => c.id === cat.id).stages.find((s) => s.name === `Temp ${stamp}`);
  const d5 = await deal(sam, 50, [tmp]);
  ids.push(d5);
  const del = await admin.req('DELETE', `/crm/stages/${tmp.id}`, { target_id: open[0].id });
  r = await q();
  ok('deleted stage: record still counted in the funnel', del.status === 200 && r.data.funnel.find((s) => s.stage_id === open[0].id).reached === 5, [del.status, del.data]);
} else console.log('skip stage-delete check:', extra.status, extra.data);

await admin.put('/crm/analytics/deal/targets', { month, team_target: null, users: [] });
for (const id of ids) { await admin.del(`/crm/deal/items/${id}`); await admin.del(`/crm/recycle-bin/${id}`); }
r = await admin.del(`/crm/categories/${cat.id}`);
ok('cleanup: pipeline removed', r.status === 200, r.data);
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
