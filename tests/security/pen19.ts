// Signed in, bending the rules of the work itself: approving one's own leave, editing or
// deleting colleagues' messages, posts, comments and tasks, voting twice, promoting oneself
// in a workgroup or chat, widening one's own share of a file, changing an event one is only
// invited to, reading a help desk's internal notes or a test's right answers, closing
// someone else's meeting, marking other people's notifications, taking over another email
// address — each tried by someone who may take part, but not do that.
import { admin, person, raw, ok, finish, stamp } from './lib.ts';

const ann = await person('Ann');
const bob = await person('Bob');
const cat = await person('Cat');
const refused = (r: any) => r.status >= 400;
const start = new Date(Date.now() + 4 * 86400e3);

// ---- leave: approving one's own ----
const myLeave = (await bob.post('/hr/absences', { kind: 'vacation', starts_on: '2027-06-07', ends_on: '2027-06-08' })).data;
ok('nobody approves their own leave', refused(await bob.post(`/hr/absences/${myLeave.id}/decide`, { approve: true })));
const annLeave = (await ann.post('/hr/absences', { kind: 'vacation', starts_on: '2027-06-14', ends_on: '2027-06-15' })).data;
ok('…or a colleague\'s, without being their manager', refused(await bob.post(`/hr/absences/${annLeave.id}/decide`, { approve: true })));
ok('…or cancels a colleague\'s', refused(await bob.post(`/hr/absences/${annLeave.id}/cancel`, {})));
ok('their leave is still waiting', JSON.stringify((await bob.get('/hr/absences/mine')).data).includes(myLeave.id) && !/approved/.test(JSON.stringify((await bob.get('/hr/absences/mine')).data.items?.find?.((a: any) => a.id === myLeave.id)?.status ?? '')));

// ---- a chat Bob is in ----
const chat = (await ann.post('/im/chats', { title: `Team ${stamp}`, members: [bob.id, cat.id] })).data;
const annMsg = (await ann.post(`/im/chats/${chat.id}/messages`, { body: `Ann says ${stamp}` })).data;
const mid = annMsg.id ?? annMsg.message?.id;
ok('Bob can\'t edit Ann\'s message', refused(await bob.patch(`/im/messages/${mid}`, { body: 'edited by bob' })));
ok('…or delete it', refused(await bob.del(`/im/messages/${mid}`)));
ok('…or make himself the chat\'s admin', refused(await bob.put(`/im/chats/${chat.id}/members/${bob.id}`, { role: 'admin', is_admin: true })) || !JSON.stringify((await ann.get(`/im/chats/${chat.id}`)).data).match(new RegExp(`"${bob.id}"[^}]*"role":"(admin|owner)"`)));
ok('…or remove Cat', refused(await bob.del(`/im/chats/${chat.id}/members/${cat.id}`)) || JSON.stringify((await ann.get(`/im/chats/${chat.id}`)).data).includes(cat.id));
ok('…or rename the chat', refused(await bob.patch(`/im/chats/${chat.id}`, { title: 'Bob rules' })) || (await ann.get(`/im/chats/${chat.id}`)).data.title !== 'Bob rules');
ok('Ann\'s message is as she wrote it', JSON.stringify((await ann.get(`/im/chats/${chat.id}/messages`)).data).includes(`Ann says ${stamp}`));
const poll = (await ann.post(`/im/chats/${chat.id}/polls`, { question: 'Lunch?', options: ['Pizza', 'Salad'] })).data;
const pid = poll.id ?? poll.message?.id;
if (pid) {
  await bob.post(`/im/messages/${pid}/vote`, { options: [0] });
  await bob.post(`/im/messages/${pid}/vote`, { options: [0] });
  const counts = JSON.stringify((await ann.get(`/im/messages/${pid}`)).data);
  ok('voting twice in a chat poll counts once', !/"votes":\s*2|"count":\s*2/.test(counts), counts.slice(0, 300));
  ok('only the poll\'s author closes it', refused(await bob.post(`/im/messages/${pid}/close-poll`, {})));
}

// ---- the feed ----
const post = (await ann.post('/feed', { body: `Ann news ${stamp}`, poll: { question: 'Go?', options: ['yes', 'no'] } })).data;
ok('Bob can\'t edit Ann\'s post', refused(await bob.patch(`/feed/${post.id}`, { body: 'bob was here' })));
ok('…or delete it', refused(await bob.del(`/feed/${post.id}`)));
const c = (await ann.post(`/feed/${post.id}/comments`, { body: `Ann comment ${stamp}` })).data;
const cid = c.id ?? c.comment?.id ?? c.items?.at?.(-1)?.id;
ok('…or her comment', refused(await bob.patch(`/feed/comments/${cid}`, { body: 'bob' })) && refused(await bob.del(`/feed/comments/${cid}`)));
await bob.post(`/feed/${post.id}/vote`, { option: 0 });
await bob.post(`/feed/${post.id}/vote`, { option: 0 });
await bob.post(`/feed/${post.id}/vote`, { option: 1 });
const fp = (await ann.get(`/feed/${post.id}`)).data;
const total = JSON.stringify(fp.poll ?? fp).match(/"votes":(\d+)/g)?.map((m: string) => Number(m.split(':')[1])).reduce((a: number, b: number) => a + b, 0) ?? 0;
ok('one person, one vote in a feed poll', total <= 1, fp.poll ?? fp);
ok('the post is as Ann wrote it', fp.body === `Ann news ${stamp}`);

// ---- tasks: an observer, a responsible person ----
const task = (await ann.post('/tasks', { title: `Ann task ${stamp}`, responsible_id: cat.id, observer_ids: [bob.id], auditors: [bob.id] })).data;
ok('an observer can\'t rename the task', refused(await bob.patch(`/tasks/${task.id}`, { title: 'bob' })) || (await ann.get(`/tasks/${task.id}`)).data.title === `Ann task ${stamp}`);
ok('…or make himself responsible', refused(await bob.patch(`/tasks/${task.id}`, { responsible_id: bob.id })) || (await ann.get(`/tasks/${task.id}`)).data.responsible_id !== bob.id);
ok('…or delete it', refused(await bob.del(`/tasks/${task.id}`)) && (await ann.get(`/tasks/${task.id}`)).status === 200);
const tc = (await ann.post(`/tasks/${task.id}/comments`, { body: `Ann task comment ${stamp}` })).data;
const tcid = tc.id ?? tc.comment?.id;
ok('…or change Ann\'s comment', !tcid || (refused(await bob.patch(`/tasks/${task.id}/comments/${tcid}`, { body: 'bob' })) && refused(await bob.del(`/tasks/${task.id}/comments/${tcid}`))));
const catTask = (await ann.post('/tasks', { title: `Cat only ${stamp}`, responsible_id: cat.id })).data;
ok('someone outside a task can\'t complete it', refused(await bob.post(`/tasks/${catTask.id}/status`, { status: 'done' })));

// ---- workgroups ----
const group = (await ann.post('/projects', { name: `Ann team ${stamp}`, kind: 'group', visibility: 'closed', member_ids: [bob.id] })).data;
ok('a member can\'t make himself owner', refused(await bob.put(`/projects/${group.id}/members/${bob.id}`, { role: 'owner' })) || !JSON.stringify((await ann.get(`/projects/${group.id}`)).data.members ?? []).includes(`"${bob.id}","role":"owner"`));
ok('…or remove the owner', refused(await bob.del(`/projects/${group.id}/members/${ann.id}`)));
ok('…or open the group to everyone', refused(await bob.patch(`/projects/${group.id}`, { visibility: 'open' })) || (await ann.get(`/projects/${group.id}`)).data.visibility === 'closed');
ok('…or delete it', refused(await bob.del(`/projects/${group.id}`)) && (await ann.get(`/projects/${group.id}`)).status === 200);
const closed = (await ann.post('/projects', { name: `Ann closed ${stamp}`, kind: 'group', visibility: 'closed' })).data;
ok('joining a closed group needs approval', refused(await cat.post(`/projects/${closed.id}/join`, {})) || !JSON.stringify((await ann.get(`/projects/${closed.id}`)).data.members ?? []).includes(cat.id));

// ---- drive: a view share stays a view share ----
const up = async (c: any, name: string) => { const f = new FormData(); f.append('file', new Blob(['ann drive']), name); return (await c.post('/files', f)).data.id; };
const node = (await ann.post('/drive/files', { file_id: await up(ann, `share-${stamp}.txt`), name: `share-${stamp}.txt` })).data;
await ann.put(`/drive/nodes/${node.id}/shares`, [{ user_id: bob.id, level: 'view' }]);
ok('a viewer can\'t raise his own level', refused(await bob.put(`/drive/nodes/${node.id}/shares`, [{ user_id: bob.id, level: 'edit' }])));
ok('…or share it on', refused(await bob.put(`/drive/nodes/${node.id}/shares`, [{ user_id: bob.id, level: 'view' }, { user_id: cat.id, level: 'view' }])));
ok('…or make a public link', refused(await bob.post(`/drive/nodes/${node.id}/links`, {})));
ok('…or rename or delete it', refused(await bob.patch(`/drive/nodes/${node.id}`, { name: 'bob.txt' })) && refused(await bob.del(`/drive/nodes/${node.id}`)));
ok('…or upload a new version', refused(await bob.post(`/drive/nodes/${node.id}/versions`, { file_id: await up(bob, 'bob.txt') })));
ok('the file is Ann\'s as before', (await (await ann.req('GET', `/drive/nodes/${node.id}/download`, undefined, true) as Response).text()) === 'ann drive');

// ---- calendar: invited is not organiser ----
const ev = (await ann.post('/calendar/events', { title: `Ann meeting ${stamp}`, timezone: 'UTC', start: start.toISOString(), end: new Date(+start + 3600e3).toISOString(), attendees: [bob.id] })).data;
ok('a guest of an event can\'t change it', refused(await bob.patch(`/calendar/events/${ev.id}`, { title: 'bob', timezone: 'UTC', start: start.toISOString(), end: new Date(+start + 7200e3).toISOString() })) || (await ann.get(`/calendar/events/${ev.id}`)).data.title === `Ann meeting ${stamp}`);
ok('…or delete it for everyone', (await ann.get(`/calendar/events/${ev.id}`)).status === 200 || refused(await bob.del(`/calendar/events/${ev.id}`)));
ok('…or put events in Ann\'s calendar', refused(await bob.post('/calendar/events', { title: 'planted', calendar_id: ann.id, timezone: 'UTC', start: start.toISOString(), end: new Date(+start + 3600e3).toISOString() })));

// ---- boards and knowledge base: viewers stay viewers ----
const board = (await ann.post('/boards', { title: `Ann board ${stamp}` })).data;
await ann.put(`/boards/${board.id}/access`, { users: [{ user_id: bob.id, level: 'view' }], everyone: null });
ok('a board viewer can\'t draw on it', refused(await bob.put(`/boards/${board.id}/scene`, { elements: [{ id: 'x', type: 'rectangle' }] })));
ok('…or share it', refused(await bob.put(`/boards/${board.id}/access`, { users: [{ user_id: bob.id, level: 'edit' }] })) && refused(await bob.post(`/boards/${board.id}/links`, { level: 'edit' })));

// ---- the help desk: internal notes stay internal ----
const ticket = (await bob.post('/support/tickets', { title: `Bob's ticket ${stamp}`, body: 'my laptop' })).data;
const tid = ticket.id ?? ticket.ticket?.id;
await admin.post(`/support/tickets/${tid}/messages`, { body: `INTERNAL note ${stamp}`, note: true });
await admin.post(`/support/tickets/${tid}/messages`, { body: `Public reply ${stamp}` });
const seen = JSON.stringify((await bob.get(`/support/tickets/${tid}`)).data);
ok('the requester gets the reply', seen.includes(`Public reply ${stamp}`));
ok('…but never the agents\' internal note', !seen.includes(`INTERNAL note ${stamp}`));
ok('a requester can\'t post an internal note', refused(await bob.post(`/support/tickets/${tid}/messages`, { body: 'x', note: true })) || !JSON.stringify((await admin.get(`/support/tickets/${tid}`)).data).match(/"note":true[^}]*"body":"x"|"body":"x"[^}]*"note":true/));
ok('…or assign it to himself as the agent', refused(await bob.patch(`/support/tickets/${tid}`, { assignee_id: bob.id, priority: 'urgent' })) || (await admin.get(`/support/tickets/${tid}`)).data?.assignee_id !== bob.id);
ok('a colleague can\'t read Bob\'s ticket', refused(await cat.get(`/support/tickets/${tid}`)));

// ---- meetings ----
const meeting = (await ann.post('/meetings', { title: `Ann meeting ${stamp}`, starts_at: start.toISOString(), duration_minutes: 30, timezone: 'UTC', members: [{ user_id: bob.id }] })).data;
ok('a member can\'t close the meeting', refused(await bob.post(`/meetings/${meeting.id}/close`, {})));
ok('…or mark others present or absent', refused(await bob.post(`/meetings/${meeting.id}/attendance`, { user_id: ann.id, present: false, status: 'absent' })));
ok('…or change who is in it', refused(await bob.put(`/meetings/${meeting.id}/members`, { members: [{ user_id: bob.id, role: 'owner' }] })));

// ---- training: the right answers stay hidden until the end ----
const course = (await admin.post('/training/courses', { title: `Pen course ${stamp}`, description: 'x', to_all: true })).data;
const cid2 = course.id ?? course.course?.id;
await admin.post(`/training/courses/${cid2}/questions`, { kind: 'single', text: `Capital? ${stamp}`, points: 10, answers: [{ text: 'Paris', is_correct: true }, { text: 'Lyon' }], explanation: `SECRET explanation ${stamp}` });
const test = (await admin.post(`/training/courses/${cid2}/tests`, { title: 'Final', navigation: 'skip', show_answers: 'at_end', attempt_limit: 1, pass_percent: 50 })).data;
await admin.patch(`/training/courses/${cid2}`, { is_published: true });
await bob.post(`/training/courses/${cid2}/enrol`);
const attempt = (await bob.post(`/training/tests/${test.id}/attempts`)).data;
const shown = JSON.stringify(attempt);
ok('a test in progress shows no right answer and no explanation', !!attempt.id && !/"(is_correct|correct)":true|"right_answers":\[/.test(shown) && !shown.includes(`SECRET explanation ${stamp}`), shown.slice(200, 900));
ok('…nor does the test itself', !/is_correct|"correct":true/.test(JSON.stringify((await bob.get(`/training/tests/${test.id}`)).data)) && !JSON.stringify((await bob.get(`/training/tests/${test.id}`)).data).includes(`SECRET explanation ${stamp}`));
ok('a learner can\'t grant himself more attempts', refused(await bob.post(`/training/tests/${test.id}/extra-attempts`, { user_id: bob.id, extra: 5 })));
ok('…or review (grade) his own attempt', refused(await bob.post(`/training/attempts/${attempt.id}/review`, { question_id: attempt.questions?.[0]?.question_id ?? attempt.id, points: 10 })));

// ---- notifications, profile, email ----
const notes = (await ann.get('/notifications')).data.items ?? [];
if (notes[0]) {
  await bob.post(`/notifications/${notes[0].id}/read`, {});
  const after = ((await ann.get('/notifications')).data.items ?? []).find((n: any) => n.id === notes[0].id);
  ok('Bob can\'t mark Ann\'s notifications read', !after || after.read_at === notes[0].read_at);
}
ok('nobody takes over another person\'s email address', refused(await bob.patch('/profile', { email: ann.email })) || (await bob.get('/auth/me')).data.email === bob.email);
ok('…or the admin\'s', refused(await bob.patch('/profile', { email: 'admin@acme.test' })) || (await bob.get('/auth/me')).data.email === bob.email);
ok('Ann still signs in with her address', (await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: ann.email, password: 'password123' }) })).status === 200);
ok('a password change needs the current one', refused(await bob.post('/auth/change-password', { current_password: 'wrong-one', new_password: 'password999' })));
ok('turning two-step sign-in off needs the password', refused(await bob.post('/auth/2fa/disable', { password: 'wrong-one' })));
ok('nobody changes a colleague\'s picture', refused(await bob.post(`/users/${ann.id}/avatar`, {})) && refused(await bob.patch(`/users/${ann.id}`, { first_name: 'Hacked' })));

// ---- approvals: nobody approves their own request ----
const sol = await admin.post('/lists/solutions/expenses', { name: `Pen expenses ${stamp}`, finance: [cat.id] });
const listId = sol.data?.id;
if (listId) {
  const fields = (await admin.get(`/lists/${listId}`)).data.fields ?? [];
  const values: Record<string, unknown> = {};
  for (const f of fields) if (f.required || f.kind !== 'file') values[f.code] = f.kind === 'file' ? undefined : f.kind === 'number' || f.kind === 'money' ? 50 : f.kind === 'date' ? '2027-01-10' : f.kind === 'select' ? f.options?.[0] : f.kind === 'user' ? bob.id : 'x';
  const made = await bob.post(`/lists/${listId}/items`, { title: `Bob expense ${stamp}`, values });
  if (made.status !== 200) console.log('expense item:', made.status, JSON.stringify(made.data).slice(0, 300), JSON.stringify(fields).slice(0, 400));
  const item = made.data;
  await new Promise((r) => setTimeout(r, 800));
  const mine = (await bob.get('/approvals')).data.items ?? [];
  const catApprovals = (await cat.get('/approvals')).data.items ?? [];
  const step = catApprovals.find((a: any) => JSON.stringify(a).includes(item?.id ?? '§')) ?? catApprovals[0];
  ok('the request waits for someone else', !!item?.id);
  if (step) ok('Bob can\'t approve the step meant for Cat', refused(await bob.patch(`/approvals/${step.id}`, { approve: true })));
  ok('…and has no approval of his own request', !mine.some((a: any) => JSON.stringify(a).includes(item?.id ?? '§')));
}

await finish();
