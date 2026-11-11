// Process automation on a smart process (RPA): each stage hands its people tasks (robots), and
// the record moves on by itself once all its tasks are done (the "tasks done" trigger), through
// to the end.
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
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Rpa${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
let code = '';

try {
  const bob = await mk('Bob'), cat = await mk('Cat');
  const T = (await admin.post('/crm/types', { name: `Purchase ${stamp}`, name_plural: `Purchases ${stamp}`, icon: 'file' })).data;
  code = T.code;
  const cfg = (await admin.get('/crm/config')).data.types.find((t: any) => t.code === code);
  const cat0 = cfg.categories[0];
  const proc = cat0.stages.filter((s: any) => s.semantics === 'process');
  const won = cat0.stages.find((s: any) => s.semantics === 'success');
  ok('a smart process with stages', proc.length >= 2 && !!won, cat0.stages.map((s: any) => s.name));
  const [s1, s2] = proc;
  // Stage 1: Bob checks the budget. Stage 2: Cat orders. Each stage moves on when its tasks are done.
  await admin.post('/crm/robots', { category_id: cat0.id, stage_id: s1.id, name: `Budget ${stamp}`, action: 'task', params: { title: 'Check the budget for {{title}}', responsible: bob.id } });
  await admin.post('/crm/robots', { category_id: cat0.id, stage_id: s2.id, name: `Order ${stamp}`, action: 'task', params: { title: 'Order {{title}}', responsible: cat.id } });
  let r = await admin.post('/crm/triggers', { category_id: cat0.id, stage_id: s2.id, name: `Budget checked ${stamp}`, event: 'tasks_done' });
  ok('a trigger for "all its tasks are done"', r.status === 200, r.data);
  await admin.post('/crm/triggers', { category_id: cat0.id, stage_id: won.id, name: `Ordered ${stamp}`, event: 'tasks_done' });

  const rec = (await admin.post(`/crm/${code}/items`, { title: `Laptops ${stamp}`, stage_id: s1.id })).data;
  const bobTask = await waitFor(async () => (await bob.get(`/tasks?q=${encodeURIComponent(`Laptops ${stamp}`)}`)).data.items?.[0]);
  ok('stage 1 gives Bob his task', !!bobTask && bobTask.title === `Check the budget for Laptops ${stamp}`, bobTask);
  await bob.post(`/tasks/${bobTask.id}/status`, { status: 'done' });
  const moved = await waitFor(async () => (await admin.get(`/crm/${code}/items/${rec.id}`)).data.stage_id === s2.id);
  ok('Bob done: the record moves on by itself', !!moved);
  const catTask = await waitFor(async () => (await cat.get(`/tasks?q=${encodeURIComponent(`Order Laptops ${stamp}`)}`)).data.items?.[0]);
  ok('…and stage 2 gives Cat hers', !!catTask, catTask);
  await cat.post(`/tasks/${catTask.id}/status`, { status: 'done' });
  const finished = await waitFor(async () => (await admin.get(`/crm/${code}/items/${rec.id}`)).data.stage_id === won.id);
  ok('Cat done: the record is through', !!finished);
  const tl = (await admin.get(`/crm/items/${rec.id}/timeline`)).data.items;
  ok('the history shows the tasks and the moves', tl.filter((e: any) => e.kind === 'trigger').length === 2 && tl.some((e: any) => e.kind === 'task'), tl.map((e: any) => e.kind));
} finally {
  if (code) {
    for (const x of (await admin.get(`/crm/${code}/items?limit=50`)).data.items ?? []) await admin.del(`/crm/${code}/items/${x.id}`);
    await admin.del(`/crm/types/${code}`).catch(() => {});
  }
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
