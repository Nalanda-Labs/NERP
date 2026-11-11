// Sales generator: a deal (or lead) for every client in a segment, to the client's responsible
// person or chosen people in turn, skipping clients with an open one, now or on a repeat.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const waitFor = async <T>(f: () => Promise<T | null | undefined | false>, ms = 20000): Promise<T | null> => { const end = Date.now() + ms; while (Date.now() < end) { const v = await f(); if (v) return v as T; await sleep(400); } return null; };
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Sg${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const made: string[] = [];
let catId = '', segId = '';

try {
  const ann = await mk('Ann'), bob = await mk('Bob'), cat = await mk('Cat');
  const contacts: any[] = [];
  for (const [i, owner] of [ann, bob, ann].entries()) contacts.push((await admin.post('/crm/contact/items', { first_name: `Client${i}`, last_name: `Gen${stamp}`, assigned_to: owner.id })).data);
  made.push(...contacts.map((c) => c.id));
  const seg = (await admin.post('/marketing/segments', { name: `Clients ${stamp}`, entity_type: 3, filter: { q: `gen${stamp}` } })).data;
  segId = seg.id;
  const leadSeg = (await admin.post('/marketing/segments', { name: `Leads ${stamp}`, entity_type: 1, filter: { q: `gen${stamp}` } })).data;
  const pipe = (await admin.post('/crm/deal/categories', { name: `Renewals ${stamp}` })).data.categories.find((c: any) => c.name === `Renewals ${stamp}`);
  catId = pipe.id;
  const stage = pipe.stages.find((s: any) => s.semantics === 'process' && s.sort > pipe.stages[0].sort) ?? pipe.stages[1];
  // Client 2 already has an open deal in the pipeline.
  const open = (await admin.post('/crm/deal/items', { title: `Open ${stamp}`, category_id: pipe.id, contact_ids: [contacts[2].id] })).data;
  made.push(open.id);

  ok('only marketers make generators', (await ann.post('/marketing/generators', { name: 'x', segment_id: seg.id })).status === 403);
  ok('segments of contacts or companies only', (await admin.post('/marketing/generators', { name: 'x', segment_id: leadSeg.id })).status === 400);
  ok('a stage must be in the pipeline', (await admin.post('/marketing/generators', { name: 'x', segment_id: seg.id, category_id: pipe.id, stage_id: '00000000-0000-4000-8000-000000000000' })).status === 400);
  let r = await admin.post('/marketing/generators', { name: `Renewal ${stamp}`, segment_id: seg.id, target: 'deal', category_id: pipe.id, stage_id: stage.id, title: '{{name}}: renewal {{year}}', repeat: 'monthly' });
  ok('a monthly generator, running now', r.status === 200 && r.data.repeat === 'monthly' && r.data.next_run_at, r.data);
  const gen = r.data;
  const done = await waitFor(async () => { const g = (await admin.get(`/marketing/generators/${gen.id}`)).data; return g.last_result ? g : null; });
  ok('it ran: two deals, one client skipped', done?.last_result?.created === 2 && done.last_result.skipped === 1, done);
  ok('…and is due again in a month', done && new Date(done.next_run_at).getTime() - Date.now() > 27 * 86400e3, done?.next_run_at);
  const deals = (await admin.get(`/crm/deal/items?category_id=${pipe.id}&limit=50`)).data.items.filter((d: any) => d.title.includes('renewal'));
  made.push(...deals.map((d: any) => d.id));
  ok('deals titled from the template, in the stage', deals.length === 2 && deals.every((d: any) => d.stage_id === stage.id && /^Client[01] Gen.+: renewal \d{4}$/.test(d.title)), deals.map((d: any) => [d.title, d.stage_id]));
  const d0 = deals.find((d: any) => d.title.startsWith('Client0')), d1 = deals.find((d: any) => d.title.startsWith('Client1'));
  ok('each to the client\'s responsible person', d0?.assigned_to === ann.id && d1?.assigned_to === bob.id, deals);
  const det = (await admin.get(`/crm/deal/items/${d0.id}`)).data;
  ok('…linked to the client', det.contacts.some((c: any) => c.id === contacts[0].id), det.contacts);
  ok('the generator lists what it made', done?.recent.length === 2 && done.made === 2);

  // Run again by hand: everyone has an open deal now; with skipping off, people in turn.
  r = await admin.get(`/marketing/generators/${gen.id}/preview`);
  ok('the preview: nothing to make', r.data.would_make === 0 && r.data.would_skip === 3, r.data);
  await admin.patch(`/marketing/generators/${gen.id}`, { skip_open: false, assign: 'users', user_ids: [cat.id, bob.id] });
  await admin.post(`/marketing/generators/${gen.id}/run`);
  const again = await waitFor(async () => { const g = (await admin.get(`/marketing/generators/${gen.id}`)).data; return g.made === 5 ? g : null; });
  ok('run by hand: three more, to the chosen people in turn', !!again && again.last_result.manual === true, again);
  const after = (await admin.get(`/crm/deal/items?category_id=${pipe.id}&limit=50`)).data.items.filter((d: any) => d.title.includes('renewal') && !made.includes(d.id));
  made.push(...after.map((d: any) => d.id));
  const owners = after.map((d: any) => d.assigned_to);
  ok('…Cat, Bob, Cat', owners.filter((o: string) => o === cat.id).length === 2 && owners.filter((o: string) => o === bob.id).length === 1, owners);
  ok('running by hand keeps the schedule', again?.next_run_at === done?.next_run_at, [again?.next_run_at, done?.next_run_at]);

  // Leads, once, later.
  const at = new Date(Date.now() + 86400e3).toISOString();
  r = await admin.post('/marketing/generators', { name: `Leads ${stamp}`, segment_id: seg.id, target: 'lead', title: 'Offer for {{name}}', start_at: at });
  ok('a lead generator for tomorrow', r.status === 200 && r.data.target_code === 'lead' && new Date(r.data.next_run_at).getTime() === new Date(at).getTime() && !r.data.last_run_at, r.data);
  r = await admin.patch(`/marketing/generators/${r.data.id}`, { is_active: false });
  ok('paused: nothing scheduled', r.data.next_run_at === null && !r.data.is_active, r.data);
  ok('deleting it', (await admin.del(`/marketing/generators/${r.data.id}`)).status === 200);
} finally {
  for (const id of made) await admin.del(`/crm/deal/items/${id}`).catch(() => {});
  for (const id of made) await admin.del(`/crm/contact/items/${id}`).catch(() => {});
  if (segId) await admin.del(`/marketing/segments/${segId}`).catch(() => {});
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
