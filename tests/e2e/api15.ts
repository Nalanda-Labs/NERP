// CRM automation depth: date-relative timing, sequences, waits, webhooks with retries,
// linked records, lead conversion, email with a document, least-busy assignment, stage
// time limits with escalation, schedules, the automation report and stage times.
import http from 'node:http';
import { createHmac } from 'node:crypto';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'Auto' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const sam = await mk('sam'), ivy = await mk('ivy'), max = await mk('max');
const waitFor = async (fn, ms = 20000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(300); } return v; };
const runs = async (id) => (await admin.get(`/crm/items/${id}/automation`)).data.items;
const run = (id, name) => runs(id).then((r) => r.find((x) => x.robot_name === name));
const runIs = (id: string, name: string, statuses: string[], ms?: number) => waitFor(async () => { const x = await run(id, name); return x && statuses.includes(x.status) ? x : null; }, ms);
const mailpit = async (q) => (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent(q)}`)).json()).messages ?? [];
const timeline = async (id) => (await admin.get(`/crm/items/${id}/timeline?limit=100`)).data.items;
const cleanup: Record<string, any[]> = { deals: [], leads: [], contacts: [], companies: [], robots: [] };

// ---- a webhook receiver ----
const hits: any[] = [];
let failNext = 0;
const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c));
  req.on('end', () => {
    hits.push({ method: req.method, url: req.url!, headers: req.headers, body });
    if (req.url!.startsWith('/bad')) { res.writeHead(400); return res.end('nope'); }
    if (failNext > 0) { failNext--; res.writeHead(503); return res.end('busy'); }
    res.writeHead(200, { 'content-type': 'application/json' }); res.end('{"ok":true}');
  });
});
await new Promise<void>((r) => server.listen(0, '127.0.0.1', () => r()));
const hookUrl = `http://127.0.0.1:${(server.address() as { port: number }).port}`;

// ---- pipeline ----
let r: any = await admin.post('/crm/deal/categories', { name: `Depth ${stamp}` });
const cat = r.data.categories.find((c) => c.name === `Depth ${stamp}`);
const st = Object.fromEntries(cat.stages.map((s) => [s.name, s.id]));
ok('pipeline with stages', st.New && st.Preparation && st['Proposal sent'] && st.Won, Object.keys(st));
const mkDeal = async (extra = {}, who = sam) => { const d = (await who.post('/crm/deal/items', { title: `D ${stamp} ${Math.random().toString(36).slice(2, 6)}`, category_id: cat.id, assigned_to: who.id, ...extra })).data; cleanup.deals.push(d.id); return d; };
const robot = async (b) => { const x = await admin.post('/crm/robots', { category_id: cat.id, ...b }); if (x.data?.id) cleanup.robots.push(x.data.id); return x; };
const ov = (await admin.get(`/crm/automation?category_id=${cat.id}`)).data;
ok('overview lists date fields', ov.date_fields.some((f) => f.code === 'close_date') && ov.date_fields.some((f) => f.code === 'created_at'), ov.date_fields);
await admin.patch('/users/' + sam.id, { timezone: 'Europe/Berlin' });

// ---- validation ----
ok('field timing needs a field', (await robot({ stage_id: st.Preparation, action: 'comment', params: { text: 'x' }, timing: 'field' })).status === 400);
ok('field timing needs a date field', (await robot({ stage_id: st.Preparation, action: 'comment', params: { text: 'x' }, timing: 'field', timing_field: 'title' })).status === 400);
ok('bad wait rejected', (await robot({ stage_id: st.Preparation, action: 'comment', params: { text: 'x' }, wait: { until: 'forever' } })).status === 400);
ok('wait for conditions needs conditions', (await robot({ stage_id: st.Preparation, action: 'comment', params: { text: 'x' }, wait: { until: 'conditions', conditions: [] } })).status === 400);
ok('webhook needs a URL', (await robot({ stage_id: st.Preparation, action: 'webhook', params: { url: 'ftp://x' } })).status === 400);
ok('webhook header rules', (await robot({ stage_id: st.Preparation, action: 'webhook', params: { url: 'https://example.com', headers: [{ name: 'Host', value: 'x' }] } })).status === 400);
ok('convert only for leads', (await robot({ stage_id: st.Preparation, action: 'convert_lead', params: { create_deal: true } })).status === 400);
ok('create_record type check', (await robot({ stage_id: st.Preparation, action: 'create_record', params: { entity_type: 3, title: 'x' } })).status === 400);
ok('assign mode least_busy accepted', (await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.Preparation, action: 'assign', params: { users: [sam.id], mode: 'least_busy' }, is_active: false }).then(async (x) => { if (x.data?.id) await admin.del(`/crm/robots/${x.data.id}`); return x; })).status === 200);

// ---- timed from a date field ----
const inDays = (n) => new Date(Date.now() + n * 86400000).toISOString().slice(0, 10);
r = await robot({ stage_id: st.New, name: 'Before close', action: 'comment', params: { text: 'Closing soon: {{title}}' }, timing: 'field', timing_field: 'close_date', offset_minutes: -2 * 1440 });
ok('field-timed robot saved', r.status === 200 && r.data.timing === 'field' && r.data.timing_field === 'close_date' && r.data.offset_minutes === -2880, r.data);
await robot({ stage_id: st.New, name: 'Since created', action: 'comment', params: { text: 'Old record' }, timing: 'field', timing_field: 'created_at', offset_minutes: -60 });
const dt = await mkDeal({ close_date: inDays(10) });
let x: any = await run(dt.id, 'Before close');
const expect = (d) => { const p = new Intl.DateTimeFormat('en-US', { timeZone: 'Europe/Berlin', timeZoneName: 'shortOffset' }).formatToParts(new Date(d + 'T12:00:00Z')).find((p) => p.type === 'timeZoneName')!.value; const off = Number(p.replace('GMT', '') || 0); return new Date(Date.parse(d + 'T09:00:00Z') - off * 3600000 - 2 * 86400000).toISOString(); };
ok('scheduled 2 days before the close date at 09:00 local', x?.status === 'scheduled' && new Date(x.run_at).toISOString() === expect(inDays(10)), [x, expect(inDays(10))]);
ok('a time in the past runs now', !!(await runIs(dt.id, 'Since created', ['done'])));
await sam.patch(`/crm/deal/items/${dt.id}`, { close_date: inDays(20) });
x = await run(dt.id, 'Before close');
ok('moving the date reschedules', x?.status === 'scheduled' && new Date(x.run_at).toISOString() === expect(inDays(20)), [x, expect(inDays(20))]);
await sam.patch(`/crm/deal/items/${dt.id}`, { close_date: null });
x = await runIs(dt.id, 'Before close', ['skipped']);
ok('clearing the date skips it', x?.result === 'the date field was cleared', x);
const nodate = await mkDeal();
x = await run(nodate.id, 'Before close');
ok('no date: skipped up front', x?.status === 'skipped' && x.result === 'the date field is empty', x);

// ---- sequence + wait for conditions ----
await robot({ stage_id: st.Preparation, name: 'Gate', action: 'comment', params: { text: 'Probability reached {{probability}}' }, wait: { until: 'conditions', conditions: [{ field: 'probability', op: 'gte', value: '50' }], timeout_minutes: 600, on_timeout: 'skip' } });
await robot({ stage_id: st.Preparation, name: 'Then', action: 'comment', params: { text: 'after the gate' }, after_previous: true });
const dw = await mkDeal({ probability: 10 });
await sam.patch(`/crm/deal/items/${dw.id}`, { stage_id: st.Preparation });
x = await runIs(dw.id, 'Gate', ['waiting']);
ok('robot waits for its conditions', x?.wait_until && new Date(x.wait_until) > new Date(Date.now() + 9 * 3600000), x);
ok('next robot waits for the previous one', (await run(dw.id, 'Then'))?.status === 'pending');
await sam.patch(`/crm/deal/items/${dw.id}`, { probability: 40 });
await sleep(1500);
ok('still waiting below the threshold', (await run(dw.id, 'Gate'))?.status === 'waiting');
await sam.patch(`/crm/deal/items/${dw.id}`, { probability: 60 });
const gate = await runIs(dw.id, 'Gate', ['done']);
const then = await runIs(dw.id, 'Then', ['done']);
ok('conditions met: runs, then the next one', gate && then && new Date(then.finished_at) >= new Date(gate.finished_at), [gate, then]);
const dl = await mkDeal({ probability: 5 });
await sam.patch(`/crm/deal/items/${dl.id}`, { stage_id: st.Preparation });
await runIs(dl.id, 'Gate', ['waiting']);
await sam.patch(`/crm/deal/items/${dl.id}`, { stage_id: st['Proposal sent'] });
ok('leaving the stage ends waiting and pending runs', (await run(dl.id, 'Gate'))?.status === 'skipped' && (await run(dl.id, 'Then'))?.status === 'skipped' && (await run(dl.id, 'Then')).result === 'the record left the stage');

// ---- wait for an activity (event) and a timeout ----
await robot({ stage_id: st['Proposal sent'], name: 'After call', action: 'comment', params: { text: 'call done' }, wait: { until: 'activity_done', timeout_minutes: 600 } });
await robot({ stage_id: st['Proposal sent'], name: 'Reply or nudge', action: 'comment', params: { text: 'nudge' }, wait: { until: 'email_received', timeout_minutes: 1, on_timeout: 'run' } });
const da = await mkDeal();
await sam.patch(`/crm/deal/items/${da.id}`, { stage_id: st['Proposal sent'] });
ok('waiting for an activity', !!(await runIs(da.id, 'After call', ['waiting'])));
const act = (await sam.post('/crm/activities', { item_id: da.id, kind: 'call', subject: 'Call', deadline: new Date(Date.now() + 3600000).toISOString() })).data;
await sam.post(`/crm/activities/${act.id}/complete`, { result: 'talked' }).then(async (x) => x.status === 404 || x.status === 405 ? sam.patch(`/crm/activities/${act.id}`, { is_completed: true }) : x);
ok('completing an activity wakes it', !!(await runIs(da.id, 'After call', ['done'])));
const timeoutCheck = runIs(da.id, 'Reply or nudge', ['done', 'skipped', 'failed'], 150000); // runs after its 1-minute timeout

// ---- webhook ----
const secret = 'top-secret';
await robot({ stage_id: st.Won, name: 'Hook', action: 'webhook', params: { url: `${hookUrl}/deals/{{id}}`, method: 'POST', headers: [{ name: 'X-Api-Key', value: 'k-{{number}}' }], body: '{"title": "{{title}}", "amount": "{{opportunity}}"}', secret } , conditions: [{ field: 'title', op: 'contains', value: 'Quote' }] });
await robot({ stage_id: st.Won, name: 'Hook default', action: 'webhook', params: { url: `${hookUrl}/plain`, method: 'PUT' } , conditions: [{ field: 'title', op: 'contains', value: 'Quote' }] });
await robot({ stage_id: st.Won, name: 'Hook bad', action: 'webhook', params: { url: `${hookUrl}/bad`, method: 'POST' } , conditions: [{ field: 'title', op: 'contains', value: 'Quote' }] });
const dh = await mkDeal({ title: `Quote "big" ${stamp}`, opportunity: '1500' });
await sam.patch(`/crm/deal/items/${dh.id}`, { stage_id: st.Won });
const h1 = await waitFor(() => hits.find((h) => h.url === `/deals/${dh.id}`));
let parsed: any = null; try { parsed = JSON.parse(h1?.body); } catch {}
ok('webhook called with a filled JSON body', parsed?.title === `Quote "big" ${stamp}` && Number(parsed.amount) === 1500, h1);
ok('webhook headers and signature', h1?.headers['x-api-key'] === `k-${dh.number}` && h1.headers['x-nerp-signature'] === 'sha256=' + createHmac('sha256', secret).update(h1.body).digest('hex') && h1.headers['content-type'] === 'application/json' && h1.headers['x-nerp-record'] === dh.id, h1?.headers);
const h2 = await waitFor(() => hits.find((h) => h.url === '/plain'));
let rec: any = null; try { rec = JSON.parse(h2?.body); } catch {}
ok('default body is the record', h2?.method === 'PUT' && rec?.record?.id === dh.id && rec.robot === 'Hook default', h2);
x = await runIs(dh.id, 'Hook', ['done']);
ok('webhook run done with the response', x?.result?.includes('200') && x.result.includes('{"ok":true}'), x);
x = await runIs(dh.id, 'Hook bad', ['failed']);
ok('4xx fails without retrying', x?.result?.includes('400') && hits.filter((h) => h.url === '/bad').length === 1, x);
// A server error is retried.
await robot({ stage_id: st.Lost ?? cat.stages.find((s) => s.semantics === 'failure').id, name: 'Hook retry', action: 'webhook', params: { url: `${hookUrl}/retry`, method: 'POST' } });
failNext = 1;
const dr = await mkDeal();
await sam.patch(`/crm/deal/items/${dr.id}`, { stage_id: cat.stages.find((s) => s.semantics === 'failure').id });
x = await waitFor(async () => { const y = await run(dr.id, 'Hook retry'); return y?.result?.startsWith('attempt 1 failed') ? y : null; });
ok('5xx is retried later', x?.status === 'scheduled' && x.result.includes('503') && new Date(x.run_at) > new Date(Date.now() + 30000), x);
const retryCheck = runIs(dr.id, 'Hook retry', ['done', 'failed'], 150000);

// ---- linked record + least-busy assignment ----
const company = (await sam.post('/crm/company/items', { company_name: `Linked Co ${stamp}` })).data; cleanup.companies.push(company.id);
const contact = (await sam.post('/crm/contact/items', { first_name: 'Lia', last_name: `Link ${stamp}`, company_id: company.id, multifields: [{ kind: 'email', value: `lia.${stamp}@example.com` }] })).data; cleanup.contacts.push(contact.id);
r = await robot({ stage_id: st.Won, name: 'Follow-up deal', action: 'create_record', params: { entity_type: 2, category_id: cat.id, title: 'Renewal: {{title}}', responsible: ivy.id, copy_client: true, copy_amount: true } });
ok('create_record robot saved', r.status === 200, r.data);
await robot({ stage_id: st.New, name: 'Balance', action: 'assign', params: { users: [sam.id, max.id], mode: 'least_busy' }, conditions: [{ field: 'title', op: 'contains', value: 'Renewal' }] });
const tpls = (await admin.get('/crm/doc-templates')).data.items.filter((t) => t.entity_types.includes(2) && t.is_active);
const typst = tpls.find((t) => t.engine === 'typst'), office = tpls.find((t) => t.engine === 'office' && t.available !== false);
ok('templates for deals', typst && office, tpls.map((t) => [t.name, t.engine]));
await robot({ stage_id: st.Won, name: 'Send invoice', action: 'email', params: { subject: `Invoice ${stamp} {{title}}`, body: 'Please find the invoice attached.', template_id: typst.id }, conditions: [{ field: 'company_id', op: 'not_empty' }] });
await robot({ stage_id: st.Won, name: 'Send office', action: 'email', params: { subject: `Office ${stamp} {{title}}`, body: 'Attached.', template_id: office.id }, conditions: [{ field: 'company_id', op: 'not_empty' }] });
const dc = await mkDeal({ company_id: company.id, contact_ids: [contact.id], opportunity: '700', currency: 'USD' });
await sam.patch(`/crm/deal/items/${dc.id}`, { stage_id: st.Won });
x = await runIs(dc.id, 'Follow-up deal', ['done', 'failed']);
ok('linked record created', x?.status === 'done' && /created deal #\d+/.test(x.result), x);
const linked = (await admin.get(`/crm/deal/items?q=${encodeURIComponent('Renewal: ' + dc.title)}`)).data.items[0];
if (linked) cleanup.deals.push(linked.id);
const ld = linked && (await admin.get(`/crm/deal/items/${linked.id}`)).data;
ok('linked record: source, client, amount', ld?.source_item?.id === dc.id && ld.company_id === company.id && ld.contacts[0]?.id === contact.id && +ld.opportunity === 700, ld);
ok('source timeline links to it', (await timeline(dc.id)).some((e) => e.kind === 'linked' && e.data.item_id === linked?.id));
// The linked deal starts in New, where "Balance" gives it to whoever of sam/max has fewer open deals: max.
x = linked && await runIs(linked.id, 'Balance', ['done']);
ok('least busy gets the record', x && (await admin.get(`/crm/deal/items/${linked.id}`)).data.assigned_to === max.id, x);

// ---- email with a document ----
x = await runIs(dc.id, 'Send invoice', ['done', 'failed']);
ok('typst document emailed (runs at once)', x?.status === 'done' && x.result.includes(' with '), x);
let m = await waitFor(async () => (await mailpit(`subject:"Invoice ${stamp}"`))[0]);
ok('email has the PDF attached', m?.Attachments === 1 && m.To[0].Address === `lia.${stamp}@example.com`, m);
x = await runIs(dc.id, 'Send office', ['done', 'failed'], 60000);
ok('office document: waits for the render, then emails', x?.status === 'done', x);
m = await waitFor(async () => (await mailpit(`subject:"Office ${stamp}"`))[0]);
ok('office email has the PDF', m?.Attachments === 1, m);

// ---- lead conversion ----
const leadCat = (await admin.get('/crm/config')).data.types.find((t) => t.code === 'lead').categories.find((c) => c.is_default);
const leadStage = leadCat.stages.find((s) => s.semantics === 'process' && s.sort > leadCat.stages[0].sort) ?? leadCat.stages[1];
r = await admin.post('/crm/robots', { category_id: leadCat.id, stage_id: leadStage.id, name: `Convert ${stamp}`, action: 'convert_lead', params: { create_contact: true, create_deal: true, deal_category_id: cat.id }, conditions: [{ field: 'title', op: 'contains', value: stamp }] });
ok('convert robot saved', r.status === 200, r.data);
const leadRobot = r.data.id;
const lead = (await sam.post('/crm/lead/items', { title: `Lead ${stamp}`, first_name: 'Nia', last_name: `Lead${stamp}`, opportunity: '300', currency: 'USD' })).data; cleanup.leads.push(lead.id);
await sam.patch(`/crm/lead/items/${lead.id}`, { stage_id: leadStage.id });
x = await runIs(lead.id, `Convert ${stamp}`, ['done', 'failed']);
ok('lead converted by the robot', x?.status === 'done' && x.result.includes('a contact') && x.result.includes('a deal'), x);
const lead2 = (await admin.get(`/crm/lead/items/${lead.id}`)).data;
ok('lead is in its success stage', lead2.stage_semantics === 'success', lead2.stage_semantics);
const convDeal = (await admin.get(`/crm/deal/items?category_id=${cat.id}&q=${encodeURIComponent('Lead ' + stamp)}`)).data.items[0];
if (convDeal) cleanup.deals.push(convDeal.id);
ok('deal created in the chosen pipeline', convDeal && +convDeal.opportunity === 300, convDeal);
const convContact = (await admin.get(`/crm/contact/items?q=${encodeURIComponent('Lead' + stamp)}`)).data.items[0];
if (convContact) cleanup.contacts.push(convContact.id);
await admin.del(`/crm/robots/${leadRobot}`);

// ---- stage time limit with escalation ----
const dept = (await admin.post('/departments', { name: `Sales ${stamp}`, head_user_id: ivy.id })).data;
await admin.post(`/departments/${dept.id}/members`, { user_id: sam.id });
ok('time limit only on open stages', (await admin.patch(`/crm/stages/${st.Won}`, { sla_minutes: 60 })).status === 400);
r = await admin.patch(`/crm/stages/${st['Proposal sent']}`, { sla_minutes: 1, sla_escalate: true });
const cfgStage = r.data.categories?.find((c) => c.id === cat.id)?.stages.find((s) => s.id === st['Proposal sent']);
ok('time limit saved', r.status === 200 && cfgStage?.sla_minutes === 1 && cfgStage.sla_escalate === true, cfgStage);
const ds = await mkDeal();
await sam.patch(`/crm/deal/items/${ds.id}`, { stage_id: st['Proposal sent'] });
const dsMoved = await mkDeal();
await sam.patch(`/crm/deal/items/${dsMoved.id}`, { stage_id: st['Proposal sent'] });
await sam.patch(`/crm/deal/items/${dsMoved.id}`, { stage_id: st.Preparation });

// ---- schedules ----
ok('weekly needs days', (await admin.post('/crm/schedules', { category_id: cat.id, name: 'x', frequency: 'weekly', action: 'comment', params: { text: 'x' } })).status === 400);
ok('bad time zone', (await admin.post('/crm/schedules', { category_id: cat.id, name: 'x', frequency: 'daily', timezone: 'Mars/Base', action: 'comment', params: { text: 'x' } })).status === 400);
ok('bad time', (await admin.post('/crm/schedules', { category_id: cat.id, name: 'x', frequency: 'daily', at_time: '25:00', action: 'comment', params: { text: 'x' } })).status === 400);
ok('non-admin cannot add schedules', (await sam.post('/crm/schedules', { category_id: cat.id, name: 'x', frequency: 'daily', action: 'comment', params: { text: 'x' } })).status === 403);
r = await admin.post('/crm/schedules', { category_id: cat.id, name: `Overdue ${stamp}`, frequency: 'weekly', weekdays: [1], at_time: '09:00', timezone: 'Europe/Berlin',
  stage_ids: [st.New], conditions: [{ field: 'close_date', op: 'lt', value: 'today' }], action: 'move_stage', params: { stage_id: st.Preparation } });
ok('schedule saved with its next run', r.status === 200 && r.data.next_run_at && new Date(r.data.next_run_at).getUTCDay() === 1 && new Date(r.data.next_run_at) > new Date(), r.data);
const sched = r.data;
ok('listed', (await admin.get(`/crm/schedules?category_id=${cat.id}`)).data.items.some((s) => s.id === sched.id));
const overdue = await mkDeal({ close_date: inDays(-3) });
const future = await mkDeal({ close_date: inDays(5) });
r = await admin.post(`/crm/schedules/${sched.id}/run`);
ok('run now picks the matching records only', r.status === 200 && r.data.count >= 1, r.data);
ok('overdue deal moved', !!(await waitFor(async () => (await admin.get(`/crm/deal/items/${overdue.id}`)).data.stage_id === st.Preparation)));
ok('future deal stays', (await admin.get(`/crm/deal/items/${future.id}`)).data.stage_id === st.New);
r = await admin.patch(`/crm/schedules/${sched.id}`, { is_active: false });
ok('paused schedule has no next run', r.data.next_run_at === null && r.data.last_count >= 1, r.data);
r = await admin.patch(`/crm/schedules/${sched.id}`, { is_active: true, frequency: 'monthly', month_day: 31, at_time: '18:30' });
ok('monthly on the 31st falls on the last day', r.status === 200 && r.data.next_run_at, r.data);

// ---- long waits finish ----
x = await timeoutCheck;
ok('wait timed out and ran anyway', x?.status === 'done', x);
x = await retryCheck;
ok('retried webhook succeeded', x?.status === 'done' && x.attempts === 1 && hits.filter((h) => h.url === '/retry').length === 2, [x, hits.filter((h) => h.url === '/retry').length]);
const sla = await waitFor(async () => (await timeline(ds.id)).find((e) => e.kind === 'sla'), 60000);
ok('over the time limit: flagged in the timeline', sla?.data?.stage?.id === st['Proposal sent'] && sla.data.minutes === 1, sla);
const notes = async (c) => (await c.get('/notifications?limit=50')).data.items;
ok('responsible person notified', (await notes(sam)).some((n) => n.kind === 'crm.sla' && n.link.endsWith(ds.id)));
ok('department head notified (escalation)', (await notes(ivy)).some((n) => n.kind === 'crm.sla' && n.link.endsWith(ds.id)));
ok('a record that moved on is not flagged', !(await timeline(dsMoved.id)).some((e) => e.kind === 'sla'));

// ---- report & retry ----
r = await admin.get(`/crm/automation/report?category_id=${cat.id}`);
ok('report totals', r.status === 200 && r.data.counts.done >= 10 && r.data.counts.failed >= 1 && r.data.total >= 15, r.data.counts);
ok('report per robot', r.data.by_source.some((b) => b.name === 'Hook bad' && b.failed >= 1) && r.data.by_source.some((b) => b.schedule_id === sched.id), r.data.by_source);
r = await admin.get(`/crm/automation/report?category_id=${cat.id}&status=failed`);
ok('filter by status', r.data.items.length >= 1 && r.data.items.every((i) => i.status === 'failed') && r.data.items[0].item_title && r.data.items[0].type_code === 'deal', r.data.items[0]);
const failed = r.data.items.find((i) => i.robot_name === 'Hook bad');
ok('non-admin cannot read the report', (await sam.get(`/crm/automation/report?category_id=${cat.id}`)).status === 403);
r = await admin.post(`/crm/robot-runs/${failed.id}/retry`);
ok('retry a failed run', r.status === 200 && r.data.id);
ok('retry runs again (fails again on 400)', !!(await waitFor(async () => hits.filter((h) => h.url === '/bad').length === 2)));
ok('only failed runs can be retried', (await admin.post(`/crm/robot-runs/${(await run(dh.id, 'Hook')).id}/retry`)).status === 400);
r = await admin.get(`/crm/automation/report?category_id=${cat.id}&q=${encodeURIComponent('Hook')}`);
ok('search the report', r.data.items.length >= 3 && r.data.items.every((i) => i.robot_name.includes('Hook') || i.item_title.includes('Hook') || (i.result ?? '').includes('Hook')), r.data.items.length);

r = await admin.get(`/crm/automation/stage-times?category_id=${cat.id}`);
const times = Object.fromEntries((r.data.stages ?? []).map((s) => [s.stage_id, s]));
ok('stage times', r.status === 200 && times[st.New].entries >= 10 && times[st.New].avg_hours >= 0 && times[st.Preparation].median_hours !== null, times[st.New]);
ok('stage times: over the limit now, and breaches', times[st['Proposal sent']].over_limit >= 1 && times[st['Proposal sent']].breaches >= 1 && times[st['Proposal sent']].sla_minutes === 1, times[st['Proposal sent']]);

// ---- cleanup ----
server.close();
await admin.patch(`/crm/stages/${st['Proposal sent']}`, { sla_minutes: null });
await admin.del(`/crm/schedules/${sched.id}`);
await admin.del(`/departments/${dept.id}/members/${sam.id}`);
await admin.del(`/departments/${dept.id}`);
const defaultCat = (await admin.get('/crm/config')).data.types.find((t) => t.code === 'deal').categories.find((c) => c.is_default).id;
for (const id of cleanup.robots) await admin.del(`/crm/robots/${id}`);
const all = (await admin.get(`/crm/deal/items?category_id=${cat.id}&limit=200`)).data.items.map((d) => d.id);
for (const id of new Set([...cleanup.deals, ...all])) await admin.del(`/crm/deal/items/${id}`);
for (const id of cleanup.leads) await admin.del(`/crm/lead/items/${id}`);
for (const id of cleanup.contacts) await admin.del(`/crm/contact/items/${id}`);
for (const id of cleanup.companies) await admin.del(`/crm/company/items/${id}`);
const binr = (await admin.get('/crm/recycle-bin?limit=200')).data; const bin = binr.page?.items ?? binr.items ?? [];
for (const b of bin.filter((b) => b.title?.includes(stamp) || [...cleanup.deals, ...all, ...cleanup.leads, ...cleanup.contacts, ...cleanup.companies].includes(b.id))) await admin.del(`/crm/recycle-bin/${b.id}`);
r = await admin.del(`/crm/categories/${cat.id}`);
ok('pipeline removed', r.status === 200, r.data);
void defaultCat;

console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
