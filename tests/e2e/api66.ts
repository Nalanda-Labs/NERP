// People: reactions on feed posts; appreciation badges (given in the feed, told to the
// colleagues, counted on profiles); online and last-seen status; the employee list as CSV.
import pg from 'pg';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `People${stamp}`, position: '=SUM(1,2)' })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ana, ben, cal] = await Promise.all(['Ana', 'Ben', 'Cal'].map(user));

try {
  // ---- reactions ----
  let r = await ana.post('/feed', { body: `Launch day ${stamp}` });
  const post = r.data;
  r = await ben.post(`/feed/${post.id}/like`, { reaction: 'clap' });
  ok('a reaction', r.data.reactions.clap === 1 && r.data.my_reaction === 'clap' && r.data.likes === 1, r.data);
  r = await cal.post(`/feed/${post.id}/like`);
  ok('a plain like still works', r.data.reactions.like === 1 && r.data.likes === 2, r.data);
  r = await ben.post(`/feed/${post.id}/like`, { reaction: 'love' });
  ok('reacting again changes it', r.data.reactions.love === 1 && !r.data.reactions.clap && r.data.likes === 2, r.data);
  ok('only known reactions', (await ben.post(`/feed/${post.id}/like`, { reaction: 'poop' })).status === 400);
  r = await ben.del(`/feed/${post.id}/like`);
  ok('taken back', r.data.my_reaction === null && r.data.likes === 1, r.data);

  // ---- appreciation badges ----
  ok('a badge must be one of the list', (await ana.post('/feed', { body: 'x', badge: 'genius', badge_user_ids: [ben.id] })).status === 400);
  ok('and given to someone', (await ana.post('/feed', { body: 'x', badge: 'thanks', badge_user_ids: [ana.id] })).status === 400);
  r = await ana.post('/feed', { body: `For fixing the build ${stamp}`, badge: 'problem_solver', badge_user_ids: [ben.id, cal.id] });
  ok('a badge post', r.data.badge === 'problem_solver' && r.data.badge_people.length === 2, r.data);
  await ana.post('/feed', { body: `Thanks for the help ${stamp}`, badge: 'thanks', badge_user_ids: [ben.id] });
  ok('they are told', JSON.stringify((await ben.get('/notifications')).data).includes('gave you “Problem solver”'));
  r = await cal.get(`/users/${ben.id}/badges`);
  ok('a profile counts them', r.data.counts.length === 2 && r.data.counts.every((c: any) => c.count === 1) && r.data.recent[0].from === `Ana People${stamp}` && r.data.badges.length === 6, r.data);

  // ---- online, last seen ----
  r = await admin.get(`/users/${ben.id}`);
  ok('someone active just now is online', r.data.online === true && !!r.data.last_seen_at, r.data);
  await db.query(`UPDATE users SET last_seen_at = now() - interval '2 hours' WHERE id = $1`, [cal.id]);
  r = await admin.get(`/users/${cal.id}`);
  ok('someone away shows when they were last seen', r.data.online === false && !!r.data.last_seen_at, r.data);

  // ---- export ----
  const res = await admin.req('GET', '/users/export.csv', undefined, true);
  const csv = await res.text();
  ok('the employee list as CSV', res.status === 200 && /text\/csv/.test(res.headers.get('content-type') ?? '') && csv.includes(`ana.${stamp}@acme.test`), csv.slice(0, 200));
  ok('cells that look like formulas are made plain text', csv.includes("'=SUM(1,2)") || csv.includes('"\'=SUM(1,2)"'), csv.split('\n').find((l) => l.includes(`ana.${stamp}`)));
  // It follows "View employees" (which employees have by default).
  ok('anyone who may see the directory may export it', (await ana.req('GET', '/users/export.csv', undefined, true)).status === 200);
  ok('signed-out callers may not', (await new Client().req('GET', '/users/export.csv', undefined, true)).status === 401);
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
