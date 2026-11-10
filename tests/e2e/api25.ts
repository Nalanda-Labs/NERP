// Live chat: chat buttons (permissions, operators), a visitor's chat (start, messages, the
// CRM lead it makes, a known contact by email, round-robin operators, notifications),
// operators answering (visibility, taking, unread, live events), ending it (transcript on
// the CRM record) and spam limits.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Chat' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const eve = await user('Eve'); // not an operator
const visitor = new Client();

try {
  // ---- chat buttons ----
  ok('only managers set up chat buttons', (await ann.post('/chat/channels', { name: `X ${stamp}` })).status === 403);
  ok('operators must be active employees', (await admin.post('/chat/channels', { name: `Bad ${stamp}`, operators: ['00000000-0000-0000-0000-000000000123'] })).status === 400);
  ok('colours checked', (await admin.post('/chat/channels', { name: `Bad ${stamp}`, color: 'red' })).status === 400);
  let r = await admin.post('/chat/channels', { name: `Site ${stamp}`, operators: [ann.id, bob.id], greeting: 'Hello there' });
  const ch = r.data.items.find((c: any) => c.name === `Site ${stamp}`);
  ok('chat button made', ch && ch.key.length >= 12 && ch.operators.length === 2 && ch.ask_details, r.data);
  ok('operators see their channels', (await ann.get('/chat/channels')).data.items.some((c: any) => c.id === ch.id));
  ok('others do not', !(await eve.get('/chat/channels')).data.items.some((c: any) => c.id === ch.id));
  const pubInfo = (await visitor.get(`/public/chat/${ch.key}`)).data;
  ok('public button info', pubInfo.greeting === 'Hello there' && pubInfo.ask_details === true && !('operators' in pubInfo), pubInfo);
  ok('unknown button', (await visitor.get('/public/chat/nope')).status === 404);

  // ---- a visitor ----
  ok('name asked first', (await visitor.post(`/public/chat/${ch.key}/start`, {})).status === 400);
  ok('email checked', (await visitor.post(`/public/chat/${ch.key}/start`, { name: 'Zed', email: 'not-an-email' })).status === 400);
  r = await visitor.post(`/public/chat/${ch.key}/start`, { name: `Zoe Park ${stamp}`, email: `zoe.${stamp}@example.com`, page_url: 'https://acme.example/pricing' });
  const token = r.data.token;
  ok('chat started', r.status === 200 && token.length >= 30);
  ok('an empty chat is not listed', !(await ann.get('/chat/sessions')).data.items.some((s: any) => s.visitor_name === `Zoe Park ${stamp}`));
  ok('wrong token', (await visitor.get(`/public/chat/${ch.key}/session?token=nope`)).status === 404);
  ok('empty message', (await visitor.post(`/public/chat/${ch.key}/messages`, { token, body: '  ' })).status === 400);
  r = await visitor.post(`/public/chat/${ch.key}/messages`, { token, body: 'Do you ship to Spain?' });
  ok('first message', r.status === 200);

  // The CRM lead and the operator.
  const s1 = await waitFor(async () => (await admin.get('/chat/sessions')).data.items.find((s: any) => s.visitor_name === `Zoe Park ${stamp}` && s.item_id), 5000);
  ok('linked to a new lead, given to an operator', s1 && s1.item_code === 'lead' && [ann.id, bob.id].includes(s1.assigned_to) && s1.unread === 1 && s1.page_url === 'https://acme.example/pricing', s1);
  const lead = (await admin.get(`/crm/lead/items/${s1.item_id}`)).data;
  ok('the lead', lead.title === `Chat: Zoe Park ${stamp}` && lead.first_name === 'Zoe Park' && lead.last_name === stamp && lead.multifields.some((m: any) => m.value === `zoe.${stamp}@example.com`) && lead.assigned_to === s1.assigned_to && /Spain/.test(lead.comments), lead);
  const op = s1.assigned_to === ann.id ? ann : bob;
  const other = op === ann ? bob : ann;
  const note = await waitFor(async () => (await op.get('/notifications')).data.items?.find((n: any) => n.title.includes(`Zoe Park ${stamp}`)), 5000);
  ok('operator notified', note?.link === `/chats?session=${s1.id}`, note);

  // ---- operators ----
  ok('non-operators cannot see it', (await eve.get(`/chat/sessions/${s1.id}`)).status === 404 && !(await eve.get('/chat/sessions')).data.items.length);
  ok('both operators see it', (await other.get(`/chat/sessions/${s1.id}`)).status === 200);
  ok('counters', (await op.get('/chat/counters')).data.unread >= 1);
  ok('waiting means nobody has it', !(await op.get('/chat/sessions?who=waiting')).data.items.some((s: any) => s.id === s1.id) && (await op.get('/chat/sessions?who=mine')).data.items.some((s: any) => s.id === s1.id));
  r = await op.post(`/chat/sessions/${s1.id}/messages`, { body: 'Yes, we do!' });
  ok('reply', r.status === 200 && r.data.messages.length === 2 && r.data.session.unread === 0, r.data);
  let v = (await visitor.get(`/public/chat/${ch.key}/session?token=${token}`)).data;
  const opFirst = op === ann ? 'Ann' : 'Bob';
  ok('the visitor sees it, with first names only', v.status === 'open' && v.operator === opFirst && v.messages[1].author === 'operator' && v.messages[1].name === opFirst && !('user_id' in v.messages[1]), v);
  const after = v.messages[1].created_at;
  await visitor.post(`/public/chat/${ch.key}/messages`, { token, body: 'Great, thanks' });
  v = (await visitor.get(`/public/chat/${ch.key}/session?token=${token}&after=${encodeURIComponent(after)}`)).data;
  ok('polling returns only newer messages', v.messages.length === 1 && v.messages[0].body === 'Great, thanks', v);
  r = await other.post(`/chat/sessions/${s1.id}/take`, {});
  ok('another operator takes it', r.data.session.assigned_to === other.id && r.data.messages.at(-1).body.endsWith('joined the chat'), r.data);
  await other.post(`/chat/sessions/${s1.id}/read`, {});
  ok('read', (await other.get(`/chat/sessions/${s1.id}`)).data.session.unread === 0);

  // ---- ending ----
  r = await other.post(`/chat/sessions/${s1.id}/close`, {});
  ok('ended', r.data.session.status === 'closed' && r.data.messages.at(-1).author === 'system');
  ok('no more messages either way', (await visitor.post(`/public/chat/${ch.key}/messages`, { token, body: 'hello?' })).status === 400 && (await other.post(`/chat/sessions/${s1.id}/messages`, { body: 'x' })).status === 400);
  ok('the visitor sees it ended', (await visitor.get(`/public/chat/${ch.key}/session?token=${token}`)).data.status === 'closed');
  const tl = (await admin.get(`/crm/items/${s1.item_id}/timeline`)).data.items.find((e: any) => e.kind === 'chat');
  ok('transcript on the lead', tl && tl.data.count === 3 && tl.data.messages[0].who === `Zoe Park ${stamp}` && tl.data.messages[0].visitor && tl.data.channel === `Site ${stamp}`, tl);
  ok('closed list', (await other.get('/chat/sessions?status=closed')).data.items.some((s: any) => s.id === s1.id));

  // ---- a known contact; turn-taking ----
  const contact = (await admin.post('/crm/contact/items', { first_name: 'Kim', last_name: `Known${stamp}`, multifields: [{ kind: 'email', value_type: 'work', value: `kim.${stamp}@example.com` }] })).data;
  const t2 = (await visitor.post(`/public/chat/${ch.key}/start`, { name: 'Kim', email: `KIM.${stamp}@example.com` })).data.token;
  await visitor.post(`/public/chat/${ch.key}/messages`, { token: t2, body: 'Hi again' });
  const s2 = await waitFor(async () => (await admin.get('/chat/sessions')).data.items.find((s: any) => s.visitor_name === 'Kim' && s.item_id), 5000);
  ok('a known email links the contact', s2?.item_id === contact.id && s2.item_code === 'contact', s2);
  ok('the next operator in turn', s2.assigned_to !== s1.assigned_to, [s1.assigned_to, s2.assigned_to]);
  await visitor.post(`/public/chat/${ch.key}/end`, { token: t2 });
  ok('the visitor can end it', (await admin.get(`/chat/sessions/${s2.id}`)).data.session.status === 'closed');
  ok('ended by the visitor: transcript too', (await admin.get(`/crm/items/${contact.id}/timeline`)).data.items.some((e: any) => e.kind === 'chat' && e.author_name === null));

  // ---- switched off ----
  await admin.patch(`/chat/channels/${ch.id}`, { is_active: false });
  ok('a switched-off button takes no chats', (await visitor.get(`/public/chat/${ch.key}`)).status === 404 && (await visitor.post(`/public/chat/${ch.key}/start`, { name: 'X' })).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
