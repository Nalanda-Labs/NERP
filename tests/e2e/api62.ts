// Live chat operations: channel settings checked; consent before chatting; the automatic
// answer after the first message (and the away message out of hours); handing chats out in
// turn with a limit per operator, or offering them to everyone; saved replies; passing a
// chat to a colleague or another channel; the background jobs (passing on unanswered chats,
// telling waiting visitors, closing idle chats with a goodbye); the visitor's rating and a
// supervisor's review; figures and the CSV export.
import pg from 'pg';
import { Client, sleep } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const B = 'http://localhost:8080/api';
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Chat${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ola, pia, ray] = await Promise.all(['Ola', 'Pia', 'Ray'].map(user));
const pub = async (method: string, path: string, body?: unknown) => {
  const r = await fetch(B + path, { method, headers: body ? { 'content-type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined });
  const t = await r.text();
  let data: any; try { data = JSON.parse(t); } catch { data = t; }
  return { status: r.status, data };
};
const channelOf = (r: any, name: string) => r.data.items.find((c: any) => c.name === name);
const visitor = async (key: string, name: string, first: string, extra: object = {}) => {
  const t = (await pub('POST', `/public/chat/${key}/start`, { name, email: `${name.split(' ')[0].toLowerCase()}.${stamp}@example.com`, ...extra })).data.token;
  await pub('POST', `/public/chat/${key}/messages`, { token: t, body: first });
  return t;
};
const sessionOf = async (token: string) => (await db.query(`SELECT s.* FROM chat_sessions s WHERE s.token_hash = encode(sha256($1::bytea), 'hex')`, [token])).rows[0];

try {
  // ---- settings ----
  ok('settings are checked', (await admin.post('/chat/channels', { name: `Bad ${stamp}`, distribution: 'random' })).status === 400
    && (await admin.post('/chat/channels', { name: `Bad ${stamp}`, hours: [[9, 0, 60]] })).status === 400
    && (await admin.post('/chat/channels', { name: `Bad ${stamp}`, max_chats: 0 })).status === 400);
  let r = await admin.post('/chat/channels', {
    name: `Sales ${stamp}`, operators: [ola.id, pia.id], max_chats: 1, requeue_minutes: 1, auto_reply: 'Thanks! Someone will be with you shortly.',
    no_answer_minutes: 1, no_answer_message: 'Sorry for the wait, we are on it.', close_message: 'Thanks for chatting!', idle_close_minutes: 5,
    consent_text: 'I agree to the privacy notice.', ask_rating: true
  });
  const sales = channelOf(r, `Sales ${stamp}`);
  ok('a channel with its running settings', sales?.max_chats === 1 && sales.distribution === 'queue' && sales.consent_text === 'I agree to the privacy notice.', sales);
  r = await pub('GET', `/public/chat/${sales.key}`);
  ok('visitors see the consent text, and that it is open', r.data.online === true && r.data.consent_text === 'I agree to the privacy notice.' && r.data.ask_rating === true, r.data);
  ok('no consent, no chat', (await pub('POST', `/public/chat/${sales.key}/start`, { name: 'Ann' })).status === 400);

  // ---- handing out ----
  const t1 = await visitor(sales.key, `Ann ${stamp}`, 'Hello, prices please', { consent: true });
  let s1 = await sessionOf(t1);
  ok('consent is kept', !!s1.consent_at);
  let poll = (await pub('GET', `/public/chat/${sales.key}/session?token=${t1}`)).data;
  ok('the automatic answer follows the first message', poll.messages.some((m: any) => m.author === 'operator' && m.body.startsWith('Thanks! Someone')), poll.messages);
  const first = s1.assigned_to;
  ok('the chat goes to an operator', [ola.id, pia.id].includes(first), s1);
  const t2 = await visitor(sales.key, `Ben ${stamp}`, 'Hi', { consent: true });
  const s2 = await sessionOf(t2);
  ok('the next goes to the other operator', s2.assigned_to && s2.assigned_to !== first, s2);
  const t3 = await visitor(sales.key, `Cy ${stamp}`, 'Anyone?', { consent: true });
  ok('with both at their limit, the third waits for whoever is free', (await sessionOf(t3)).assigned_to === null);

  // ---- offered to everyone, out of hours ----
  r = await admin.post('/chat/channels', { name: `Support ${stamp}`, operators: [ola.id, ray.id], distribution: 'all', hours: [[((new Date().getUTCDay() + 2) % 7) + 1, 0, 60]], timezone: 'UTC', offline_message: 'We are closed now; we will answer tomorrow.' });
  const support = channelOf(r, `Support ${stamp}`);
  r = await pub('GET', `/public/chat/${support.key}`);
  ok('out of hours: the away message', r.data.online === false && r.data.offline_message === 'We are closed now; we will answer tomorrow.', r.data);
  const t4 = await visitor(support.key, `Dee ${stamp}`, 'Help!');
  const s4 = await sessionOf(t4);
  ok('offered to everyone: nobody\'s yet; marked as out of hours', s4.assigned_to === null && s4.offline === true, s4);
  poll = (await pub('GET', `/public/chat/${support.key}/session?token=${t4}`)).data;
  ok('the visitor gets the away message', poll.messages.some((m: any) => m.body === 'We are closed now; we will answer tomorrow.'), poll.messages);
  ok('every operator was told', JSON.stringify((await ray.get('/notifications')).data).includes(`Dee ${stamp} started a chat`) && JSON.stringify((await ola.get('/notifications')).data).includes(`Dee ${stamp} started a chat`));

  // ---- saved replies ----
  ok('saved replies are set up by managers', (await ola.post('/chat/canned', { shortcut: 'hi', title: 'Hi', body: 'x' })).status === 403);
  r = await admin.post('/chat/canned', { shortcut: `/hours${stamp}`, title: 'Opening hours', body: 'We are open 9 to 5.', channel_id: sales.id });
  ok('a saved reply with a short code', r.data.items.some((c: any) => c.shortcut === `hours${stamp}` && c.channel_id === sales.id), r.data);
  ok('short codes are unique', (await admin.post('/chat/canned', { shortcut: `hours${stamp}`, title: 'x', body: 'y', channel_id: sales.id })).status === 409);
  ok('operators can read them', (await ola.get('/chat/canned')).data.items.some((c: any) => c.shortcut === `hours${stamp}`));

  // ---- passing chats on ----
  const owner = first === ola.id ? ola : pia;
  const other = first === ola.id ? pia : ola;
  ok('only to an operator of the channel', (await owner.post(`/chat/sessions/${s1.id}/transfer`, { user_id: ray.id })).status === 400);
  r = await owner.post(`/chat/sessions/${s1.id}/transfer`, { user_id: other.id });
  ok('passed to a colleague', r.status === 200 && r.data.assigned_to === other.id, r.data);
  ok('who is told', JSON.stringify((await other.get('/notifications')).data).includes('passed you a chat'));
  r = await other.post(`/chat/sessions/${s1.id}/transfer`, { channel_id: support.id });
  ok('passed to another channel (offered to everyone there)', r.status === 200 && r.data.channel_id === support.id && r.data.assigned_to === null, r.data);
  const msgs = (await admin.get(`/chat/sessions/${s1.id}`)).data.messages;
  ok('the chat says so', msgs.some((m: any) => m.author === 'system' && /passed the chat to/.test(m.body)) && msgs.some((m: any) => /“Support/.test(m.body)), msgs.map((m: any) => m.body));

  // ---- background jobs ----
  // No per-operator limit now, so there's someone to pass Ben's chat to.
  await admin.patch(`/chat/channels/${sales.id}`, { max_chats: null });
  // Ben's chat: unanswered (offered long ago), waiting long, idle long.
  await db.query(`UPDATE chat_sessions SET offered_at = now() - interval '10 minutes' WHERE id = $1`, [s2.id]);
  await db.query(`UPDATE chat_messages SET created_at = now() - interval '10 minutes' WHERE session_id = $1`, [s2.id]);
  const t5 = await visitor(sales.key, `Eve ${stamp}`, 'Still there?', { consent: true });
  const s5 = await sessionOf(t5);
  await db.query(`UPDATE chat_sessions SET last_message_at = now() - interval '1 hour' WHERE id = $1`, [s5.id]);
  const before2 = s2.assigned_to;
  let ben: any, eve: any;
  for (let i = 0; i < 75; i++) {
    await sleep(1000);
    ben = await sessionOf(t2);
    eve = await sessionOf(t5);
    if (ben.assigned_to !== before2 && ben.no_answer_sent && eve.status === 'closed') break;
  }
  ok('an unanswered chat is passed to the next operator', ben.assigned_to && ben.assigned_to !== before2, ben);
  poll = (await pub('GET', `/public/chat/${sales.key}/session?token=${t2}`)).data;
  ok('the waiting visitor hears from us', poll.messages.some((m: any) => m.body === 'Sorry for the wait, we are on it.'), poll.messages);
  ok('an idle chat is closed', eve.status === 'closed', eve);
  poll = (await pub('GET', `/public/chat/${sales.key}/session?token=${t5}`)).data;
  ok('with the goodbye', poll.messages.some((m: any) => m.body === 'Thanks for chatting!') && poll.status === 'closed', poll.messages);

  // ---- marks ----
  await owner.post(`/chat/sessions/${s2.id}/messages`, { body: 'Hello Ben!' }).catch(() => {});
  await admin.post(`/chat/sessions/${s2.id}/messages`, { body: 'Hello Ben, how can I help?' });
  ok('a rating is 1 to 5', (await pub('POST', `/public/chat/${sales.key}/rate`, { token: t2, rating: 9 })).status === 400);
  ok('the visitor rates the chat', (await pub('POST', `/public/chat/${sales.key}/rate`, { token: t2, rating: 4, comment: 'Quick and kind' })).status === 200);
  ok('only managers review', (await ola.post(`/chat/sessions/${s2.id}/review`, { score: 5 })).status === 403);
  r = await admin.post(`/chat/sessions/${s2.id}/review`, { score: 3, comment: 'Answer faster' });
  const seen = (await admin.get(`/chat/sessions/${s2.id}`)).data.session;
  ok('a supervisor reviews it; both marks show', r.status === 200 && seen.rating === 4 && seen.rating_comment === 'Quick and kind' && seen.review_score === 3, seen);

  // ---- figures and export ----
  ok('figures are for managers', (await ola.get('/chat/stats')).status === 403);
  r = await admin.get(`/chat/stats?channel_id=${sales.id}`);
  const tot = r.data.total;
  ok('figures: chats, answered, missed, rating, review', tot.chats === 3 && tot.answered >= 1 && tot.missed >= 1 && tot.rating === 4 && tot.review === 3 && r.data.days.length >= 1 && typeof tot.first_reply_secs === 'number', r.data);
  ok('per operator', r.data.operators.length >= 1);
  const csv = await admin.req('GET', `/chat/export.csv?channel_id=${sales.id}`, undefined, true);
  const text = await csv.text();
  ok('the CSV export has the chats and what was said', csv.status === 200 && /text\/csv/.test(csv.headers.get('content-type') ?? '') && text.includes(`Ben ${stamp}`) && text.includes('Hello Ben, how can I help?') && text.includes('Quick and kind'), text.slice(0, 400));
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
