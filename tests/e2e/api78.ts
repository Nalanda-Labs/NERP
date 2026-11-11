// Workflow depth: a request started from the feed (posted to the author and its approvers,
// who decide from the post), a process that waits for the tasks it made, test mode (only
// the testers' items start the process), and the process log over a period.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const waitFor = async <T>(f: () => Promise<T | null | undefined | false>, ms = 15000): Promise<T | null> => { const end = Date.now() + ms; while (Date.now() < end) { const v = await f(); if (v) return v as T; await sleep(300); } return null; };
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Wf${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [bob, cat, dan, eve] = await Promise.all(['Bob', 'Cat', 'Dan', 'Eve'].map(user));

try {
  // ---- a list whose process asks Cat, then gives Dan a task and waits for it ----
  const list = (await admin.post('/lists', { name: `Purchases ${stamp}`, to_all: true, fields: [{ label: 'Amount', kind: 'number', required: true }, { label: 'Status', kind: 'select', options: ['New', 'Bought'] }] })).data;
  const fields = (await admin.get(`/lists/${list.id}`)).data.fields;
  const amount = fields.find((f: any) => f.label === 'Amount').code, status = fields.find((f: any) => f.label === 'Status').code;
  let r = await admin.post(`/lists/${list.id}/workflows`, {
    name: `Buy ${stamp}`, trigger: 'create', steps: [
      { kind: 'approval', title: 'Approve {{title}}', who: { users: [cat.id] }, mode: 'any', approved: [
        { kind: 'task', who: { users: [dan.id] }, title: 'Buy {{title}}', wait: true },
        { kind: 'set_field', field: status, value: 'Bought' },
      ], rejected: [] },
    ],
  });
  const wf = r.data;
  ok('a process whose task step waits', r.status === 200 && wf.steps[0].approved[0].wait === true, r.data);

  // ---- a request from the feed ----
  r = await bob.get('/feed/processes');
  ok('the feed offers the processes one may start', r.data.items.some((p: any) => p.id === list.id), r.data);
  ok('…values are checked like the list\'s own form', (await bob.post('/feed/requests', { list_id: list.id, title: `Laptop ${stamp}`, values: {} })).status === 400);
  r = await bob.post('/feed/requests', { list_id: list.id, title: `Laptop ${stamp}`, values: { [amount]: 1200 }, note: `Need a laptop ${stamp}` });
  const post = r.data;
  ok('a request posted to the feed', r.status === 200 && post.request?.item_id && post.body === `Need a laptop ${stamp}` && post.to_all === false, r.data);
  const seen = async (c: Client) => JSON.stringify((await c.get('/feed')).data).includes(`Need a laptop ${stamp}`);
  ok('the approver sees it', await seen(cat));
  ok('…others don\'t', !(await seen(eve)));
  const catPost = (await cat.get(`/feed/${post.id}`)).data;
  ok('…with her approval waiting on it', catPost.request?.approvals?.length === 1 && catPost.request.status === 'waiting', catPost.request);
  ok('Bob has nothing to decide on his own request', (await bob.get(`/feed/${post.id}`)).data.request.approvals.length === 0);
  r = await cat.patch(`/approvals/${catPost.request.approvals[0].id}`, { approve: true });
  ok('she approves from the post', r.status === 200, r.data);

  // ---- the process waits for the task ----
  const task = await waitFor(async () => ((await dan.get(`/tasks?q=${encodeURIComponent(`Laptop ${stamp}`)}`)).data.items ?? []).find((t: any) => t.title === `Buy Laptop ${stamp}`));
  ok('Dan gets the task', !!task);
  await sleep(500);
  const itemOf = async () => (await admin.get(`/lists/items/${post.request.item_id}`)).data;
  ok('the process waits (the field is not set yet)', ((await itemOf()).values ?? {})[status] !== 'Bought', (await itemOf()).values);
  ok('…the post says it is waiting', (await bob.get(`/feed/${post.id}`)).data.request.status === 'waiting');
  await dan.post(`/tasks/${(task as any).id}/status`, { status: 'done' });
  const done = await waitFor(async () => ((await itemOf()).values ?? {})[status] === 'Bought');
  ok('when the task is done, the process carries on and ends', !!done && (await bob.get(`/feed/${post.id}`)).data.request.status === 'completed', (await itemOf()).values);

  // ---- test mode ----
  r = await admin.patch(`/workflows/${wf.id}`, { test_users: [eve.id] });
  ok('testers are set', r.status === 200 && r.data.test_users[0] === eve.id, r.data);
  ok('…only active employees', (await admin.patch(`/workflows/${wf.id}`, { test_users: ['00000000-0000-4000-8000-000000000000'] })).status === 400);
  ok('while testing, the feed doesn\'t offer it to others', !(await bob.get('/feed/processes')).data.items.some((p: any) => p.id === list.id));
  const bobItem = (await bob.post(`/lists/${list.id}/items`, { title: `Pens ${stamp}`, values: { [amount]: 5 } })).data;
  const eveItem = (await eve.post(`/lists/${list.id}/items`, { title: `Desk ${stamp}`, values: { [amount]: 300 } })).data;
  await sleep(500);
  const runs = async (id: string) => (await admin.get(`/lists/items/${id}`)).data.processes?.length ?? 0;
  ok('someone else\'s item doesn\'t start it', (await runs(bobItem.id)) === 0, await runs(bobItem.id));
  ok('a tester\'s item does', (await runs(eveItem.id)) === 1, await runs(eveItem.id));

  // ---- the log over a period ----
  const now = Date.now();
  r = await admin.get(`/workflows/${wf.id}/log?from=${new Date(now - 3600e3).toISOString()}&to=${new Date(now + 60e3).toISOString()}`);
  ok('the process log over a period, every run', r.status === 200 && r.data.items.some((e: any) => e.text === 'The tasks are done') && r.data.items.some((e: any) => e.item_title === `Desk ${stamp}`), r.data.items?.slice(0, 5));
  r = await admin.get(`/workflows/${wf.id}/log?from=2020-01-01T00:00:00Z&to=2020-02-01T00:00:00Z`);
  ok('…nothing outside it', r.data.items.length === 0);
  ok('only the list\'s managers read it', (await bob.get(`/workflows/${wf.id}/log`)).status === 403);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
