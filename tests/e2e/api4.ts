import { execSync } from 'node:child_process';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 400)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'P' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const own = await mk('owner'), mem = await mk('member');
const day = (n) => new Date(Date.UTC(2026, 11, 1 + n, 9)).toISOString().replace('.000Z', 'Z'); // Dec 1 2026 + n days, 09:00Z
const unread = async (c) => (await c.get('/notifications?limit=50')).data.items;

// ================= Board columns =================
let r: any = await own.post('/projects', { name: `Plan ${stamp}`, member_ids: [mem.id] });
const proj = r.data.id;
ok('new project gets 4 default columns', r.data.stages.map((s) => s.name).join() === 'To do,In progress,Review,Done', r.data.stages);
const st = Object.fromEntries(r.data.stages.map((s) => [s.name, s]));
const memWs = await mem.ws();
r = await own.post(`/projects/${proj}/stages`, { name: 'Design', color: 'violet', after_id: st['To do'].id });
ok('owner adds a column after To do', r.status === 200 && r.data.map((s) => s.name).join() === 'To do,Design,In progress,Review,Done', r.data);
const design = r.data.find((s) => s.name === 'Design');
await sleep(300);
ok('members get live column update', memWs.events.some((e) => e.type === 'project.stages'));
ok('member cannot add columns', (await mem.post(`/projects/${proj}/stages`, { name: 'x' })).status === 403);
ok('bad column status rejected', (await own.post(`/projects/${proj}/stages`, { name: 'x', status: 'bogus' })).status === 400);

r = await own.post('/tasks', { title: 'Logo', project_id: proj, responsible_id: mem.id, requires_review: true });
const logo = r.data;
ok('project task starts in To do', logo.stage_id === st['To do'].id);
r = await mem.post(`/tasks/${logo.id}/stage`, { stage_id: design.id });
ok('move to custom column keeps status', r.data.stage_id === design.id && r.data.status === 'new', r.data);
r = await mem.post(`/tasks/${logo.id}/status`, { status: 'in_progress' });
ok('starting work keeps it in the custom column', r.data.stage_id === design.id && r.data.status === 'in_progress');
r = await mem.post(`/tasks/${logo.id}/stage`, { stage_id: st['Done'].id });
ok('dropping a review task on Done lands in Review', r.data.status === 'review' && r.data.stage_id === st['Review'].id, r.data);
r = await own.post(`/tasks/${logo.id}/status`, { status: 'done' });
ok('accepting moves it to Done column', r.data.status === 'done' && r.data.stage_id === st['Done'].id);
r = await own.post(`/tasks/${logo.id}/stage`, { stage_id: st['In progress'].id });
ok('dragging back reopens it', r.data.status === 'in_progress' && !r.data.completed_at);
r = await own.patch(`/projects/${proj}/stages/${design.id}`, { name: 'Design & UX', color: 'rose', status: null });
ok('rename column', r.data.find((s) => s.id === design.id).name === 'Design & UX');
await own.post(`/tasks/${logo.id}/stage`, { stage_id: design.id });
r = await own.del(`/projects/${proj}/stages/${design.id}?move_to=${st['To do'].id}`);
ok('delete column moves its cards', r.data.length === 4 && (await own.get(`/tasks/${logo.id}`)).data.stage_id === st['To do'].id);
ok('history records column moves', (await own.get(`/tasks/${logo.id}/history`)).data.some((h) => h.field === 'stage'));
ok('existing projects got columns (migration)', (await admin.get('/projects?archived=true')).status === 200 &&
  (await Promise.all((await admin.get('/projects')).data.slice(0, 3).map((p) => admin.get(`/projects/${p.id}/stages`)))).every((x) => x.data.length >= 4));

// ================= Dependencies & Gantt =================
const A = (await own.post('/tasks', { title: 'A design', project_id: proj, responsible_id: mem.id, start_date: day(0), deadline: day(2) })).data;
const Bt = (await own.post('/tasks', { title: 'B build', project_id: proj, responsible_id: mem.id, start_date: day(1), deadline: day(3) })).data;
const C = (await own.post('/tasks', { title: 'C launch', project_id: proj, responsible_id: own.id, start_date: day(4), deadline: day(5) })).data;
ok('start after deadline rejected', (await own.post('/tasks', { title: 'x', start_date: day(3), deadline: day(1) })).status === 400);
r = await own.post(`/tasks/${Bt.id}/dependencies`, { depends_on_id: A.id });
ok('add dependency', r.status === 200);
r = await own.get(`/tasks/${Bt.id}`);
ok('dependent shifted to start when predecessor ends', r.data.start_date === day(2) && r.data.deadline === day(4), { s: r.data.start_date, d: r.data.deadline });
ok('blocked_by shown', r.data.blocked_by_count === 1 && r.data.blocked_by[0].id === A.id);
ok('blocking shown on predecessor', (await own.get(`/tasks/${A.id}`)).data.blocking[0]?.id === Bt.id);
ok('responsible told about reschedule', (await unread(mem)).some((n) => n.kind === 'task.rescheduled'));
ok('cycle rejected', (await own.post(`/tasks/${A.id}/dependencies`, { depends_on_id: Bt.id })).status === 400);
ok('self-dependency rejected', (await own.post(`/tasks/${A.id}/dependencies`, { depends_on_id: A.id })).status === 400);
ok('non-editor cannot link', (await mem.post(`/tasks/${C.id}/dependencies`, { depends_on_id: Bt.id })).status === 403);
await own.post(`/tasks/${C.id}/dependencies`, { depends_on_id: Bt.id });
ok('C not moved (already after B)', (await own.get(`/tasks/${C.id}`)).data.start_date === day(4));
r = await own.patch(`/tasks/${A.id}`, { deadline: day(5) });
ok('moving A later cascades to B and C', (await own.get(`/tasks/${Bt.id}`)).data.start_date === day(5) && (await own.get(`/tasks/${C.id}`)).data.start_date === day(7),
  [(await own.get(`/tasks/${Bt.id}`)).data.start_date, (await own.get(`/tasks/${C.id}`)).data.start_date]);
ok('reschedule recorded in history', (await own.get(`/tasks/${C.id}/history`)).data.some((h) => h.field === 'rescheduled'));
r = await own.get(`/tasks/gantt?project_id=${proj}`);
ok('gantt returns tasks + edges', r.data.tasks.length >= 4 && r.data.dependencies.length === 2 && r.data.dependencies.some((e) => e.task_id === Bt.id && e.depends_on_id === A.id), r.data.dependencies);
await own.post(`/tasks/${A.id}/status`, { status: 'done' });
ok('finished predecessor unblocks', (await own.get(`/tasks/${Bt.id}`)).data.blocked_by_count === 0);
ok('remove dependency', (await own.del(`/tasks/${C.id}/dependencies/${Bt.id}`)).status === 200 && (await own.get(`/tasks/${C.id}`)).data.blocked_by.length === 0);

// ================= Templates & recurring =================
r = await own.post(`/tasks/${Bt.id}/save-as-template`, {});
ok('save task as template', r.status === 200 && r.data.title === 'B build' && r.data.deadline_offset_secs === 2 * 86400 && r.data.project_id === proj, r.data);
const tpl = r.data.id;
ok('others cannot see the template', (await mem.get(`/task-templates/${tpl}`)).status === 404);
r = await own.post(`/task-templates/${tpl}/use`);
ok('use template creates task', r.status === 200 && r.data.template_id === tpl && r.data.template.mine && r.data.stage_id === st['To do'].id, r.data);
ok('template deadline from offset', Math.abs(+new Date(r.data.deadline) - Date.now() - 2 * 864e5) < 60e3);
ok('weekly without weekdays rejected', (await own.post('/task-templates', { title: 'x', recurrence: { freq: 'weekly', interval: 1, weekdays: [], time: '09:00', timezone: 'UTC', starts_on: '2026-11-01' } })).status === 400);
ok('unknown time zone rejected', (await own.post('/task-templates', { title: 'x', recurrence: { freq: 'daily', interval: 1, time: '09:00', timezone: 'Mars/Base', starts_on: '2026-11-01' } })).status === 400);

const soon = new Date(Date.now() + 60e3);
const hhmm = `${String(soon.getUTCHours()).padStart(2, '0')}:${String(soon.getUTCMinutes()).padStart(2, '0')}`;
const today = soon.toISOString().slice(0, 10);
r = await own.post('/task-templates', { title: `Daily standup notes ${stamp}`, responsible_id: mem.id, checklist: ['Yesterday', 'Today', 'Blockers'],
  deadline_offset_secs: 3600, recurrence: { freq: 'daily', interval: 1, time: hhmm, timezone: 'UTC', starts_on: today } });
ok('create recurring template', r.status === 200 && r.data.schedule === `Every day at ${hhmm}` && !!r.data.next_run_at, r.data);
const rec = r.data;
console.log(`     (waiting for the ${hhmm} UTC run, up to ~2.5 min…)`);
let made: any = null;
for (let i = 0; i < 160 && !made; i++) {
  if (i % 10 === 0) execSync('redis-cli del jobs:recurring:tasks.recurring');
  await sleep(1000);
  const list = (await mem.get(`/tasks?view=my&q=${encodeURIComponent('Daily standup notes ' + stamp)}`)).data.items;
  made = list[0] ?? null;
}
ok('recurring task created on schedule', !!made, 'none');
if (made) {
  const d = (await mem.get(`/tasks/${made.id}`)).data;
  ok('created for the template owner, assigned per template', d.created_by === own.id && d.responsible_id === mem.id && d.checklist.length === 3);
  ok('deadline = scheduled time + offset', +new Date(d.deadline) - +new Date(rec.next_run_at) === 3600e3, [d.deadline, rec.next_run_at]);
  ok('assignee notified', (await unread(mem)).some((n) => n.kind === 'task.assigned' && n.link === `/tasks/${made.id}`));
  ok('task shows its schedule', d.template?.schedule === `Every day at ${hhmm}` && d.template.mine === false);
  const t2 = (await own.get(`/task-templates/${rec.id}`)).data;
  ok('template advanced to tomorrow', t2.run_count === 1 && +new Date(t2.next_run_at) - +new Date(rec.next_run_at) === 864e5 && t2.task_count === 1, t2);
}
r = await own.patch(`/task-templates/${rec.id}`, { ...rec, recurrence: null, is_active: true });
ok('turn off recurrence', r.data.recurrence === null && r.data.next_run_at === null);
ok('delete template', (await own.del(`/task-templates/${rec.id}`)).status === 200);
memWs.close();
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
