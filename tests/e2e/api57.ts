// Meetings: creating (checks, invitations, the calendar event), who sees them (people, the
// workgroup, nobody else), the agenda (topics and sub-points, proposals, presenters, their
// reports, order), moving a meeting (the calendar follows), changing people, starting,
// attendance, decisions, a task from a topic, closing with minutes, reopening, the next
// meeting with the unfinished topics, a topic's history, files, cancelling.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Meets' })).data;
  const c = new Client() as Client & { id: string };
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const olga = await user('Olga');
const kai = await user('Kai');
const mia = await user('Mia');
const ned = await user('Ned');
const wes = await user('Wes');
const notified = async (c: Client, title: string | RegExp) => {
  for (let i = 0; i < 30; i++) {
    const items = (await c.get('/notifications')).data.items as any[];
    const n = items.find((x) => (typeof title === 'string' ? x.title === title : title.test(x.title)));
    if (n) return n;
    await new Promise((r) => setTimeout(r, 300));
  }
  return null;
};
const day = (d: number, h = 10) => { const x = new Date(); x.setUTCDate(x.getUTCDate() + d); x.setUTCHours(h, 0, 0, 0); return x.toISOString(); };

try {
  const group = (await olga.post('/projects', { name: `Board ${stamp}`, kind: 'group', visibility: 'closed', member_ids: [wes.id] })).data;

  // ---- creating ----
  const base = { title: `Weekly ${stamp}`, starts_at: day(2), duration_minutes: 45, timezone: 'UTC' };
  ok('a start time is needed', (await olga.post('/meetings', { ...base, starts_at: undefined })).status === 400);
  ok('5 minutes at least', (await olga.post('/meetings', { ...base, duration_minutes: 2 })).status === 400);
  ok('time zones are checked', (await olga.post('/meetings', { ...base, timezone: 'Mars/Base' })).status === 400);
  ok('people must exist', (await olga.post('/meetings', { ...base, members: [{ user_id: '00000000-0000-4000-8000-000000000000' }] })).status === 400);
  ok('roles are checked', (await olga.post('/meetings', { ...base, members: [{ user_id: kai.id, role: 'boss' }] })).status === 400);
  ok('only for workgroups you work in', (await ned.post('/meetings', { ...base, project_id: group.id })).status === 400);
  let r = await olga.post('/meetings', { ...base, description: 'Status of the week', place: 'Room 2', project_id: group.id, members: [{ user_id: kai.id, role: 'keeper' }, { user_id: mia.id }] });
  const m = r.data;
  ok('a meeting', r.status === 200 && m.state === 'prepare' && m.my_role === 'owner' && m.members.length === 3 && m.members[0].role === 'owner' && m.members[1].role === 'keeper', r.data);
  ok('on the calendar, with the people invited', m.event_id && (await mia.get(`/calendar/events/${m.event_id}`)).status === 200 && (await kai.get(`/calendar/events/${m.event_id}`)).data.title === `Weekly ${stamp}`);
  ok('they are told', await notified(mia, new RegExp(`^Meeting “Weekly ${stamp}”`)));

  // ---- who sees it ----
  ok('outsiders do not', (await ned.get(`/meetings/${m.id}`)).status === 404 && !(await ned.get('/meetings')).data.items.some((x: any) => x.id === m.id));
  r = await wes.get(`/meetings/${m.id}`);
  ok('the workgroup does, without taking part', r.status === 200 && r.data.my_role === 'viewer' && !r.data.can.propose && !r.data.can.run, r.data.can);
  ok('in her upcoming meetings', (await mia.get('/meetings?scope=upcoming')).data.items.some((x: any) => x.id === m.id && x.my_role === 'member'));
  ok('in the workgroup\'s list', (await wes.get(`/meetings?scope=all&project_id=${group.id}`)).data.items.some((x: any) => x.id === m.id));

  // ---- the agenda ----
  r = await mia.post(`/meetings/${m.id}/agenda`, { title: `Hiring ${stamp}`, description: 'Two open positions', responsible_ids: [mia.id] });
  ok('a member proposes a topic', r.status === 200 && r.data.agenda.length === 1 && r.data.agenda[0].proposed_by.id === mia.id, r.data);
  const hiring = r.data.agenda[0];
  ok('viewers do not propose', (await wes.post(`/meetings/${m.id}/agenda`, { title: 'x' })).status === 403);
  ok('outsiders do not either', (await ned.post(`/meetings/${m.id}/agenda`, { title: 'x' })).status === 404);
  ok('presenters take part in the meeting', (await olga.post(`/meetings/${m.id}/agenda`, { title: 'x', responsible_ids: [ned.id] })).status === 400);
  r = await olga.post(`/meetings/${m.id}/agenda`, { title: `Budget ${stamp}`, responsible_ids: [kai.id], duration_minutes: 15 });
  const budget = r.data.agenda.find((a: any) => a.title === `Budget ${stamp}`);
  ok('a topic with a presenter and a time', budget?.responsible[0]?.name === 'Kai Meets' && budget.duration_minutes === 15, budget);
  ok('the presenter is asked for a report', await notified(kai, `You present “Budget ${stamp}” at “Weekly ${stamp}”, ${new Date(day(2)).toUTCString().slice(0, 3)} ${new Date(day(2)).getUTCDate()} ${new Date(day(2)).toUTCString().slice(8, 11)}, 10:00`));
  r = await olga.post(`/meetings/${m.id}/agenda`, { title: `Q3 numbers ${stamp}`, parent_id: budget.id });
  const sub = r.data.agenda.find((a: any) => a.parent_id === budget.id);
  ok('a sub-point', !!sub);
  ok('one level of sub-points', (await olga.post(`/meetings/${m.id}/agenda`, { title: 'x', parent_id: sub.id })).status === 400);
  ok('the proposer rewords her topic', (await mia.patch(`/meetings/agenda/${hiring.id}`, { title: `Hiring plan ${stamp}` })).data.agenda.some((a: any) => a.title === `Hiring plan ${stamp}`));
  ok('…but does not record decisions', (await mia.patch(`/meetings/agenda/${hiring.id}`, { decision: 'x' })).status === 403);
  ok('…or touch other topics', (await mia.patch(`/meetings/agenda/${budget.id}`, { title: 'x' })).status === 403);

  // ---- reports ----
  ok('only presenters report', (await mia.put(`/meetings/agenda/${budget.id}/report`, { body: 'x' })).status === 400);
  await kai.put(`/meetings/agenda/${budget.id}/report`, { body: 'We spent 80%.' });
  r = await kai.put(`/meetings/agenda/${budget.id}/report`, { body: 'We spent 82% of the budget.' });
  const rep = r.data.agenda.find((a: any) => a.id === budget.id).reports;
  ok('a report, replaced when written again', rep.length === 1 && rep[0].body === 'We spent 82% of the budget.' && rep[0].name === 'Kai Meets', rep);

  // ---- order ----
  ok('members do not reorder', (await mia.put(`/meetings/${m.id}/agenda/order`, { ids: [budget.id, hiring.id] })).status === 403);
  r = await olga.put(`/meetings/${m.id}/agenda/order`, { ids: [budget.id, hiring.id] });
  ok('the owner reorders', r.data.agenda.filter((a: any) => !a.parent_id).map((a: any) => a.id).join() === [budget.id, hiring.id].join(), r.data.agenda.map((a: any) => a.title));

  // ---- moving it, people ----
  ok('members do not change the meeting', (await mia.patch(`/meetings/${m.id}`, { place: 'x' })).status === 403);
  r = await kai.patch(`/meetings/${m.id}`, { starts_at: day(3, 14) });
  ok('the keeper moves it', r.status === 200 && r.data.starts_at === day(3, 14).replace('.000Z', 'Z'), r.data.starts_at);
  const ev = (await mia.get(`/calendar/events/${m.id && r.data.event_id}`)).data;
  ok('the calendar follows', ev.start === day(3, 14) || new Date(ev.start ?? ev.starts_at).toISOString() === day(3, 14), ev);
  ok('people are told it moved', await notified(mia, new RegExp(`^Meeting “Weekly ${stamp}” moved to`)));
  ok('the keeper does not change the people', (await kai.put(`/meetings/${m.id}/members`, [])).status === 403);
  r = await olga.put(`/meetings/${m.id}/members`, [{ user_id: kai.id, role: 'keeper' }, { user_id: mia.id }, { user_id: ned.id }]);
  ok('the owner adds someone', r.data.members.length === 4 && (await ned.get(`/meetings/${m.id}`)).status === 200);
  ok('…who is invited on the calendar too', (await ned.get(`/calendar/events/${m.event_id}`)).status === 200);
  ok('a meeting has one owner', (await olga.put(`/meetings/${m.id}/members`, [{ user_id: kai.id, role: 'owner' }, { user_id: mia.id, role: 'owner' }])).status === 400);
  r = await olga.put(`/meetings/${m.id}/members`, [{ user_id: kai.id, role: 'keeper' }, { user_id: mia.id }]);
  ok('…and removes him', r.data.members.length === 3 && (await ned.get(`/meetings/${m.id}`)).status === 404);

  // ---- the meeting ----
  ok('members do not start it', (await mia.post(`/meetings/${m.id}/start`)).status === 403);
  r = await kai.post(`/meetings/${m.id}/start`);
  ok('started', r.data.state === 'in_progress' && r.data.started_at);
  r = await kai.post(`/meetings/${m.id}/attendance`, { user_id: mia.id, attended: true });
  await kai.post(`/meetings/${m.id}/attendance`, { user_id: olga.id, attended: false });
  ok('attendance', r.data.members.find((x: any) => x.id === mia.id).attended === true);
  r = await kai.patch(`/meetings/agenda/${budget.id}`, { decision: 'Freeze new spending until October.', status: 'done' });
  ok('a decision, topic settled', r.data.agenda.find((a: any) => a.id === budget.id).status === 'done');
  await kai.patch(`/meetings/agenda/${hiring.id}`, { decision: 'Post the first role this week.' });
  r = await kai.post(`/meetings/agenda/${hiring.id}/task`, { deadline: day(9) });
  const task = r.data.agenda.find((a: any) => a.id === hiring.id).task;
  ok('a task from the topic', task?.title === `Hiring plan ${stamp}`, r.data);
  const t = (await mia.get(`/tasks/${task.id}`)).data;
  ok('for its presenter, with the decision (outside the workgroup: the keeper is not in it)', t.responsible_id === mia.id && t.project_id === null && t.description.includes('Post the first role') && t.description.includes(`/meetings/${m.id}`), t);
  ok('one task per topic', (await kai.post(`/meetings/agenda/${hiring.id}/task`, {})).status === 409);
  r = await olga.post(`/meetings/agenda/${sub.id}/task`, { responsible_id: kai.id });
  const subTask = r.data.agenda.find((a: any) => a.id === sub.id).task;
  ok('made by someone in the workgroup, the task goes in the workgroup', (await olga.get(`/tasks/${subTask.id}`)).data.project_id === group.id);

  // ---- closing ----
  ok('members do not close it', (await mia.post(`/meetings/${m.id}/close`)).status === 403);
  await kai.patch(`/meetings/${m.id}`, { minutes: 'Short meeting; budget frozen.' });
  r = await kai.post(`/meetings/${m.id}/close`);
  ok('closed', r.data.state === 'closed' && r.data.closed_at);
  ok('everyone gets the minutes', await notified(mia, `Minutes of “Weekly ${stamp}” are ready`) && await notified(olga, `Minutes of “Weekly ${stamp}” are ready`));
  ok('closed: no new topics', (await olga.post(`/meetings/${m.id}/agenda`, { title: 'x' })).status === 400);
  ok('…no changes', (await olga.patch(`/meetings/${m.id}`, { title: 'x' })).status === 400);
  ok('…but the minutes text can be corrected', (await kai.patch(`/meetings/${m.id}`, { minutes: 'Short meeting; budget frozen until October.' })).data.minutes === 'Short meeting; budget frozen until October.');
  ok('…no reports', (await kai.put(`/meetings/agenda/${budget.id}/report`, { body: 'late' })).status === 400);
  ok('only the owner reopens', (await kai.post(`/meetings/${m.id}/reopen`)).status === 403);
  ok('reopened', (await olga.post(`/meetings/${m.id}/reopen`)).data.state === 'in_progress');
  await kai.post(`/meetings/${m.id}/close`);

  // ---- the next meeting ----
  ok('the next one comes later', (await olga.post(`/meetings/${m.id}/next`, { starts_at: day(1) })).status === 400);
  r = await olga.post(`/meetings/${m.id}/next`, { starts_at: day(10) });
  const next = r.data;
  ok('the next meeting: same people and workgroup', r.status === 200 && next.parent_id === m.id && next.members.length === 3 && next.project?.id === group.id && next.previous?.id === m.id, r.data);
  ok('…with the unfinished topic, not the settled one', next.agenda.length === 1 && next.agenda[0].topic_id === hiring.topic_id && next.agenda[0].responsible[0].id === mia.id, next.agenda);
  ok('…marked carried in the first meeting', (await olga.get(`/meetings/${m.id}`)).data.agenda.find((a: any) => a.id === hiring.id).status === 'carried');
  ok('…and linked from it', (await olga.get(`/meetings/${m.id}`)).data.next?.id === next.id);
  ok('planned once', (await olga.post(`/meetings/${m.id}/next`, { starts_at: day(11) })).status === 409);
  r = await mia.get(`/meetings/topics/${hiring.topic_id}/history`);
  ok('the topic\'s history across meetings', r.data.items.length === 2 && r.data.items[0].decision === 'Post the first role this week.' && r.data.items[0].status === 'carried' && r.data.items[1].meeting.id === next.id, r.data);
  ok('…not for outsiders', (await ned.get(`/meetings/topics/${hiring.topic_id}/history`)).status === 404);

  // ---- files ----
  const fd = new FormData();
  fd.append('file', new Blob(['agenda notes']), `notes-${stamp}.txt`);
  const file = (await olga.req('POST', '/files', fd) as any).data;
  ok('files on the meeting', (await olga.patch(`/meetings/${next.id}`, { file_ids: [file.id] })).data.files[0]?.name === `notes-${stamp}.txt`);
  ok('…people open them', ((await mia.req('GET', `/files/${file.id}/content`, undefined, true)) as Response).status === 200);
  ok('…outsiders do not', ((await ned.req('GET', `/files/${file.id}/content`, undefined, true)) as Response).status !== 200);

  // ---- cancelling ----
  ok('members do not cancel', (await mia.del(`/meetings/${next.id}`)).status === 403);
  const nextEvent = next.event_id;
  ok('the owner cancels', (await olga.del(`/meetings/${next.id}`)).status === 200 && (await mia.get(`/meetings/${next.id}`)).status === 404);
  ok('…the calendar event goes', (await mia.get(`/calendar/events/${nextEvent}`)).status === 404);
  ok('…and people are told', await notified(mia, new RegExp(`^Meeting “Weekly ${stamp}” on .* was cancelled$`)));
  ok('past meetings', (await mia.get('/meetings?scope=past')).data.items.some((x: any) => x.id === m.id) || (await mia.get('/meetings?scope=all')).data.items.some((x: any) => x.id === m.id && x.state === 'closed'));
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
