import { execSync } from 'node:child_process';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(extra).slice(0, 400)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name[0].toUpperCase() + name.slice(1), last_name: 'Test' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const tom = await mk('tom'), uma = await mk('uma'), vic = await mk('vic'), wes = await mk('wes');
const tomWs = await tom.ws();
const unread = async (c) => (await c.get('/notifications?limit=50')).data.items;

// ---- projects ----
let r: any = await tom.post('/projects', { name: `Website ${stamp}`, description: 'Relaunch', member_ids: [uma.id] });
ok('create private project (creator is owner)', r.status === 200 && r.data.access.manage && r.data.members.length === 2, JSON.stringify(r.data));
const proj = r.data.id;
ok('member added gets notification', (await unread(uma)).some((n) => n.kind === 'project.added'));
ok('outsider cannot see private project', (await vic.get(`/projects/${proj}`)).status === 404);
ok('outsider project list excludes it', !(await vic.get('/projects')).data.some((p) => p.id === proj));
ok('viewer/outsider cannot add task to project', (await vic.post('/tasks', { title: 'x', project_id: proj })).status === 404);

// ---- create with review ----
r = await tom.post('/tasks', { title: 'Design homepage', description: 'Hero + nav', responsible_id: uma.id, project_id: proj, requires_review: true,
  deadline: new Date(Date.now() + 3 * 864e5).toISOString(), priority: 'high', checklist: ['Wireframe', 'Mockup', ''], watcher_ids: [admin.id] });
ok('create task', r.status === 200 && r.data.number > 0 && r.data.checklist.length === 2 && r.data.people.length === 3, JSON.stringify(r.data));
const t1 = r.data.id;
ok('responsible notified', (await unread(uma)).some((n) => n.kind === 'task.assigned' && n.link === `/tasks/${t1}`));
ok('outsider cannot see task', (await vic.get(`/tasks/${t1}`)).status === 404);
r = await uma.get(`/tasks/${t1}`);
ok('responsible can participate but not edit', r.data.access.participate && !r.data.access.edit && !r.data.access.delete);
ok('responsible cannot edit title', (await uma.patch(`/tasks/${t1}`, { title: 'hacked' })).status === 403);

// ---- checklist ----
const item = r.data.checklist[0].id;
ok('tick checklist item', (await uma.patch(`/tasks/${t1}/checklist/${item}`, { is_done: true })).status === 200);
r = await uma.post(`/tasks/${t1}/checklist`, { title: 'Review copy' });
ok('add checklist item', r.status === 200);
r = await uma.get(`/tasks/${t1}`);
ok('checklist counts', r.data.checklist_total === 3 && r.data.checklist_done === 1, `${r.data.checklist_total}/${r.data.checklist_done}`);

// ---- review workflow + realtime ----
tomWs.events.length = 0;
r = await uma.post(`/tasks/${t1}/status`, { status: 'done' });
ok('completing a review task -> review', r.data.status === 'review', JSON.stringify(r.data).slice(0, 200));
await sleep(300);
ok('creator gets live task.changed', tomWs.events.some((e) => e.type === 'task.changed' && e.data.id === t1));
ok('creator notified to review', (await unread(tom)).some((n) => n.kind === 'task.review'));
ok('responsible cannot accept own work', (await uma.post(`/tasks/${t1}/status`, { status: 'done' })).status === 400);
r = await tom.post(`/tasks/${t1}/status`, { status: 'in_progress' });
ok('creator returns it', r.data.status === 'in_progress');
ok('responsible told it was returned', (await unread(uma)).some((n) => n.kind === 'task.returned'));
await uma.post(`/tasks/${t1}/status`, { status: 'done' });
r = await tom.post(`/tasks/${t1}/status`, { status: 'done' });
ok('creator accepts -> done', r.data.status === 'done' && !!r.data.completed_at);
ok('responsible told it was accepted', (await unread(uma)).some((n) => n.kind === 'task.accepted'));

// ---- comments & mentions ----
r = await uma.post(`/tasks/${t1}/comments`, { body: `Looks good — @[Vic Test](${vic.id}) can you check the copy?` });
ok('comment', r.status === 200 && r.data.author_name === 'Uma Test');
ok('mentioned user notified', (await unread(vic)).some((n) => n.kind === 'task.mention' && n.body.includes('@Vic Test')));
ok('mentioned user can now see the task (watcher)', (await vic.get(`/tasks/${t1}`)).status === 200);
ok('participants notified of comment', (await unread(tom)).some((n) => n.kind === 'task.comment'));
ok('watcher (admin) notified of comment', (await unread(admin)).some((n) => n.kind === 'task.comment'));
const cid = r.data.id;
ok('others cannot edit comment', (await vic.patch(`/tasks/${t1}/comments/${cid}`, { body: 'x' })).status === 400);
ok('author edits comment', (await uma.patch(`/tasks/${t1}/comments/${cid}`, { body: 'Edited' })).data.edited_at !== null);
ok('comments listed', (await vic.get(`/tasks/${t1}/comments`)).data.length === 1);

// ---- attachments ----
const fd = new FormData(); fd.append('file', new Blob(['spec text']), `spec-${stamp}.txt`);
const file = (await uma.post('/files', fd)).data;
ok('attach own file', (await uma.post(`/tasks/${t1}/files`, { file_id: file.id })).status === 200);
const dl = await vic.req('GET', `/files/${file.id}/content`, undefined, true);
ok('task viewer can download attachment', dl.status === 200 && (await dl.text()) === 'spec text');
ok('outsider cannot download attachment', (await wes.req('GET', `/files/${file.id}/content`, undefined, true)).status === 404);
ok('cannot attach someone else\'s file', (await tom.post(`/tasks/${t1}/files`, { file_id: file.id })).status === 400);

// ---- subtasks ----
r = await tom.post('/tasks', { title: 'Subtask A', parent_id: t1, responsible_id: tom.id });
ok('subtask inherits project', r.data.project_id === proj && r.data.parent?.id === t1, JSON.stringify(r.data).slice(0, 300));
const sub = r.data.id;
ok('parent lists subtask', (await tom.get(`/tasks/${t1}`)).data.subtasks.some((s) => s.id === sub));
ok('no cycles', (await tom.patch(`/tasks/${t1}`, { parent_id: sub })).status === 400);

// ---- time tracking ----
r = await tom.post('/tasks', { title: 'Timer task', responsible_id: uma.id });
const t2 = r.data.id;
r = await uma.post(`/tasks/${t2}/timer/start`);
ok('start timer', r.status === 200);
r = await uma.get(`/tasks/${t2}`);
ok('timer start moves new -> in_progress', r.data.status === 'in_progress' && !!r.data.my_timer_started_at);
ok('my running timer', (await uma.get('/tasks/timer')).data?.task_id === t2);
await sleep(1100);
await uma.post(`/tasks/${sub}/timer/start`).then(() => {});
r = await uma.get('/tasks/timer');
ok('starting elsewhere switches timer (if allowed)', r.data === null || r.data.task_id === t2 || r.data.task_id === sub);
await uma.post(`/tasks/${t2}/timer/stop`);
ok('manual time entry', (await uma.post(`/tasks/${t2}/time`, { minutes: 90, comment: 'call' })).status === 200);
r = await uma.get(`/tasks/${t2}`);
ok('time spent adds up', r.data.time_spent_secs >= 5400, r.data.time_spent_secs);
ok('time entries listed', (await uma.get(`/tasks/${t2}/time`)).data.length >= 1);
ok('outsider cannot log time', (await wes.post(`/tasks/${t2}/time`, { minutes: 5 })).status === 404);

// ---- lists, filters, counters ----
r = await tom.post('/tasks', { title: `Overdue ${stamp}`, responsible_id: uma.id, deadline: new Date(Date.now() - 3600e3).toISOString() });
const t3 = r.data.id;
r = await tom.post('/tasks', { title: `Soon ${stamp}`, responsible_id: uma.id, deadline: new Date(Date.now() + 2 * 3600e3).toISOString() });
const t4 = r.data.id;
r = await uma.get('/tasks?view=my&status=new,in_progress,review');
ok('my tasks', r.data.items.some((t) => t.id === t2) && !r.data.items.some((t) => t.id === t1), r.data.items.map((t) => t.title));
ok('created by me', (await tom.get('/tasks?view=created')).data.total >= 5);
ok('watching', (await vic.get('/tasks?view=watching')).data.items.some((t) => t.id === t1));
ok('overdue filter', (await uma.get('/tasks?view=my&overdue=true')).data.items.every((t) => t.is_overdue) && (await uma.get('/tasks?view=my&overdue=true')).data.items.some((t) => t.id === t3));
ok('search by #number', (await uma.get(`/tasks?q=%23${(await uma.get(`/tasks/${t3}`)).data.number}`)).data.items[0]?.id === t3);
r = await uma.get('/tasks/counters');
ok('counters', r.data.my_open >= 3 && r.data.my_overdue >= 1 && r.data.my_due_soon >= 1, JSON.stringify(r.data));
ok('project filter', (await tom.get(`/tasks?project_id=${proj}`)).data.total === 2);

// ---- board move ----
r = await tom.post('/tasks', { title: 'Card 1', project_id: proj, responsible_id: tom.id });
const c1 = r.data.id;
r = await tom.post('/tasks', { title: 'Card 2', project_id: proj, responsible_id: tom.id });
const c2 = r.data.id;
r = await tom.post(`/tasks/${c1}/move`, { status: 'in_progress', after_id: null, before_id: null });
ok('move card to column', r.data.status === 'in_progress');
r = await tom.post(`/tasks/${c2}/move`, { status: 'in_progress', after_id: c1 });
const order = (await tom.get(`/tasks?project_id=${proj}&status=in_progress&sort=position`)).data.items.map((t) => t.id);
ok('board order respected', order.indexOf(c1) < order.indexOf(c2), order);

// ---- edit, history, visibility ----
r = await tom.patch(`/tasks/${t4}`, { deadline: new Date(Date.now() + 5 * 864e5).toISOString(), assistant_ids: [wes.id] });
ok('edit deadline + add assistant', r.status === 200 && r.data.people.some((p) => p.user_id === wes.id && p.role === 'assistant'));
ok('assistant notified', (await unread(wes)).some((n) => n.kind === 'task.assistant'));
ok('deadline change notified', (await unread(uma)).some((n) => n.kind === 'task.deadline_changed'));
r = await tom.get(`/tasks/${t1}/history`);
ok('history', ['created', 'status', 'checklist_done', 'file_added'].every((f) => r.data.some((h) => h.field === f)), r.data.map((h) => h.field));
await tom.patch(`/projects/${proj}`, { visibility: 'open' });
ok('open project visible to everyone', (await wes.get(`/projects/${proj}`)).status === 200 && (await wes.get(`/tasks/${c1}`)).status === 200);
ok('non-member cannot create in open project', (await wes.post('/tasks', { title: 'x', project_id: proj })).status === 400);
r = await wes.post(`/projects/${proj}/join`);
ok('join open project', r.data.access.role === 'member' && (await wes.post('/tasks', { title: 'joined task', project_id: proj })).status === 200);

// ---- deletion & project rules ----
ok('responsible cannot delete', (await uma.del(`/tasks/${t2}`)).status === 403);
ok('creator deletes', (await tom.del(`/tasks/${t2}`)).status === 200 && (await tom.get(`/tasks/${t2}`)).status === 404);
ok('project with tasks cannot be deleted', (await tom.del(`/projects/${proj}`)).status === 400);
ok('last owner cannot leave', (await tom.post(`/projects/${proj}/leave`)).status === 400);
ok('archive project', (await tom.patch(`/projects/${proj}`, { is_archived: true })).data.is_archived === true);
ok('archived project rejects new tasks', (await tom.post('/tasks', { title: 'x', project_id: proj })).status === 400);
ok('admin (manage_all) sees everything', (await admin.get(`/tasks/${t3}`)).data.access.edit === true);

// ---- deadline reminders job ----
execSync('redis-cli del jobs:recurring:tasks.deadlines');
console.log('     (waiting up to 70s for the reminder job to run…)');
let got = false;
for (let i = 0; i < 70 && !got; i++) { await sleep(1000); const n = await unread(uma); got = n.some((x) => x.kind === 'task.overdue' && x.link === `/tasks/${t3}`); }
ok('overdue reminder sent', got);
ok('creator also told about overdue', (await unread(tom)).some((x) => x.kind === 'task.overdue'));
const n2 = (await unread(uma)).filter((x) => x.kind === 'task.overdue' && x.link === `/tasks/${t3}`).length;
ok('reminder sent only once', n2 === 1, n2);

tomWs.close();
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
