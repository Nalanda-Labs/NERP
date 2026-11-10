// Chats and calls for tasks and calendar events: a task's chat holds its creator, responsible
// person, assistants and watchers, an event's its organiser and guests; both follow changes to
// the people and the title, can't be edited from the messenger, are only for the people on
// them, go away with the task or event, and a call in one is joined, not doubled.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Linked${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat, dan, eve] = await Promise.all(['Ann', 'Bob', 'Cat', 'Dan', 'Eve'].map(user));
const people = async (c: Client, chat: string) => {
  const r = await c.get(`/im/chats/${chat}`);
  return r.status === 200 ? (r.data.people ?? []).map((p: any) => p.id).sort() : null;
};
const ids = (...cs: Client[]) => cs.map((c) => c.id).sort();
const texts = async (c: Client, chat: string) => ((await c.get(`/im/chats/${chat}/messages`)).data.items ?? []).map((m: any) => m.body);

try {
  // ---- a task's chat ----
  const task = (await ann.post('/tasks', { title: `Ship it ${stamp}`, responsible_id: bob.id, assistant_ids: [cat.id] })).data;
  let r = await bob.post(`/tasks/${task.id}/chat`);
  const tc = r.data.chat_id;
  ok('the task\'s chat', r.status === 200 && !!tc, r.data);
  ok('the same chat for everyone, every time', (await ann.post(`/tasks/${task.id}/chat`)).data.chat_id === tc && (await cat.post(`/tasks/${task.id}/chat`)).data.chat_id === tc);
  r = await ann.get(`/im/chats/${tc}`);
  ok('named after the task, linked to it', r.data.title === `Task #${task.number}: Ship it ${stamp}` && r.data.task_id === task.id && r.data.kind === 'group', r.data);
  ok('creator, responsible person and assistants', JSON.stringify(await people(ann, tc)) === JSON.stringify(ids(ann, bob, cat)), await people(ann, tc));
  ok('starts with a note of what it is', (await texts(ann, tc)).some((t: string) => t.includes(`Task #${task.number}`)));
  ok('not for someone outside the task', (await dan.post(`/tasks/${task.id}/chat`)).status >= 400 && (await dan.get(`/im/chats/${tc}`)).status === 404);
  await dan.post(`/tasks/${task.id}/watch`).catch(() => {});
  const watching = (await dan.get(`/tasks/${task.id}`)).status === 200;
  if (watching) {
    ok('watching joins the chat', (await people(ann, tc))?.includes(dan.id), await people(ann, tc));
    await dan.del(`/tasks/${task.id}/watch`);
    ok('and stopping leaves it', !(await people(ann, tc))?.includes(dan.id));
  }
  await ann.patch(`/tasks/${task.id}`, { responsible_id: eve.id, assistant_ids: [], watcher_ids: [dan.id] });
  ok('the people change with the task', JSON.stringify(await people(ann, tc)) === JSON.stringify(ids(ann, dan, eve)), await people(ann, tc));
  ok('those taken off can no longer read it', (await bob.get(`/im/chats/${tc}`)).status === 404 && (await cat.get(`/im/chats/${tc}/messages`)).status === 404);
  ok('the chat says who came and went', (await texts(ann, tc)).some((t: string) => /joined/.test(t)) && (await texts(ann, tc)).some((t: string) => /left the task/.test(t)));
  await ann.patch(`/tasks/${task.id}`, { title: `Ship it now ${stamp}` });
  ok('renaming the task renames the chat', (await ann.get(`/im/chats/${tc}`)).data.title === `Task #${task.number}: Ship it now ${stamp}`);
  ok('members and name are not edited in the messenger', (await ann.patch(`/im/chats/${tc}`, { add: [bob.id] })).status === 400 && (await eve.post(`/im/chats/${tc}/leave`)).status === 400 && (await ann.patch(`/im/chats/${tc}`, { title: 'x' })).status === 400);
  ok('messages work as usual', (await eve.post(`/im/chats/${tc}/messages`, { body: `On it ${stamp}` })).status === 200 && (await texts(dan, tc)).includes(`On it ${stamp}`));

  // ---- a call in it ----
  r = await eve.post('/calls', { chat_id: tc });
  const call = r.data.id;
  ok('a call in the task\'s chat', r.status === 200 && !!call, r.data);
  await eve.post(`/calls/${call}/join`, {});
  ok('pressing Call again joins it, not a second call', (await ann.post('/calls', { chat_id: tc })).data.id === call);
  ok('not for those off the task', (await bob.post('/calls', { chat_id: tc })).status === 404);
  await eve.post(`/calls/${call}/leave`).catch(() => {});
  await ann.post(`/calls/${call}/leave`).catch(() => {});

  // ---- deleting the task ----
  r = await ann.del(`/tasks/${task.id}`);
  ok('deleting the task takes its chat', r.status === 200 && (await ann.get(`/im/chats/${tc}`)).status === 404 && !(await ann.get('/im/chats')).data.items?.some((c: any) => c.id === tc));
  const other = (await ann.post('/tasks', { title: `No chat ${stamp}`, responsible_id: bob.id })).data;
  ok('a task without a chat deletes as before', (await ann.del(`/tasks/${other.id}`)).status === 200);

  // ---- an event's chat ----
  const start = new Date(Date.now() + 86400e3);
  const evb = (b: any) => ({ timezone: 'UTC', start: start.toISOString(), end: new Date(+start + 3600e3).toISOString(), ...b });
  const ev = (await ann.post('/calendar/events', evb({ title: `Planning ${stamp}`, attendees: [bob.id, cat.id] }))).data;
  r = await cat.post(`/calendar/events/${ev.id}/chat`);
  const ec = r.data.chat_id;
  ok('the event\'s chat, for a guest', r.status === 200 && !!ec, r.data);
  r = await ann.get(`/im/chats/${ec}`);
  ok('organiser and guests, named after it', r.data.title === `Planning ${stamp}` && r.data.event_id === ev.id && JSON.stringify(await people(ann, ec)) === JSON.stringify(ids(ann, bob, cat)), r.data);
  await bob.post(`/calendar/events/${ev.id}/respond`, { status: 'declined' });
  ok('saying "not going" keeps you in (you may still want to read it)', (await people(ann, ec))?.includes(bob.id));
  ok('not for someone not invited', (await dan.post(`/calendar/events/${ev.id}/chat`)).status >= 400 && (await dan.get(`/im/chats/${ec}`)).status === 404);
  await ann.patch(`/calendar/events/${ev.id}`, evb({ title: `Planning day ${stamp}`, attendees: [cat.id, dan.id] }));
  ok('guests and title follow the event', JSON.stringify(await people(ann, ec)) === JSON.stringify(ids(ann, cat, dan)) && (await ann.get(`/im/chats/${ec}`)).data.title === `Planning day ${stamp}`, await people(ann, ec));
  r = await dan.post('/calls', { chat_id: ec });
  await dan.post(`/calls/${r.data.id}/join`, {});
  ok('the meeting\'s call', r.status === 200 && (await cat.post('/calls', { chat_id: ec })).data.id === r.data.id);
  await dan.post(`/calls/${r.data.id}/leave`).catch(() => {});
  await cat.post(`/calls/${r.data.id}/leave`).catch(() => {});
  ok('a guest cannot delete the event', (await cat.del(`/calendar/events/${ev.id}`)).status >= 400 && (await cat.get(`/im/chats/${ec}`)).status === 200);
  r = await ann.del(`/calendar/events/${ev.id}`);
  ok('deleting the event takes its chat', r.status === 200 && (await cat.get(`/im/chats/${ec}`)).status === 404);

  // ---- workgroup chats are unchanged ----
  const g = (await ann.post('/projects', { name: `Linked group ${stamp}`, kind: 'group' })).data;
  const gc = (await ann.post(`/projects/${g.id}/chat`)).data.chat_id;
  r = await ann.get(`/im/chats/${gc}`);
  ok('a workgroup\'s chat still works', r.status === 200 && r.data.project_id === g.id && !r.data.task_id && !r.data.event_id, r.data);
  ok('and a plain group chat is still edited in the messenger', await (async () => {
    const c = (await ann.post('/im/chats', { title: `Plain ${stamp}`, members: [bob.id] })).data;
    return (await ann.patch(`/im/chats/${c.id}`, { title: `Plainer ${stamp}` })).status === 200;
  })());
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
