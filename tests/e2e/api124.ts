// Cross-functional teams: people from different departments under a lead, with roles; the lead
// manages the members; the team chat follows the team; profiles list a person's teams.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
let teamId = '';

try {
  const dept = async (name: string) => (await admin.post('/departments', { name: `${name} ${s}` })).data;
  const [eng, sales] = [await dept('Engineering'), await dept('Sales')];
  const person = async (first: string, d: any) => {
    const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `T${s}` })).data;
    await admin.post(`/departments/${d.id}/members`, { user_id: u.id });
    const c = new Client();
    await c.post('/auth/login', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123' });
    return { u, c };
  };
  const lea = await person('Lea', eng), max = await person('Max', sales), kim = await person('Kim', eng), ned = await person('Ned', sales), out = await person('Oz', sales);

  ok('teams are made by those who manage the structure', (await lea.c.post('/teams', { name: 'Mine' })).status === 403);
  ok('a colour is #rrggbb', (await admin.post('/teams', { name: 'X', color: 'red' })).status === 400);
  ok('members are employees', (await admin.post('/teams', { name: 'X', members: [{ user_id: '00000000-0000-0000-0000-000000000001' }] })).status === 400);
  let r = await admin.post('/teams', { name: `Launch ${s}`, description: 'Ship the new plan', color: '#2563eb', lead_id: lea.u.id, members: [{ user_id: max.u.id, role: 'Design' }, { user_id: kim.u.id, role: 'QA' }] });
  teamId = r.data.id;
  const m = (x: any, id: string) => x.members.find((p: any) => p.id === id);
  ok('a team across departments, the lead first', r.status === 200 && r.data.members.length === 3 && r.data.members[0].id === lea.u.id && r.data.members[0].lead === true, r.data.members);
  ok('…with roles and each person\'s department', m(r.data, max.u.id).role === 'Design' && m(r.data, max.u.id).departments === `Sales ${s}` && m(r.data, kim.u.id).departments === `Engineering ${s}`, r.data.members);
  const list = (await lea.c.get('/teams')).data.items.find((t: any) => t.id === teamId);
  ok('listed for everyone, "yours" for its people', list && list.members === 3 && list.mine === true && list.lead.name === `Lea T${s}` && (await out.c.get('/teams')).data.items.find((t: any) => t.id === teamId).mine === false, list);

  // ---- the lead manages the members ----
  ok('members don\'t change the team', (await max.c.put(`/teams/${teamId}/members`, { members: [] })).status === 403);
  r = await lea.c.put(`/teams/${teamId}/members`, { members: [{ user_id: max.u.id, role: 'Design' }, { user_id: ned.u.id, role: 'Sales' }] });
  ok('the lead does', r.status === 200 && r.data.members.map((p: any) => p.id).sort().join() === [lea.u.id, max.u.id, ned.u.id].sort().join(), r.data.members);

  // ---- the chat ----
  ok('outsiders don\'t get into the team chat', (await out.c.post(`/teams/${teamId}/chat`, {})).status === 403);
  r = await max.c.post(`/teams/${teamId}/chat`, {});
  const chat = r.data.chat_id;
  const inChat = async () => (await db.query('SELECT user_id FROM im_members WHERE chat_id = $1', [chat])).rows.map((x) => x.user_id).sort().join();
  ok('the team chat has the team', r.status === 200 && (await inChat()) === [lea.u.id, max.u.id, ned.u.id].sort().join());
  await lea.c.put(`/teams/${teamId}/members`, { members: [{ user_id: max.u.id }, { user_id: ned.u.id }, { user_id: kim.u.id }] });
  ok('…and follows it', (await inChat()) === [lea.u.id, max.u.id, ned.u.id, kim.u.id].sort().join());
  await admin.patch(`/teams/${teamId}`, { name: `Launch v2 ${s}`, lead_id: lea.u.id, color: '#2563eb' });
  ok('…and its name', (await db.query('SELECT title FROM im_chats WHERE id = $1', [chat])).rows[0].title === `Launch v2 ${s}`);
  const detail = (await max.c.get(`/im/chats/${chat}`)).data;
  ok('the chat follows the team, not by hand', !JSON.stringify(detail).includes('"add_people":true'), detail);

  // ---- profiles ----
  r = await admin.get(`/users/${max.u.id}/teams`);
  ok('a profile lists the person\'s teams', r.data.items.length === 1 && r.data.items[0].name === `Launch v2 ${s}`);
  ok('…and the lead\'s says so', (await admin.get(`/users/${lea.u.id}/teams`)).data.items[0].lead === true);

  await admin.del(`/teams/${teamId}`);
  ok('a removed team takes its chat', (await db.query('SELECT 1 FROM im_chats WHERE id = $1', [chat])).rowCount === 0 && (await admin.get(`/teams/${teamId}`)).status === 404);
  teamId = '';
} finally {
  if (teamId) await db.query('DELETE FROM teams WHERE id = $1', [teamId]).catch(() => {});
  await db.query('DELETE FROM departments WHERE name LIKE $1', [`% ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
