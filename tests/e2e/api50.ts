// Task depth: checklist points given to people, rating finished tasks, a required result,
// new comments since I last looked, my subordinates' tasks, my planner, custom fields,
// deadlines off weekends and holidays, a deadline window, and the recycle bin.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Depth${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat] = await Promise.all(['Ann', 'Bob', 'Cat'].map(user));
const err = (r: any) => r.data?.error?.message ?? '';

try {
  // ---- checklist points for people ----
  let t = (await ann.post('/tasks', { title: `Launch ${stamp}`, responsible_id: bob.id, checklist: ['Write notes'] })).data;
  let r = await ann.post(`/tasks/${t.id}/checklist`, { title: 'Check the site', assignee_id: cat.id });
  t = (await ann.get(`/tasks/${t.id}`)).data;
  const item = t.checklist.find((c: any) => c.title === 'Check the site');
  ok('a checklist point given to someone', item?.assignee_id === cat.id && item.assignee_name.startsWith('Cat'), t.checklist);
  ok('who joins the task as an assistant', t.people.some((p: any) => p.user_id === cat.id && p.role === 'assistant'));
  ok('and can tick it', (await cat.patch(`/tasks/${t.id}/checklist/${item.id}`, { is_done: true })).status === 200);
  await ann.patch(`/tasks/${t.id}/checklist/${t.checklist[0].id}`, { assignee_id: bob.id });
  ok('a point handed to someone else', (await ann.get(`/tasks/${t.id}`)).data.checklist[0].assignee_id === bob.id);

  // ---- result and rating ----
  await ann.patch(`/tasks/${t.id}`, { requires_result: true });
  r = await bob.post(`/tasks/${t.id}/status`, { status: 'done' });
  ok('a task needing a result can\'t be finished without one', r.status === 400 && /result/.test(err(r)), r.data);
  const c1 = (await bob.post(`/tasks/${t.id}/comments`, { body: `Launched at 9:00 ${stamp}` })).data;
  r = await bob.put(`/tasks/${t.id}/result`, { comment_id: c1.id ?? c1.comment?.id });
  ok('mark a comment as the result', r.status === 200 && r.data.result?.body === `Launched at 9:00 ${stamp}`, r.data.result ?? r.data);
  ok('then it can be finished', (await bob.post(`/tasks/${t.id}/status`, { status: 'done' })).data.status === 'done');
  ok('only the creator rates', (await bob.post(`/tasks/${t.id}/rate`, { rating: 'good' })).status === 400);
  r = await ann.post(`/tasks/${t.id}/rate`, { rating: 'good' });
  ok('the creator rates a finished task', r.data.rating === 'good');
  const open = (await ann.post('/tasks', { title: `Open ${stamp}`, responsible_id: bob.id })).data;
  ok('not an unfinished one', (await ann.post(`/tasks/${open.id}/rate`, { rating: 'bad' })).status === 400);

  // ---- new comments ----
  await bob.get(`/tasks/${open.id}`);
  await ann.post(`/tasks/${open.id}/comments`, { body: 'Any news?' });
  r = await bob.get(`/tasks?view=my&unread=true`);
  ok('tasks with comments I haven\'t seen', r.data.items.some((x: any) => x.id === open.id && x.new_comments === 1), r.data.items.map((x: any) => [x.title, x.new_comments]));
  await bob.get(`/tasks/${open.id}`);
  ok('opening the task reads them', !(await bob.get(`/tasks?view=my&unread=true`)).data.items.some((x: any) => x.id === open.id));
  ok('my own comments aren\'t new to me', (await ann.get(`/tasks?view=created`)).data.items.find((x: any) => x.id === open.id)?.new_comments === 0);

  // ---- supervising ----
  const dept = (await admin.post('/departments', { name: `Team ${stamp}`, head_user_id: cat.id })).data;
  const subd = (await admin.post('/departments', { name: `Sub ${stamp}`, parent_id: dept.id })).data;
  await admin.post(`/departments/${subd.id}/members`, { user_id: bob.id });
  r = await cat.get('/tasks?view=supervising&status=new,in_progress,review');
  ok('a head sees their people\'s tasks (sub-departments too)', r.data.items.some((x: any) => x.id === open.id), r.data.items.map((x: any) => x.title));
  ok('others have no one to supervise', !(await ann.get('/tasks?view=supervising')).data.items.some((x: any) => x.id === open.id));

  // ---- planner ----
  r = await bob.get('/tasks/planner');
  const [now, , later] = r.data.stages;
  ok('my planner starts with three columns, my open tasks in the first', r.data.stages.length === 3 && r.data.tasks.find((x: any) => x.id === open.id)?.planner_stage_id === now.id, r.data);
  await bob.post(`/tasks/${open.id}/planner`, { stage_id: later.id });
  ok('move a task to "Later" (only in my planner)', (await bob.get('/tasks/planner')).data.tasks.find((x: any) => x.id === open.id)?.planner_stage_id === later.id && (await bob.get(`/tasks/${open.id}`)).data.status === 'new');
  r = await bob.post('/tasks/planner/stages', { name: 'Waiting', color: 'amber' });
  ok('my own columns', r.data.stages.length === 4 && r.data.stages[3].name === 'Waiting');
  ok('nobody else\'s', (await ann.patch(`/tasks/planner/stages/${later.id}`, { name: 'x' })).status === 404);
  await bob.del(`/tasks/planner/stages/${later.id}`);
  ok('deleting a column sends its cards to the first', (await bob.get('/tasks/planner')).data.tasks.find((x: any) => x.id === open.id)?.planner_stage_id === now.id);

  // ---- custom fields ----
  ok('company-wide fields are for administrators', (await ann.post('/tasks/fields', { name: `Client ${stamp}`, kind: 'text' })).status === 403);
  r = await admin.post('/tasks/fields', { name: `Size ${stamp}`, kind: 'select', options: ['S', 'M', 'L'], required: true });
  const size = r.data.items.find((f: any) => f.name === `Size ${stamp}`);
  ok('a required list field', size?.kind === 'select' && size.options.length === 3, r.data);
  ok('a list field needs choices', (await admin.post('/tasks/fields', { name: `Bad ${stamp}`, kind: 'select' })).status === 400);
  const proj = (await ann.post('/projects', { name: `Proj ${stamp}` })).data;
  r = await ann.post('/tasks/fields', { name: `Budget ${stamp}`, kind: 'number', project_id: proj.id });
  const budget = r.data.items.find((f: any) => f.name === `Budget ${stamp}`);
  ok('a project owner adds fields for its tasks', !!budget, r.data);
  r = await ann.post('/tasks', { title: `Fielded ${stamp}`, project_id: proj.id, fields: { [budget.id]: '1500' } });
  ok('a required field must be filled when the form sends fields', r.status === 400 && new RegExp(`Size ${stamp}`).test(err(r)), r.data);
  r = await ann.post('/tasks', { title: `Fielded ${stamp}`, project_id: proj.id, fields: { [budget.id]: '1500', [size.id]: 'M' } });
  const ft = r.data;
  ok('values are kept and typed', ft.fields?.find((f: any) => f.id === budget.id)?.value === 1500 && ft.fields.find((f: any) => f.id === size.id)?.value === 'M', ft.fields);
  ok('a value outside the list is refused', (await ann.put(`/tasks/${ft.id}/fields`, { values: { [size.id]: 'XXL' } })).status === 400);
  ok('a project\'s field doesn\'t apply elsewhere', (await ann.put(`/tasks/${open.id}/fields`, { values: { [budget.id]: 3 } })).status === 400);
  ok('other tasks only show the company-wide ones', (await ann.get(`/tasks/${open.id}`)).data.fields.every((f: any) => f.project_id === null));

  // ---- working days ----
  const sat = new Date(); sat.setUTCDate(sat.getUTCDate() + ((6 - sat.getUTCDay() + 7) % 7 || 7)); sat.setUTCHours(15, 0, 0, 0);
  r = await ann.post('/tasks', { title: `Weekend ${stamp}`, deadline: sat.toISOString(), skip_nonworking: true });
  ok('a Saturday deadline moves to Monday', new Date(r.data.deadline).getUTCDay() === 1 && new Date(r.data.deadline).getUTCHours() === 15, r.data.deadline);
  r = await ann.patch(`/tasks/${r.data.id}`, { deadline: new Date(+sat + 86400e3).toISOString() });
  ok('and so does a Sunday one set later', new Date(r.data.deadline).getUTCDay() === 1, r.data.deadline);
  ok('without the option it stays', new Date((await ann.post('/tasks', { title: `Sat ${stamp}`, deadline: sat.toISOString() })).data.deadline).getUTCDay() === 6);
  const from = new Date(+sat - 86400e3), to = new Date(+sat + 4 * 86400e3);
  r = await ann.get(`/tasks?view=created&deadline_from=${from.toISOString()}&deadline_to=${to.toISOString()}`);
  ok('tasks by deadline window', r.data.items.length >= 2 && r.data.items.every((x: any) => x.deadline >= from.toISOString() && x.deadline < to.toISOString()), r.data.items.map((x: any) => x.deadline));

  // ---- recycle bin ----
  const sub = (await ann.post('/tasks', { title: `Sub ${stamp}`, parent_id: t.id })).data;
  await ann.post(`/tasks/${t.id}/dependencies`, { depends_on_id: open.id }).catch(() => {});
  ok('deleting puts the task in the bin', (await ann.del(`/tasks/${t.id}`)).status === 200 && (await ann.get(`/tasks/${t.id}`)).status === 404);
  r = await ann.get('/tasks/trash');
  ok('the bin lists it for 30 days', r.data.days === 30 && r.data.items.some((x: any) => x.id === t.id), r.data);
  ok('not for others', !(await cat.get('/tasks/trash')).data.items.some((x: any) => x.id === t.id) && (await cat.post(`/tasks/trash/${t.id}/restore`)).status === 404);
  ok('its subtask stays', (await ann.get(`/tasks/${sub.id}`)).data.parent_id === null);
  r = await ann.post(`/tasks/trash/${t.id}/restore`);
  ok('put back with its parts', r.status === 200 && r.data.number === t.number && r.data.checklist.length === 2 && r.data.result?.body === `Launched at 9:00 ${stamp}` && r.data.rating === 'good' && r.data.people.some((p: any) => p.user_id === cat.id), r.data);
  ok('its subtask is its subtask again', (await ann.get(`/tasks/${sub.id}`)).data.parent_id === t.id);
  ok('and the bin is empty of it', !(await ann.get('/tasks/trash')).data.items.some((x: any) => x.id === t.id));
  await ann.del(`/tasks/${open.id}`);
  ok('deleting from the bin is final', (await ann.del(`/tasks/trash/${open.id}`)).status === 200 && (await ann.post(`/tasks/trash/${open.id}/restore`)).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
