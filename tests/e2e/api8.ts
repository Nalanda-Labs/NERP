// CRM automation: robots, conditions, delays, loops, triggers, webhooks.
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
const runs = async (id) => (await admin.get(`/crm/items/${id}/automation`)).data.items;
const done = (id, n = 1) => waitFor(async () => { const r = await runs(id); return r.filter((x) => x.status !== 'scheduled' && x.status !== 'running').length >= n ? r : null; });

// ---- a pipeline of our own ----
let r: any = await admin.post('/crm/deal/categories', { name: `Auto ${stamp}` });
const cat = r.data.categories.find((c) => c.name === `Auto ${stamp}`);
const st = Object.fromEntries(cat.stages.map((s) => [s.name, s.id]));
ok('pipeline with stages', st.New && st.Preparation && st['Proposal sent'] && st.Won, Object.keys(st));
const mkDeal = async (extra = {}) => (await sam.post('/crm/deal/items', { title: `D ${stamp} ${Math.random().toString(36).slice(2, 6)}`, category_id: cat.id, assigned_to: sam.id, ...extra })).data;

// ---- validation ----
ok('non-admin cannot add robots', (await sam.post('/crm/robots', { category_id: cat.id, stage_id: st.New, action: 'comment', params: { text: 'x' } })).status === 403);
ok('unknown action', (await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.New, action: 'launch' })).status === 400);
const otherStage = (await admin.get('/crm/config')).data.types.find((t) => t.code === 'deal').categories.find((c) => c.is_default).stages[0].id;
ok('stage must be in pipeline', (await admin.post('/crm/robots', { category_id: cat.id, stage_id: otherStage, action: 'comment', params: { text: 'x' } })).status === 400);
ok('required params', (await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.New, action: 'activity', params: {} })).status === 400);
ok('bad condition op', (await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.New, action: 'comment', params: { text: 'x' }, conditions: [{ field: 'opportunity', op: 'like', value: 1 }] })).status === 400);
ok('move_stage needs own pipeline stage', (await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.New, action: 'move_stage', params: { stage_id: otherStage } })).status === 400);

// ---- robots on "New" ----
r = await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.New, name: 'Call them', action: 'activity', params: { kind: 'call', subject: 'Call about {{title}} ({{amount}})', deadline_minutes: 60, responsible: 'assigned' } });
ok('create activity robot', r.status === 200 && r.data.params.kind === 'call', r.data);
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.New, name: 'Big deal note', action: 'comment', params: { text: 'Big one: {{amount}}' }, conditions: [{ field: 'opportunity', op: 'gt', value: '1000' }] });
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.New, name: 'Tell Ivy', action: 'notify', params: { to: [ivy.id, 'assigned'], message: 'New deal {{title}} by {{assigned}}' } });
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.New, name: 'Probability', action: 'set_field', params: { field: 'probability', value: 10 } });

const small = await mkDeal({ opportunity: '500', currency: 'USD' });
let rr: any = await done(small.id, 4);
ok('robots ran on create', rr && rr.filter((x) => x.status === 'done').length === 3 && rr.some((x) => x.robot_name === 'Big deal note' && x.status === 'skipped' && x.result === 'conditions not met'), rr);
r = await sam.get(`/crm/activities?item_id=${small.id}&state=open`);
ok('activity planned with placeholders', r.data.items.some((a) => a.kind === 'call' && a.subject === `Call about ${small.title} ($500.00)` && a.responsible_id === sam.id), r.data.items);
ok('field set by robot', +(await sam.get(`/crm/deal/items/${small.id}`)).data.probability === 10);
r = await ivy.get('/notifications');
ok('notification sent', (r.data.items ?? r.data).some((n) => n.title === `New deal ${small.title} by sam Doc`), r.data);
r = await sam.get(`/crm/items/${small.id}/timeline`);
ok('timeline: robots by Automation', r.data.items.filter((e) => e.kind === 'robot' && e.author_name?.startsWith('Automation')).length === 3, r.data.items.map((e) => [e.kind, e.author_name]));
const big = await mkDeal({ opportunity: '5000', currency: 'EUR' });
rr = await done(big.id, 4);
r = await sam.get(`/crm/items/${big.id}/timeline`);
ok('condition met -> comment', r.data.items.some((e) => e.kind === 'comment' && e.body === 'Big one: €5,000.00'), r.data.items.map((e) => e.body));

// ---- delays: skipped when the record moves on ----
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.Preparation, name: 'Later', action: 'comment', params: { text: 'later' }, delay_minutes: 1 });
const d1 = await mkDeal();
await done(d1.id, 4);
await sam.patch(`/crm/deal/items/${d1.id}`, { stage_id: st.Preparation });
rr = await waitFor(async () => (await runs(d1.id)).find((x) => x.robot_name === 'Later'));
ok('delayed robot scheduled ~1 min ahead', rr?.status === 'scheduled' && +new Date(rr.run_at) - Date.now() > 40000, rr);
await sam.patch(`/crm/deal/items/${d1.id}`, { stage_id: st.New });
const delayedCheck = (async () => { const x = await waitFor(async () => (await runs(d1.id)).find((x) => x.robot_name === 'Later' && x.status !== 'scheduled'), 120000); return x; })();

// ---- assign round robin + task + email + document ----
const inv = (await admin.get('/crm/doc-templates?type=deal')).data.items.find((t) => t.builtin_key === 'typst.invoice');
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st['Proposal sent'], name: 'Spread', action: 'assign', params: { users: [sam.id, ivy.id], mode: 'round_robin' } });
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st['Proposal sent'], name: 'Follow up', action: 'task', params: { title: `Follow up ${stamp} {{title}}`, deadline_minutes: 1440, responsible: 'creator' } });
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st['Proposal sent'], name: 'Mail', action: 'email', params: { subject: `Your proposal ${stamp}`, body: 'Dear client,\nhere is {{title}}.' } });
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st['Proposal sent'], name: 'Invoice', action: 'document', params: { template_id: inv.id } });
const contact = (await sam.post('/crm/contact/items', { first_name: 'Cli', last_name: `Ent${stamp}`, multifields: [{ kind: 'email', value: `client.${stamp}@example.com` }] })).data.id;
const p1 = await mkDeal({ contact_ids: [contact] }), p2 = await mkDeal({ contact_ids: [contact] });
await done(p1.id, 4); await done(p2.id, 4);
await sam.patch(`/crm/deal/items/${p1.id}`, { stage_id: st['Proposal sent'] });
await sam.patch(`/crm/deal/items/${p2.id}`, { stage_id: st['Proposal sent'] });
await done(p1.id, 8); await done(p2.id, 8);
const a1 = (await admin.get(`/crm/deal/items/${p1.id}`)).data.assigned_to, a2 = (await admin.get(`/crm/deal/items/${p2.id}`)).data.assigned_to;
ok('round robin spreads', new Set([a1, a2]).size === 2 && [a1, a2].every((a) => [sam.id, ivy.id].includes(a)), [a1, a2]);
r = await sam.get(`/tasks?q=${encodeURIComponent(`Follow up ${stamp}`)}`);
ok('task created for the creator', (r.data.items ?? []).length >= 1, r.data);
r = await sam.get(`/crm/items/${p1.id}/documents`);
ok('document created by robot', r.data.items.length === 1 && r.data.items[0].status === 'ready', r.data);
const mails = await waitFor(async () => { const m = await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent(`subject:"Your proposal ${stamp}"`)}`)).json(); return m.messages?.length >= 2 ? m : null; }, 15000);
ok('emails sent to the client', mails?.messages?.length === 2 && mails.messages[0].To[0].Address === `client.${stamp}@example.com`, mails);
r = await sam.get(`/crm/activities?item_id=${p1.id}&state=completed`);
ok('email logged as activity', r.data.items?.some((a) => a.kind === 'email' && a.subject === `Your proposal ${stamp}`), r.data);
const nomail = await mkDeal();
await done(nomail.id, 4);
await sam.patch(`/crm/deal/items/${nomail.id}`, { stage_id: st['Proposal sent'] });
rr = await done(nomail.id, 8);
ok('email without address fails visibly', rr.some((x) => x.robot_name === 'Mail' && x.status === 'failed' && /no email/.test(x.result)), rr);

// ---- loops are cut off ----
await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.Negotiation, name: 'Ping', action: 'move_stage', params: { stage_id: st[cat.stages[4].name] } });
await admin.post('/crm/robots', { category_id: cat.id, stage_id: cat.stages[4].id, name: 'Pong', action: 'move_stage', params: { stage_id: st.Negotiation } });
const loop = await mkDeal();
await done(loop.id, 4);
await sam.patch(`/crm/deal/items/${loop.id}`, { stage_id: st.Negotiation });
const stopped = await waitFor(async () => (await sam.get(`/crm/items/${loop.id}/timeline?limit=100`)).data.items.find((e) => e.kind === 'automation' && e.data.event === 'stopped'), 30000);
ok('ping-pong loop stopped', !!stopped, stopped);
r = await runs(loop.id);
ok('loop ran a bounded number of times', r.filter((x) => ['Ping', 'Pong'].includes(x.robot_name)).length <= 6, r.length);

// ---- triggers ----
ok('bad trigger event', (await admin.post('/crm/triggers', { category_id: cat.id, stage_id: st.Won, event: 'sunrise' })).status === 400);
r = await admin.post('/crm/triggers', { category_id: cat.id, stage_id: st.Preparation, name: 'Call done', event: 'activity_done', params: { kind: 'call' } });
ok('create trigger', r.status === 200 && r.data.params.kind === 'call');
const t1 = await mkDeal();
await done(t1.id, 4);
const call = (await sam.get(`/crm/activities?item_id=${t1.id}&state=open`)).data.items.find((a) => a.kind === 'call');
await sam.post(`/crm/activities/${call.id}/complete`, { result: 'talked' });
ok('activity_done trigger moved the deal', (await sam.get(`/crm/deal/items/${t1.id}`)).data.stage_id === st.Preparation);
r = await sam.get(`/crm/items/${t1.id}/timeline`);
ok('trigger on timeline', r.data.items.some((e) => e.kind === 'trigger' && e.data.name === 'Call done'));
await admin.post('/crm/triggers', { category_id: cat.id, stage_id: st.New, name: 'Back', event: 'comment_added' });
await sam.post(`/crm/items/${t1.id}/timeline`, { body: 'hello' });
ok('triggers do not move backwards by default', (await sam.get(`/crm/deal/items/${t1.id}`)).data.stage_id === st.Preparation);
r = await admin.post('/crm/triggers', { category_id: cat.id, stage_id: st.Won, name: 'Paid', event: 'webhook' });
ok('webhook trigger has a token', r.data.token?.length >= 20, r.data);
const hook = r.data.token;
let wr = await fetch(`http://localhost:8080/api/crm/automation/webhook/${hook}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ item_id: t1.id }) });
ok('webhook moves the deal (no login)', wr.status === 200 && (await wr.json()).moved === true && (await sam.get(`/crm/deal/items/${t1.id}`)).data.stage_id === st.Won);
wr = await fetch(`http://localhost:8080/api/crm/automation/webhook/nope`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ item_id: t1.id }) });
ok('wrong token 404', wr.status === 404);
wr = await fetch(`http://localhost:8080/api/crm/automation/webhook/${hook}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ item_id: contact }) });
ok('webhook only for its pipeline', wr.status === 404);

// ---- overview, delay outcome, cleanup ----
r = await admin.get(`/crm/automation?category_id=${cat.id}`);
ok('overview', r.data.robots.length >= 10 && r.data.triggers.length === 3 && r.data.webhook_base.endsWith('/api/crm/automation/webhook/'), [r.data.robots.length, r.data.triggers.length]);
ok('non-admin cannot read config', (await sam.get(`/crm/automation?category_id=${cat.id}`)).status === 403);
const later = await delayedCheck;
ok('delayed robot skipped after the deal left the stage', later?.status === 'skipped' && later.result === 'the record left the stage', later);
const users = (await admin.get('/users?q=automation')).data;
ok('Automation user hidden from employees', !(users.items ?? users).some((u) => u.email === 'automation@nerp.invalid'), users);
ok('Automation user cannot sign in', (await new Client().post('/auth/login', { email: 'automation@nerp.invalid', password: '!' })).status !== 200);
// Move the test deals out (to the default pipeline) so the pipeline can be removed.
const defaultCat = (await admin.get('/crm/config')).data.types.find((t) => t.code === 'deal').categories.find((c) => c.is_default).id;
for (const id of [small, big, d1, p1, p2, nomail, loop, t1].map((x) => x.id)) { await admin.patch(`/crm/deal/items/${id}`, { category_id: defaultCat }); await admin.del(`/crm/deal/items/${id}`); }
r = await admin.del(`/crm/categories/${cat.id}`);
ok('pipeline removed', r.status === 200, r.data);

console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
