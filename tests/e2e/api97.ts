// Business processes on CRM records: designed by CRM administrators with the record's fields;
// started when a record is made, when it changes, or by hand; approvals by the responsible
// person's head; conditions on amounts; changes to the stage and custom fields; tasks; waits;
// cancelling; and no loops from the processes' own changes.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const waitFor = async <T>(f: () => Promise<T | null | undefined | false>, ms = 15000): Promise<T | null> => { const end = Date.now() + ms; while (Date.now() < end) { const v = await f(); if (v) return v as T; await sleep(300); } return null; };
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Bp${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const templates: string[] = [];
const fieldIds: string[] = [];
let catId = '';

try {
  const sam = await mk('Sam'), boss = await mk('Boss'), ivy = await mk('Ivy');
  const dept = (await admin.post('/departments', { name: `Sales ${stamp}`, head_user_id: boss.id })).data;
  await admin.post(`/departments/${dept.id}/members`, { user_id: sam.id });
  const tier = (await admin.post('/crm/deal/fields', { label: `Tier ${stamp}`, kind: 'list', settings: { options: [{ label: 'Gold' }, { label: 'Silver' }] } })).data;
  fieldIds.push(tier.id);
  const pipe = (await admin.post('/crm/deal/categories', { name: `Approvals ${stamp}` })).data.categories.find((c: any) => c.name === `Approvals ${stamp}`);
  catId = pipe.id;
  const stageNames = pipe.stages.map((s: any) => s.name);

  let r = await admin.get('/crm/deal/workflows');
  ok('the record\'s fields for designing: stage, amount, custom fields', r.data.fields.some((f: any) => f.code === 'stage' && f.options.includes(stageNames[1])) && r.data.fields.some((f: any) => f.code === 'opportunity' && f.kind === 'number') && r.data.fields.some((f: any) => f.code === tier.code && f.options.includes('Gold')), r.data.fields);
  ok('only CRM administrators design processes', (await sam.post('/crm/deal/workflows', { name: 'x' })).status === 403);
  ok('steps are checked against the fields', (await admin.post('/crm/deal/workflows', { name: `Bad ${stamp}`, steps: [{ kind: 'set_field', field: 'nope', value: 'x' }] })).status === 400);

  // Big deals need the head's approval; then the deal moves on and is marked Gold.
  r = await admin.post('/crm/deal/workflows', {
    name: `Big deal check ${stamp}`, trigger: 'create',
    steps: [{
      kind: 'condition', field: 'opportunity', op: 'gt', value: '10000',
      then: [{ kind: 'approval', title: 'Approve {{title}}', who: { users: [], author: false, head: true }, mode: 'any',
        approved: [{ kind: 'set_field', field: 'stage', value: stageNames[1] }, { kind: 'set_field', field: tier.code, value: 'Gold' }, { kind: 'notify', who: { users: [], author: true, head: false }, text: 'Approved: {{title}}' }],
        rejected: [{ kind: 'stop', status: 'rejected' }] }],
      else: [{ kind: 'set_field', field: tier.code, value: 'Silver' }],
    }],
  });
  ok('a process for new deals', r.status === 200 && r.data.crm_type === 'deal', r.data);
  templates.push(r.data.id);

  const small = (await sam.post('/crm/deal/items', { title: `Small ${stamp}`, category_id: pipe.id, opportunity: '500', assigned_to: sam.id })).data;
  let rec = (await sam.get(`/crm/deal/items/${small.id}`)).data;
  ok('a small deal goes through at once (Silver)', rec.uf[tier.code] && (await sam.get(`/crm/items/${small.id}/workflows`)).data.processes[0]?.status === 'completed', [rec.uf, (await sam.get(`/crm/items/${small.id}/workflows`)).data]);

  const big = (await sam.post('/crm/deal/items', { title: `Big ${stamp}`, category_id: pipe.id, opportunity: '25000', assigned_to: sam.id })).data;
  let procs = (await sam.get(`/crm/items/${big.id}/workflows`)).data.processes;
  ok('a big deal waits for the head\'s approval', procs[0]?.status === 'waiting', procs);
  const ap = (await boss.get('/approvals')).data.items.find((a: any) => a.item_id === big.id);
  ok('…which the head sees, linked to the deal', ap?.title === `Approve Big ${stamp}` && ap.link === `/crm/deal/${big.id}`, ap);
  await boss.patch(`/approvals/${ap.id}`, { approve: true });
  rec = (await sam.get(`/crm/deal/items/${big.id}`)).data;
  ok('approved: the stage moves on', rec.stage_name === stageNames[1], rec.stage_name);
  const goldId = (await admin.get('/crm/deal/fields')).data.find((f: any) => f.id === tier.id).settings.options.find((o: any) => o.label === 'Gold').id;
  ok('…the custom field is set (by its choice)', rec.uf[tier.code] === goldId, rec.uf);
  ok('…and the responsible person is told', !!(await waitFor(async () => (await sam.get('/notifications?limit=20')).data.items.some((n: any) => n.title === `Approved: Big ${stamp}`))));
  procs = (await sam.get(`/crm/items/${big.id}/workflows`)).data.processes;
  ok('the process completed, with its log', procs[0].status === 'completed' && procs[0].log.some((l: any) => /Set “Stage”/.test(l.text)), procs[0]);
  const tl = (await sam.get(`/crm/items/${big.id}/timeline`)).data.items;
  ok('its changes are in the record\'s history like any change', tl.some((e: any) => e.kind === 'stage'));

  // Started by hand; with a task; cancelled.
  r = await admin.post('/crm/deal/workflows', { name: `Follow-up ${stamp}`, trigger: 'manual', steps: [{ kind: 'task', who: { users: [], author: true, head: false }, title: 'Call about {{title}}', wait: true }, { kind: 'notify', who: { users: [ivy.id], author: false, head: false }, text: 'Followed up' }] });
  templates.push(r.data.id);
  const manual = r.data.id;
  let list = (await sam.get(`/crm/items/${small.id}/workflows`)).data;
  ok('records offer the processes started by hand', list.can_start.some((t: any) => t.id === manual), list);
  ok('only people who may change the record start them', (await ivy.post(`/crm/items/${big.id}/workflows/${manual}`)).status === 403);
  r = await sam.post(`/crm/items/${small.id}/workflows/${manual}`);
  const run = r.data.processes.find((p: any) => p.name === `Follow-up ${stamp}`);
  ok('started by hand: it makes a task and waits for it', r.status === 200 && run.status === 'waiting', r.data);
  ok('…not twice at once', (await sam.post(`/crm/items/${small.id}/workflows/${manual}`)).status === 400);
  r = await sam.post(`/workflows/instances/${run.id}/cancel`);
  ok('…and its starter cancels it', r.status === 200 && r.data.find((p: any) => p.id === run.id).status === 'cancelled', r.data);

  // On change; and no loops: a process's own change doesn't start processes.
  r = await admin.post('/crm/deal/workflows', { name: `On change ${stamp}`, trigger: 'update', steps: [{ kind: 'set_field', field: 'comments', value: 'Touched by a process' }] });
  templates.push(r.data.id);
  await sam.patch(`/crm/deal/items/${small.id}`, { title: `Small renamed ${stamp}` });
  rec = (await sam.get(`/crm/deal/items/${small.id}`)).data;
  const runs = (await sam.get(`/crm/items/${small.id}/workflows`)).data.processes.filter((p: any) => p.name === `On change ${stamp}`);
  ok('a change starts the process (once: its own change starts nothing)', rec.comments === 'Touched by a process' && runs.length === 1, runs.length);

  // Testing: only the testers' records.
  await admin.patch(`/workflows/${templates[0]}`, { test_users: [ivy.id] });
  const third = (await sam.post('/crm/deal/items', { title: `Third ${stamp}`, category_id: pipe.id, opportunity: '1', assigned_to: sam.id })).data;
  ok('in testing, other people\'s new records don\'t start it', (await sam.get(`/crm/items/${third.id}/workflows`)).data.processes.length === 0);
} finally {
  for (const t of templates) await admin.del(`/workflows/${t}`).catch(() => {});
  if (catId) for (const d of (await admin.get(`/crm/deal/items?category_id=${catId}&limit=50`)).data.items ?? []) await admin.del(`/crm/deal/items/${d.id}`);
  for (const f of fieldIds) await admin.del(`/crm/deal/fields/${f}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
