// Task depth, part two: board automation (rules when a task enters a column, triggers that
// move tasks), templates with subtasks shared with everyone, copying a project, the
// efficiency report, and flows (queue, manual and self-service hand-out, with their numbers).
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Flow${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat, dan] = await Promise.all(['Ann', 'Bob', 'Cat', 'Dan'].map(user));

try {
  // ---- automation ----
  const proj = (await ann.post('/projects', { name: `Studio ${stamp}`, member_ids: [bob.id, cat.id] })).data;
  const [todo, , review, done] = proj.stages;
  const waiting = (await ann.post(`/projects/${proj.id}/stages`, { name: 'Waiting for client' })).data.find((s: any) => s.name === 'Waiting for client');
  const check = (await ann.post(`/projects/${proj.id}/stages`, { name: 'Checking' })).data.find((s: any) => s.name === 'Checking');
  ok('only owners set up automation', (await bob.post(`/projects/${proj.id}/automation/rules`, { stage_id: review.id, kind: 'comment', params: { text: 'x' } })).status === 403);
  let r = await ann.post(`/projects/${proj.id}/automation/rules`, { stage_id: check.id, kind: 'set_responsible', params: { user_id: cat.id } });
  ok('a rule: entering Checking gives the task to Cat', r.status === 200 && r.data.rules.length === 1, r.data);
  await ann.post(`/projects/${proj.id}/automation/rules`, { stage_id: check.id, kind: 'comment', params: { text: 'Ready for {responsible}: #{number} {task}' } });
  await ann.post(`/projects/${proj.id}/automation/rules`, { stage_id: check.id, kind: 'create_subtask', params: { title: 'Check spelling of {task}', responsible: 'creator' } });
  await ann.post(`/projects/${proj.id}/automation/rules`, { stage_id: todo.id, kind: 'set_deadline', params: { days: 3 } });
  ok('rule settings are checked', (await ann.post(`/projects/${proj.id}/automation/rules`, { stage_id: review.id, kind: 'set_priority', params: { priority: 'urgent' } })).status === 400 && (await ann.post(`/projects/${proj.id}/automation/rules`, { stage_id: review.id, kind: 'explode' })).status === 400);
  let t = (await bob.post('/tasks', { title: `Poster ${stamp}`, project_id: proj.id })).data;
  t = await waitFor(async () => { const x = (await bob.get(`/tasks/${t.id}`)).data; return x.deadline ? x : null; });
  ok('a new task enters "To do": its deadline is set three days out', t && Math.round((Date.parse(t.deadline) - Date.now()) / 86400e3) === 3, t?.deadline);
  await bob.post(`/tasks/${t.id}/stage`, { stage_id: check.id });
  t = await waitFor(async () => { const x = (await ann.get(`/tasks/${t.id}`)).data; return x.responsible_id === cat.id && x.subtasks.length ? x : null; });
  ok('moving to Checking runs its rules in order', !!t, t);
  const comments = (await ann.get(`/tasks/${t.id}/comments`)).data;
  ok('placeholders filled in the comment', comments.some((c: any) => c.body === `Ready for Cat Flow${stamp}: #${t.number} Poster ${stamp}`), comments.map((c: any) => c.body));
  ok('the subtask goes to the creator', t.subtasks[0]?.title === `Check spelling of Poster ${stamp}` && t.subtasks[0].responsible_id === bob.id, t.subtasks);
  r = await ann.get(`/projects/${proj.id}/automation`);
  ok('the automation log says what happened', r.data.log.length >= 4 && r.data.log.some((l: any) => /responsible/.test(l.what)), r.data.log);

  r = await ann.post(`/projects/${proj.id}/automation/triggers`, { event: 'comment', stage_id: waiting.id });
  ok('a trigger: a comment moves the task to "Waiting for client"', r.data.triggers.length === 1);
  await ann.post(`/projects/${proj.id}/automation/triggers`, { event: 'checklist_done', stage_id: done.id });
  ok('trigger events are checked', (await ann.post(`/projects/${proj.id}/automation/triggers`, { event: 'moon', stage_id: done.id })).status === 400);
  const t2 = (await bob.post('/tasks', { title: `Flyer ${stamp}`, project_id: proj.id, checklist: ['Print'] })).data;
  await cat.post(`/tasks/${t2.id}/comments`, { body: 'Sent to the client' }).catch(() => {});
  await ann.post(`/tasks/${t2.id}/comments`, { body: 'Sent to the client' });
  ok('commenting moved it', (await waitFor(async () => (await ann.get(`/tasks/${t2.id}`)).data.stage_id === waiting.id)) === true);
  const item = (await bob.get(`/tasks/${t2.id}`)).data.checklist[0];
  await bob.patch(`/tasks/${t2.id}/checklist/${item.id}`, { is_done: true });
  r = await bob.get(`/tasks/${t2.id}`);
  ok('ticking the whole checklist moves it to Done (and finishes it)', r.data.stage_id === done.id && r.data.status === 'done', r.data);

  // ---- templates with subtasks, shared ----
  r = await ann.post('/task-templates', { title: `Onboarding ${stamp}`, subtasks: [{ title: 'Laptop' }, { title: 'Accounts', responsible_id: cat.id, deadline_offset_secs: 86400 }, { title: ' ' }], shared: true });
  const tpl = r.data;
  ok('a template with subtasks (blank ones dropped), shared', tpl.subtasks.length === 2 && tpl.shared, tpl);
  ok('others see a shared template', (await dan.get('/task-templates')).data.some((x: any) => x.id === tpl.id));
  ok('but can\'t change it', (await dan.put?.(`/task-templates/${tpl.id}`, { title: 'x' }))?.status !== 200 && (await dan.patch(`/task-templates/${tpl.id}`, { title: 'x' })).status === 404);
  r = await dan.post(`/task-templates/${tpl.id}/use`);
  ok('using it makes the task and its subtasks', r.status === 200 && r.data.subtasks.length === 2 && r.data.subtasks.some((s: any) => s.title === 'Accounts' && s.responsible_id === cat.id), r.data);
  const own = (await ann.post('/task-templates', { title: `Private ${stamp}` })).data;
  ok('a private template stays private', !(await dan.get('/task-templates')).data.some((x: any) => x.id === own.id) && (await dan.post(`/task-templates/${own.id}/use`)).status === 404);

  // ---- copying a project ----
  await ann.patch(`/projects/${proj.id}`, { starts_on: '2026-01-05' });
  await ann.post('/tasks', { title: `Dated ${stamp}`, project_id: proj.id, deadline: '2026-01-10T12:00:00Z' });
  r = await ann.post(`/projects/${proj.id}/copy`, { name: `Studio copy ${stamp}`, starts_on: '2026-03-02', members: true });
  const copy = r.data;
  ok('a copy: columns, members', r.status === 200 && copy.stages.length === proj.stages.length + 2 && copy.members.length === 3, r.data);
  r = await ann.get(`/tasks?project_id=${copy.id}&limit=50`);
  const dated = r.data.items.find((x: any) => x.title === `Dated ${stamp}`);
  ok('its tasks, as new ones with dates moved with the start', dated && dated.status === 'new' && dated.deadline.startsWith('2026-03-07'), dated);
  ok('subtasks keep their parent', r.data.items.some((x: any) => x.title === `Check spelling of Poster ${stamp}` && x.parent_id));
  r = await ann.get(`/projects/${copy.id}/automation`);
  ok('and the board automation', r.data.rules.length === 4 && r.data.triggers.length === 2 && r.data.rules.every((x: any) => copy.stages.some((st: any) => st.id === x.stage_id)));

  // ---- efficiency report ----
  r = await cat.get('/tasks/reports/efficiency?scope=me');
  ok('my numbers', r.status === 200 && r.data.items.length === 1 && r.data.items[0].user_id === cat.id && r.data.items[0].open >= 1, r.data);
  ok('everyone\'s is for administrators', (await cat.get('/tasks/reports/efficiency?scope=all')).status === 403 && (await admin.get('/tasks/reports/efficiency?scope=all')).status === 200);
  const dept = (await admin.post('/departments', { name: `Design ${stamp}`, head_user_id: ann.id })).data;
  await admin.post(`/departments/${dept.id}/members`, { user_id: bob.id });
  r = await ann.get('/tasks/reports/efficiency');
  const bobRow = r.data.items.find((x: any) => x.user_id === bob.id);
  ok('a head sees their team: done, on time, efficiency', bobRow && bobRow.done >= 1 && bobRow.efficiency === 100, r.data.items);

  // ---- flows ----
  ok('only the group\'s owners make flows', (await bob.post('/flows', { project_id: proj.id, name: 'x', team: [bob.id] })).status === 400);
  r = await ann.post('/flows', { project_id: proj.id, name: `Requests ${stamp}`, distribution: 'queue', team: [bob.id, cat.id], planned_hours: 8 });
  const flow = r.data;
  ok('a queue flow with a planned time', r.status === 200 && flow.distribution === 'queue' && flow.planned_hours === 8 && flow.members.length === 2, r.data);
  const a1 = (await dan.post(`/flows/${flow.id}/tasks`, { title: `Fix printer ${stamp}` })).data;
  const a2 = (await dan.post(`/flows/${flow.id}/tasks`, { title: `New mouse ${stamp}` })).data;
  ok('anyone sends tasks; the queue hands them out in turn', a1.responsible_id !== a2.responsible_id && [bob.id, cat.id].includes(a1.responsible_id), [a1.responsible_name, a2.responsible_name]);
  ok('the deadline is the planned time', Math.round((Date.parse(a1.deadline) - Date.now()) / 3600e3) === 8, a1.deadline);
  ok('the task is in the group', a1.project_id === proj.id);
  const who = a1.responsible_id === bob.id ? bob : cat;
  await who.post(`/tasks/${a1.id}/status`, { status: 'in_progress' });
  await who.post(`/tasks/${a1.id}/status`, { status: 'done' });
  r = await ann.get(`/flows/${flow.id}`);
  ok('the flow\'s numbers: waiting, at work, done, efficiency', r.data.stats.done === 1 && r.data.stats.waiting === 1 && r.data.stats.efficiency === 100 && r.data.stats.avg_wait_hours !== null, r.data.stats);
  await ann.patch(`/flows/${flow.id}`, { distribution: 'self' });
  const a3 = (await dan.post(`/flows/${flow.id}/tasks`, { title: `Monitor ${stamp}` })).data;
  ok('self-service: the task waits for the team', (await ann.get(`/flows/${flow.id}/tasks?state=waiting`)).data.items.some((x: any) => x.id === a3.id && x.flow_waiting));
  ok('someone outside the team can\'t take it', (await dan.post(`/tasks/${a3.id}/take`)).status === 400);
  r = await cat.post(`/tasks/${a3.id}/take`);
  ok('the team takes it', r.status === 200 && r.data.responsible_id === cat.id, r.data);
  ok('only once', (await bob.post(`/tasks/${a3.id}/take`)).status === 400);
  await ann.patch(`/flows/${flow.id}`, { distribution: 'manual' });
  const a4 = (await dan.post(`/flows/${flow.id}/tasks`, { title: `Chair ${stamp}` })).data;
  ok('manual: it goes to the flow\'s administrator, waiting', a4.responsible_id === ann.id);
  await ann.patch(`/tasks/${a4.id}`, { responsible_id: bob.id });
  ok('who hands it out', (await ann.get(`/flows/${flow.id}/tasks`)).data.items.find((x: any) => x.id === a4.id)?.flow_waiting === false);
  await ann.patch(`/flows/${flow.id}`, { is_active: false });
  ok('a paused flow takes no tasks', (await dan.post(`/flows/${flow.id}/tasks`, { title: 'x' })).status === 400);
  ok('flows of the group are listed', (await bob.get(`/flows?project_id=${proj.id}`)).data.items.some((x: any) => x.id === flow.id));
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
