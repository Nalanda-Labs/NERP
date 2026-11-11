// Automation test mode: while on, a pipeline's robots and triggers run only for records whose
// responsible person is a tester, and those runs are marked as tests; off, everyone's again.
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
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Tm${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const runs = async (id: string) => (await admin.get(`/crm/items/${id}/automation`)).data.items as any[];
let catId = '';

try {
  const tess = await mk('Tess'), ron = await mk('Ron');
  let r = await admin.post('/crm/deal/categories', { name: `Testing ${stamp}` });
  const cat = r.data.categories.find((c: any) => c.name === `Testing ${stamp}`);
  catId = cat.id;
  const st = Object.fromEntries(cat.stages.map((s: any) => [s.name, s.id]));
  await admin.post('/crm/robots', { category_id: cat.id, stage_id: st.Preparation, name: `Hello ${stamp}`, action: 'comment', params: { text: `Robot says hi ${stamp}` } });
  await admin.post('/crm/triggers', { category_id: cat.id, stage_id: st['Proposal sent'], name: `On comment ${stamp}`, event: 'comment_added' });

  ok('test mode starts off', (await admin.get(`/crm/automation?category_id=${cat.id}`)).data.test_mode?.enabled === false);
  ok('turning it on needs testers', (await admin.put('/crm/automation/test-mode', { category_id: cat.id, enabled: true, user_ids: [] })).status === 400);
  ok('only CRM admins switch it', (await tess.put('/crm/automation/test-mode', { category_id: cat.id, enabled: true, user_ids: [tess.id] })).status === 403);
  r = await admin.put('/crm/automation/test-mode', { category_id: cat.id, enabled: true, user_ids: [tess.id] });
  ok('test mode on for Tess', r.status === 200 && r.data.enabled, r.data);
  const ov = (await admin.get(`/crm/automation?category_id=${cat.id}`)).data.test_mode;
  ok('…and shown in the overview', ov.enabled && ov.user_ids.includes(tess.id), ov);

  const deal = async (who: Client) => (await admin.post('/crm/deal/items', { title: `D ${stamp} ${who.id.slice(0, 4)}`, category_id: cat.id, stage_id: st.Preparation, assigned_to: who.id })).data.id as string;
  const mine = await deal(tess), other = await deal(ron);
  const run = await waitFor(async () => (await runs(mine)).find((x) => x.robot_name === `Hello ${stamp}` && x.status === 'done'));
  ok('a tester\'s record runs the robots', !!run);
  ok('…marked as a test run', run?.is_test === true, run);
  await sleep(1500);
  ok('everyone else\'s records are left alone', (await runs(other)).length === 0, await runs(other));

  await admin.post(`/crm/items/${mine}/timeline`, { body: 'hi' });
  await admin.post(`/crm/items/${other}/timeline`, { body: 'hi' });
  const moved = await waitFor(async () => (await admin.get(`/crm/deal/items/${mine}`)).data.stage_id === st['Proposal sent']);
  ok('triggers move the tester\'s record', !!moved);
  ok('…but not the others', (await admin.get(`/crm/deal/items/${other}`)).data.stage_id === st.Preparation);

  r = await admin.put('/crm/automation/test-mode', { category_id: cat.id, enabled: false });
  ok('test mode off', r.status === 200 && !r.data.enabled);
  const later = await deal(ron);
  const real = await waitFor(async () => (await runs(later)).find((x) => x.status === 'done'));
  ok('then the robots run for everyone, as real runs', real?.is_test === false, real);

  const log = (await admin.get(`/crm/automation/report?category_id=${cat.id}`)).data;
  const rows = log.items ?? log.runs ?? [];
  ok('the automation log tells test runs apart', rows.some((x: any) => x.is_test) && rows.some((x: any) => !x.is_test), log);
} finally {
  if (catId) {
    for (const d of (await admin.get(`/crm/deal/items?category_id=${catId}&limit=50`)).data.items ?? []) await admin.del(`/crm/deal/items/${d.id}`);
  }
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
