// Lists and business processes: lists (access levels, fields, values checked, numbering,
// own items, CSV), processes (validation; starting on create, on change and by hand;
// conditions, approvals by the author's head and by named people, rejection, setting fields,
// notifications, tasks, waiting; the approvals inbox; cancelling; running processes keep
// their steps).
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Wf' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const hed = await user('Hed');
const fin = await user('Fin');
const out = await user('Out');
const notes = async (c: Client, text: string) => (await c.get('/notifications')).data.items?.filter((n: any) => n.title.includes(text)) ?? [];
const procOf = async (c: Client, item: string) => (await c.get(`/lists/items/${item}`)).data.processes[0];

try {
  const ops = (await admin.post('/departments', { name: `Ops ${stamp}`, head_user_id: hed.id })).data;
  for (const u of [ann, hed]) await admin.post(`/departments/${ops.id}/members`, { user_id: u.id });

  // ---- a list ----
  ok('only list managers make lists', (await ann.post('/lists', { name: `X ${stamp}` })).status === 403);
  ok('fields checked', (await admin.post('/lists', { name: `Bad ${stamp}`, fields: [{ label: 'Kind', kind: 'select' }] })).status === 400);
  let r = await admin.post('/lists', {
    name: `Purchases ${stamp}`, to_all: false,
    access: { add: { departments: [ops.id] }, read: { users: [fin.id] } },
    fields: [
      { label: 'Amount', kind: 'money', required: true },
      { label: 'Kind', kind: 'select', options: ['Hardware', 'Software'] },
      { label: 'Status', kind: 'select', options: ['New', 'Approved', 'Rejected'] },
      { label: 'Needed by', kind: 'date' },
      { label: 'Holder', kind: 'user' },
      { label: 'Urgent', kind: 'checkbox' },
      { label: 'Quote', kind: 'file' },
    ],
  });
  const list = r.data;
  ok('list with fields', r.status === 200 && list.fields.map((f: any) => f.code).join() === 'amount,kind,status,needed_by,holder,urgent,quote', r.data);
  ok('department members add items', (await ann.get(`/lists/${list.id}`)).data.level === 'add');
  ok('readers only read', (await fin.get(`/lists/${list.id}`)).data.level === 'read' && (await fin.post(`/lists/${list.id}/items`, { title: 'x', values: { amount: 1 } })).status === 403);
  ok('others don\'t see it', (await out.get(`/lists/${list.id}`)).status === 404 && !(await out.get('/lists')).data.items.some((l: any) => l.id === list.id));

  // ---- items ----
  const add = (c: Client, title: string, values: any) => c.post(`/lists/${list.id}/items`, { title, values });
  ok('required fields', (await add(ann, 'Laptop', { kind: 'Hardware' })).status === 400);
  ok('choices checked', (await add(ann, 'Laptop', { amount: 10, kind: 'Car' })).status === 400);
  ok('unknown fields refused', (await add(ann, 'Laptop', { amount: 10, colour: 'red' })).status === 400);
  ok('dates checked', (await add(ann, 'Laptop', { amount: 10, needed_by: 'soon' })).status === 400);
  const fd = new FormData(); fd.append('file', new Blob(['quote']), `quote-${stamp}.txt`);
  const theirs = (await hed.req('POST', '/files', fd)).data;
  ok('files must be your own uploads', (await add(ann, 'Laptop', { amount: 10, quote: theirs.id })).status === 400);
  r = await add(ann, `Monitor ${stamp}`, { amount: '199,5', kind: 'Hardware', holder: ann.id, urgent: true });
  const small = r.data;
  ok('item added, values cleaned', r.status === 200 && small.number === 1 && small.values.amount === '199.50' && small.values.urgent === true && small.labels.holder === 'Ann Wf', r.data);
  ok('authors change their own items', (await ann.patch(`/lists/items/${small.id}`, { title: `Monitors ${stamp}` })).data.title === `Monitors ${stamp}`);
  ok('others with add rights don\'t', (await hed.patch(`/lists/items/${small.id}`, { title: 'x' })).status === 403);
  ok('managers do', (await admin.patch(`/lists/items/${small.id}`, { values: { kind: 'Software' } })).data.values.kind === 'Software');
  const csv = await (await admin.req('GET', `/lists/${list.id}/export`, undefined, true)).text();
  ok('CSV', csv.includes('"Amount"') && csv.includes(`"Monitors ${stamp}"`) && csv.includes('"199.50"'), csv.slice(0, 300));
  ok('search', (await fin.get(`/lists/${list.id}/items?q=monitors ${stamp}`)).data.total === 1);

  // ---- a process ----
  const steps = [{
    kind: 'condition', field: 'amount', op: 'gt', value: '1000',
    then: [{
      kind: 'approval', title: 'Approve {{title}} ({{amount}})', who: { head: true },
      approved: [
        { kind: 'approval', title: 'Finance: {{title}}', who: { users: [fin.id] }, approved: [{ kind: 'set_field', field: 'status', value: 'Approved' }, { kind: 'task', title: 'Order {{title}}', who: { users: [fin.id] }, deadline_hours: 48 }], rejected: [] },
      ],
      rejected: [{ kind: 'set_field', field: 'status', value: 'Rejected' }, { kind: 'notify', text: 'Rejected: {{title}}', who: { author: true } }, { kind: 'stop', status: 'rejected' }],
    }],
    else: [{ kind: 'set_field', field: 'status', value: 'Approved' }],
  }];
  ok('only managers make processes', (await ann.post(`/lists/${list.id}/workflows`, { name: 'x', steps })).status === 403);
  ok('steps checked', (await admin.post(`/lists/${list.id}/workflows`, { name: 'x', steps: [{ kind: 'set_field', field: 'nope', value: '1' }] })).status === 400);
  r = await admin.post(`/lists/${list.id}/workflows`, { name: `Purchase approval ${stamp}`, trigger: 'create', steps });
  const wf = r.data;
  ok('process saved', r.status === 200 && wf.steps[0].then[0].mode === 'any', r.data);

  // Small amount: approved at once.
  const cheap = (await add(ann, `Mouse ${stamp}`, { amount: 20 })).data;
  ok('small amounts are approved at once', cheap.values.status === 'Approved' && cheap.processes[0].status === 'completed', cheap.processes);

  // Large amount: the head, then finance.
  const big = (await add(ann, `Server ${stamp}`, { amount: 5000, kind: 'Hardware' })).data;
  ok('waiting for the head', big.processes[0].status === 'waiting' && big.process_status === 'waiting');
  ok('the head is asked', !!(await waitFor(async () => (await notes(hed, 'Please approve: Approve Server')).at(0), 5000)));
  let inbox = (await hed.get('/approvals')).data.items;
  ok('in the head\'s inbox, texts filled in', inbox.length === 1 && inbox[0].title === `Approve Server ${stamp} (5000.00)` && inbox[0].author === 'Ann Wf', inbox);
  ok('not in others\'', (await fin.get('/approvals')).data.items.length === 0 && (await ann.req('PATCH', `/approvals/${inbox[0].id}`, { approve: true })).status === 404);
  await hed.req('PATCH', `/approvals/${inbox[0].id}`, { approve: true, comment: 'Fine' });
  ok('decided once', (await hed.req('PATCH', `/approvals/${inbox[0].id}`, { approve: true })).status === 400);
  inbox = (await fin.get('/approvals')).data.items;
  ok('then finance', inbox.length === 1 && inbox[0].title === `Finance: Server ${stamp}`);
  await fin.req('PATCH', `/approvals/${inbox[0].id}`, { approve: true });
  let p = await procOf(ann, big.id);
  ok('completed', p.status === 'completed' && (await ann.get(`/lists/items/${big.id}`)).data.values.status === 'Approved', p);
  ok('the log tells the story', p.log.some((e: any) => e.text === 'Hed Wf approved: “Fine”') && p.log.some((e: any) => e.text.startsWith('Task “Order Server')), p.log);
  ok('finance got the task', (await fin.get('/tasks?role=responsible')).data.items?.some((t: any) => t.title === `Order Server ${stamp}`) || (await fin.get(`/tasks?q=Order Server ${stamp}`)).data.items?.length === 1);
  ok('the author hears it is complete', !!(await waitFor(async () => (await notes(ann, `is complete: Server ${stamp}`)).at(0), 5000)));

  // Rejected by the head.
  const no = (await add(ann, `Yacht ${stamp}`, { amount: 900000 })).data;
  inbox = (await hed.get('/approvals')).data.items;
  await hed.req('PATCH', `/approvals/${inbox.find((a: any) => a.title.includes('Yacht')).id}`, { approve: false, comment: 'No' });
  p = await procOf(ann, no.id);
  ok('rejected: field set, author told, ends as rejected', p.status === 'rejected' && (await ann.get(`/lists/items/${no.id}`)).data.values.status === 'Rejected' && !!(await waitFor(async () => (await notes(ann, `Rejected: Yacht ${stamp}`)).at(0), 5000)), p);

  // Running processes keep their steps.
  const pending = (await add(ann, `Desk ${stamp}`, { amount: 2000 })).data;
  await admin.patch(`/workflows/${wf.id}`, { steps: [{ kind: 'stop', status: 'rejected' }] });
  inbox = (await hed.get('/approvals')).data.items;
  await hed.req('PATCH', `/approvals/${inbox.find((a: any) => a.title.includes('Desk')).id}`, { approve: true });
  ok('the running process still goes to finance', (await fin.get('/approvals')).data.items.some((a: any) => a.title === `Finance: Desk ${stamp}`));
  ok('only the starter or a manager cancels', (await fin.post(`/workflows/instances/${(await procOf(ann, pending.id)).id}/cancel`, {})).status === 403);
  r = await ann.post(`/workflows/instances/${(await procOf(ann, pending.id)).id}/cancel`, {});
  ok('cancelled; its approval goes away', r.data[0].status === 'cancelled' && !(await fin.get('/approvals')).data.items.some((a: any) => a.title === `Finance: Desk ${stamp}`));

  // ---- by hand, on change, waiting ----
  await admin.patch(`/workflows/${wf.id}`, { is_active: false });
  const manual = (await admin.post(`/lists/${list.id}/workflows`, { name: `Check ${stamp}`, trigger: 'manual', steps: [{ kind: 'wait', hours: 0.001 }, { kind: 'set_field', field: 'kind', value: 'Software' }] })).data;
  const item = (await add(ann, `Licence ${stamp}`, { amount: 50 })).data;
  ok('nothing starts on its own', item.processes.length === 0);
  ok('others can\'t start it on Ann\'s item', (await hed.post(`/lists/items/${item.id}/start/${manual.id}`, {})).status === 403);
  r = await ann.post(`/lists/items/${item.id}/start/${manual.id}`, {});
  ok('started by hand; waiting', r.data[0].status === 'waiting', r.data);
  ok('not twice at once', (await ann.post(`/lists/items/${item.id}/start/${manual.id}`, {})).status === 400);
  ok('after the wait it goes on', !!(await waitFor(async () => (await ann.get(`/lists/items/${item.id}`)).data.values.kind === 'Software', 30000)));
  await admin.post(`/lists/${list.id}/workflows`, { name: `On change ${stamp}`, trigger: 'update', steps: [{ kind: 'notify', text: 'Changed: {{title}}', who: { users: [fin.id] } }] });
  await ann.patch(`/lists/items/${item.id}`, { values: { urgent: true } });
  ok('starts when an item changes', !!(await waitFor(async () => (await notes(fin, `Changed: Licence ${stamp}`)).at(0), 5000)));

  // ---- deleting ----
  ok('authors delete their items', (await ann.req('DELETE', `/lists/items/${cheap.id}`)).status === 200);
  ok('deleting the list', (await admin.req('DELETE', `/lists/${list.id}`)).status === 200 && (await ann.get(`/lists/${list.id}`)).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
