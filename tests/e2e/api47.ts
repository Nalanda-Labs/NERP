// Scrum teams: a backlog ranked by the team, sprints planned, started one at a time and
// completed (unfinished tasks move on), epics, story points, the definition of done, the
// burndown with scope changes, velocity, and who may do what.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Scrum${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [owner, master, dev, viewer, outsider] = await Promise.all(['Olga', 'Max', 'Dev', 'Vic', 'Otto'].map(user));

try {
  // ---- the team ----
  let r = await owner.post('/projects', { name: `Team ${stamp}`, kind: 'scrum', visibility: 'private', member_ids: [master.id, dev.id] });
  const team = r.data.id;
  ok('a Scrum team is a kind of group', r.status === 200 && r.data.kind === 'scrum', r.data);
  await owner.put(`/projects/${team}/members/${viewer.id}`, { role: 'viewer' });
  r = await owner.get(`/projects/${team}/scrum`);
  ok('its creator is the first Scrum master; sprints last two weeks', r.data.scrum_master?.id === owner.id && r.data.sprint_days === 14 && r.data.can_plan, r.data);
  ok('only members see it', (await outsider.get(`/projects/${team}/scrum`)).status === 404);
  ok('other groups have no Scrum', (await owner.get(`/projects/${(await owner.post('/projects', { name: `Plain ${stamp}` })).data.id}/scrum`)).status === 400);
  ok('the Scrum master must be on the team', (await owner.patch(`/projects/${team}/scrum`, { scrum_master_id: outsider.id })).status === 400);
  r = await owner.patch(`/projects/${team}/scrum`, { scrum_master_id: master.id, sprint_days: 7 });
  ok('hand over to a Scrum master, one-week sprints', r.data.scrum_master?.id === master.id && r.data.sprint_days === 7, r.data);
  ok('a member can\'t plan', !(await dev.get(`/projects/${team}/scrum`)).data.can_plan && (await dev.post(`/projects/${team}/sprints`, {})).status === 403);

  // ---- backlog, epics, points ----
  r = await dev.post(`/projects/${team}/epics`, { name: `Checkout ${stamp}`, color: 'emerald' });
  const epic = r.data.find((e: any) => e.name === `Checkout ${stamp}`)?.id;
  ok('members add epics', r.status === 200 && !!epic, r.data);
  ok('epic colours are checked', (await dev.post(`/projects/${team}/epics`, { name: 'x', color: 'plaid' })).status === 400);
  const mk = async (title: string, extra: any = {}) => (await dev.post('/tasks', { title: `${title} ${stamp}`, project_id: team, ...extra })).data;
  const a = await mk('Cart', { story_points: 5, epic_id: epic });
  const b = await mk('Pay', { story_points: 8 });
  const c = await mk('Receipt', { story_points: 3 });
  const d = await mk('Refunds');
  ok('tasks are made with points and an epic', a.story_points === 5 && a.epic_id === epic && a.epic_name === `Checkout ${stamp}` && a.sprint_id === null, a);
  ok('points are checked', (await dev.post('/tasks', { title: `Bad ${stamp}`, project_id: team, story_points: -1 })).status === 400);
  ok('an epic of another team is refused', (await dev.post('/tasks', { title: `Bad ${stamp}`, story_points: 1, epic_id: epic })).status === 400);
  const backlog = async () => (await dev.get(`/tasks?project_id=${team}&backlog=true&sort=backlog&limit=50`)).data.items.map((t: any) => t.id);
  ok('the backlog keeps the order tasks came in', JSON.stringify(await backlog()) === JSON.stringify([a.id, b.id, c.id, d.id]), await backlog());
  r = await dev.put(`/tasks/${d.id}/scrum`, { before_id: a.id, story_points: 2 });
  ok('rank a task to the top and estimate it', r.status === 200 && r.data.story_points === 2 && (await backlog())[0] === d.id, await backlog());
  ok('a viewer can\'t move tasks', (await viewer.put(`/tasks/${a.id}/scrum`, { story_points: 1 })).status === 403);
  r = await dev.get(`/projects/${team}/scrum`);
  ok('the backlog\'s size in tasks and points', r.data.backlog.count === 4 && r.data.backlog.points === 18, r.data.backlog);

  // ---- sprint planning ----
  r = await master.post(`/projects/${team}/sprints`, { goal: 'Take payments' });
  const s1 = r.data.id;
  ok('the Scrum master plans a sprint, numbered by default', r.status === 200 && r.data.name === 'Sprint 1' && r.data.status === 'planned', r.data);
  const s2 = (await master.post(`/projects/${team}/sprints`, { name: `Later ${stamp}` })).data.id;
  ok('an empty sprint can\'t start', (await master.post(`/sprints/${s1}/start`)).status === 400);
  for (const t of [a, b, c]) await dev.put(`/tasks/${t.id}/scrum`, { sprint_id: s1 });
  r = await dev.get(`/tasks?project_id=${team}&sprint_id=${s1}&sort=backlog`);
  ok('tasks dragged into the sprint, in order', JSON.stringify(r.data.items.map((t: any) => t.id)) === JSON.stringify([a.id, b.id, c.id]), r.data.items.map((t: any) => t.title));
  r = await dev.get(`/tasks/${a.id}`);
  ok('the move is in the task\'s history', r.data.id === a.id && JSON.stringify((await dev.get(`/tasks/${a.id}/history`)).data).includes('Sprint 1'));
  r = await master.post(`/sprints/${s1}/start`);
  ok('start the sprint: the commitment is its points, it runs a week', r.status === 200 && r.data.status === 'active' && r.data.committed_points === 16 && Math.round((Date.parse(r.data.ends_at) - Date.parse(r.data.starts_at)) / 86400e3) === 7, r.data);
  await dev.put(`/tasks/${d.id}/scrum`, { sprint_id: s2 });
  ok('only one sprint runs at a time', (await master.post(`/sprints/${s2}/start`)).status === 400);
  ok('a running sprint can\'t be deleted', (await master.del(`/sprints/${s1}`)).status === 400);

  // ---- working the sprint ----
  await dev.post(`/tasks/${a.id}/status`, { status: 'done' });
  const e = await mk('Hotfix', { story_points: 1, sprint_id: s1 });
  ok('a task added mid-sprint', e.sprint_id === s1);
  await dev.put(`/tasks/${c.id}/scrum`, { story_points: 5 });
  r = await dev.get(`/sprints/${s1}/burndown`);
  const today = r.data.days?.find((x: any) => x.remaining !== null && x.date === new Date().toISOString().slice(0, 10));
  ok('burndown: commitment, scope added, remaining today', r.data.committed === 16 && r.data.scope_added === 3 && today?.remaining === 14, r.data);
  ok('the ideal line runs from the commitment to zero', r.data.days[0].ideal === 16 && r.data.days.at(-1).ideal === 0, r.data.days);

  // ---- definition of done ----
  ok('a member can\'t change the definition of done', (await dev.put(`/projects/${team}/dod`, { items: ['x'] })).status === 403);
  r = await master.put(`/projects/${team}/dod`, { items: ['Code reviewed', 'Tests pass', '  '] });
  const dod = r.data;
  ok('the Scrum master sets it (blank points dropped)', dod.length === 2 && dod[0].title === 'Code reviewed', dod);
  r = await dev.post(`/tasks/${b.id}/dod`, { checked: [dod[0].id] });
  ok('every point must be ticked', r.status === 400 && /Tests pass/.test(r.data.error?.message), r.data);
  ok('all ticked: confirmed', (await dev.post(`/tasks/${b.id}/dod`, { checked: dod.map((x: any) => x.id) })).status === 200);
  await dev.post(`/tasks/${b.id}/status`, { status: 'done' });

  // ---- completing ----
  ok('unfinished work can\'t go to another team\'s sprint', (await master.post(`/sprints/${s1}/complete`, { move_to: crypto.randomUUID() })).status === 400);
  r = await master.post(`/sprints/${s1}/complete`, { move_to: s2 });
  ok('complete the sprint: done points counted', r.status === 200 && r.data.status === 'completed' && r.data.completed_points === 13 && r.data.task_count === 4 && r.data.done_count === 2, r.data);
  r = await dev.get(`/tasks?project_id=${team}&sprint_id=${s2}&sort=backlog`);
  const s2Tasks = r.data.items.map((t: any) => t.id);
  ok('unfinished tasks moved to the next sprint, ahead of what was there', s2Tasks.length === 3 && s2Tasks.at(-1) === d.id && s2Tasks.includes(c.id) && s2Tasks.includes(e.id), r.data.items.map((t: any) => t.title));
  ok('finished tasks stay with the old sprint', (await dev.get(`/tasks/${a.id}`)).data.sprint_id === s1);
  ok('tasks of a completed sprint can\'t be moved out', (await dev.put(`/tasks/${a.id}/scrum`, { sprint_id: null })).status === 400);
  ok('nor can tasks be put into it', (await dev.put(`/tasks/${d.id}/scrum`, { sprint_id: s1 })).status === 400);
  r = await dev.get(`/sprints/${s1}/burndown`);
  ok('a finished sprint keeps its burndown', r.status === 200 && r.data.committed === 16);
  r = await dev.get(`/projects/${team}/velocity`);
  ok('velocity: committed and completed per sprint, and the average', r.data.sprints.length === 1 && r.data.sprints[0].committed === 16 && r.data.sprints[0].completed === 13 && r.data.average === 13, r.data);
  r = await dev.get(`/projects/${team}/sprints?status=completed`);
  ok('sprint history', r.data.length === 1 && r.data[0].id === s1 && r.data[0].points_done === 13, r.data);
  r = await dev.get(`/projects/${team}/epics`);
  const ep = r.data.find((x: any) => x.id === epic);
  ok('an epic shows its progress', ep?.task_count === 1 && ep.done_count === 1 && ep.points_done === 5, r.data);

  // ---- tidying ----
  r = await master.post(`/projects/${team}/sprints`, {});
  const s3 = r.data.id;
  ok('sprint numbers keep counting', r.data.name === 'Sprint 3', r.data);
  await dev.put(`/tasks/${d.id}/scrum`, { sprint_id: s3 });
  ok('deleting a planned sprint returns its tasks to the backlog', (await master.del(`/sprints/${s3}`)).status === 200 && (await dev.get(`/tasks/${d.id}`)).data.sprint_id === null);
  ok('deleting an epic is for planners', (await dev.del(`/epics/${epic}`)).status === 403 && (await master.del(`/epics/${epic}`)).status === 200 && (await dev.get(`/tasks/${a.id}`)).data.epic_id === null);
  const other = (await owner.post('/projects', { name: `Other ${stamp}`, kind: 'scrum' })).data.id;
  await owner.put(`/projects/${other}/members/${dev.id}`, { role: 'member' });
  await owner.patch(`/tasks/${c.id}`, { project_id: other });
  r = await dev.get(`/tasks/${c.id}`);
  ok('moving a task to another team takes it out of the sprint', r.data.project_id === other && r.data.sprint_id === null, r.data);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
