// Working with an email: a task from it (linked to its record), an event from it, and
// discussing it in a chat or the feed; only for those who can read it.
import pg from 'pg';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const db = new pg.Client({ connectionString: dbUrl }); await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Mail${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob] = await Promise.all(['Ann', 'Bob'].map(user));

try {
  const deal = (await admin.post('/crm/deal/items', { title: `Mail deal ${stamp}`, assigned_to: admin.id })).data;
  const me = (await admin.get('/auth/me')).data;
  const { rows } = await db.query(
    `INSERT INTO crm_emails (direction, message_id, thread_id, from_addr, from_name, subject, body_text, snippet, created_by)
     VALUES ('in', $1, gen_random_uuid(), 'client@example.com', 'Clara Client', $2, $3, 'Please send', $4) RETURNING id`,
    [`<${stamp}@example.com>`, `Quote needed ${stamp}`, `Please send a quote for 40 chairs by Friday.\nThanks, Clara`, me.id ?? me.user?.id],
  );
  const email = rows[0].id;
  await db.query('INSERT INTO crm_email_links (email_id, item_id) VALUES ($1, $2)', [email, deal.id]);

  let r = await admin.post(`/crm/emails/${email}/task`, {});
  ok('a task from the email', r.status === 200 && !!r.data.task_id, r.data);
  const t = (await admin.get(`/tasks/${r.data.task_id}`)).data;
  ok('its subject is the title, its text the description', t.title === `Quote needed ${stamp}` && t.description.includes('40 chairs') && t.description.includes('Clara Client <client@example.com>'), t);
  ok('linked to the email\'s record', t.crm.some((c: any) => c.id === deal.id), t.crm);
  const lone = (await db.query(
    `INSERT INTO crm_emails (direction, message_id, thread_id, from_addr, subject, created_by) VALUES ('in', $1, gen_random_uuid(), 'x@example.com', $2, $3) RETURNING id`,
    [`<lone.${stamp}@example.com>`, `Private ${stamp}`, me.id ?? me.user?.id],
  )).rows[0].id;
  ok('not for someone who can\'t read the email', (await ann.post(`/crm/emails/${lone}/task`, {})).status === 404 && (await ann.post(`/crm/emails/${lone}/share`, { feed: true })).status === 404);
  await db.query('DELETE FROM crm_emails WHERE id = $1', [lone]);

  const start = new Date(Date.now() + 86400e3);
  r = await admin.post(`/crm/emails/${email}/event`, { start: start.toISOString(), end: new Date(+start + 1800e3).toISOString(), timezone: 'UTC', attendees: [bob.id] });
  ok('an event from the email, on the record', r.status === 200 && r.data.title === `Quote needed ${stamp}` && r.data.item?.id === deal.id && r.data.people.some((p: any) => p.id === bob.id), r.data);

  const chat = (await admin.post('/im/chats', { title: `Sales ${stamp}`, members: [bob.id] })).data.id;
  r = await admin.post(`/crm/emails/${email}/share`, { chat_id: chat, comment: 'Can you take this?' });
  const msgs = (await bob.get(`/im/chats/${chat}/messages`)).data.items.map((m: any) => m.body);
  ok('discussed in a chat: who wrote, the subject, the text and the record', r.status === 200 && msgs.some((b: string) => b.startsWith('Can you take this?') && b.includes(`“Quote needed ${stamp}”`) && b.includes('40 chairs') && b.includes(`/crm/deal/${deal.id}`)), msgs);
  ok('not in a chat I\'m not in', (await admin.post(`/crm/emails/${email}/share`, { chat_id: (await ann.post('/im/chats', { title: `Other ${stamp}`, members: [bob.id] })).data.id })).status === 404);
  r = await admin.post(`/crm/emails/${email}/share`, { feed: true, users: [bob.id] });
  ok('discussed in the feed, with chosen people', r.status === 200 && !!r.data.post_id, r.data);
  ok('bob sees the post', (await bob.get(`/feed/${r.data.post_id}`)).status === 200 && (await ann.get(`/feed/${r.data.post_id}`)).status === 404);
  ok('share somewhere', (await admin.post(`/crm/emails/${email}/share`, {})).status === 400);
  await db.query('DELETE FROM crm_emails WHERE id = $1', [email]);
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
