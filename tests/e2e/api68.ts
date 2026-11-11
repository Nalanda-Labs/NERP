// Security and administration: NERP only from some networks (administrators may be left
// out of it; a list that shuts out whoever saves it is refused), links for anyone switched
// off company-wide, a company limit on keeping chat messages, links to join the company
// (with and without approval), the "Invite employees" permission, feed moderators, and the
// storage overview with its clean-up.
import pg from 'pg';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

const MP = 'http://localhost:8026/api/v1';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Sec${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  c.email = email;
  return c;
};
const [eve, hal, mod] = await Promise.all(['Eve', 'Hal', 'Mod'].map(user));
const empRole = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
const role = async (code: string, permissions: string[], who: Client) => {
  const r = (await admin.post('/roles', { code: `${code}_${stamp}`, name: `${code} ${stamp}`, permissions })).data;
  await admin.put(`/users/${who.id}/roles`, { role_ids: [empRole.id, r.id] });
  return r;
};
const mailTo = async (addr: string, subjectHas: string) => {
  for (let i = 0; i < 40; i++) {
    const r = await (await fetch(`${MP}/search?query=${encodeURIComponent('to:' + addr)}&limit=5`)).json();
    const m = r.messages?.find((x: any) => x.Subject.includes(subjectHas));
    if (m) return m;
    await sleep(250);
  }
  return null;
};
const reset = () => admin.put('/settings', { ip_allow: [], ip_allow_admins_exempt: true, public_links: true, chat_retention_days: null });

try {
  // ---- IP address ranges ----
  let r = await admin.put('/settings', { ip_allow: ['10.0.0.0/33'] });
  ok('ranges are checked', r.status === 400 && /10\.0\.0\.0\/33/.test(r.data.error.message), r.data);
  r = await admin.put('/settings', { ip_allow: ['10.9.9.0/24'], ip_allow_admins_exempt: false });
  ok('a list that shuts out the person saving it is refused', r.status === 400 && /lock you out/.test(r.data.error.message), r.data);
  r = await admin.put('/settings', { ip_allow: ['10.9.9.0/24', '203.0.113.0/24'], ip_allow_admins_exempt: true });
  ok('administrators left out of the rule may save it', r.status === 200 && r.data.ip_allow.length === 2, r.data);
  r = await eve.get('/tasks');
  ok('others are refused at once', r.status === 403 && r.data.error.code === 'blocked' && /this network/.test(r.data.error.message), r.data);
  r = await new Client().post('/auth/login', { email: eve.email, password: 'password123' });
  ok('and cannot sign in', r.status === 403 && r.data.error.code === 'blocked', r.data);
  ok('administrators still work', (await admin.get('/tasks')).status === 200);
  ok('administrators see the list', 'ip_allow' in (await admin.get('/settings')).data);
  await admin.put('/settings', { ip_allow: ['10.9.9.0/24', '127.0.0.0/8', '::1'] });
  ok('from an allowed network they work again', (await eve.get('/tasks')).status === 200);
  ok('they don’t see the list', !('ip_allow' in (await eve.get('/settings')).data));
  await reset();

  // ---- links for anyone ----
  const board = (await eve.post('/boards', { title: `Plan ${stamp}` })).data;
  const link = (await eve.post(`/boards/${board.id}/links`, { level: 'view' })).data;
  const token = link.url.split('/b/')[1];
  ok('a public link works', (await new Client().get(`/public/boards/${token}`)).status === 200);
  r = await admin.put('/settings', { public_links: false });
  ok('switched off', r.data.public_links === false);
  r = await eve.post(`/boards/${board.id}/links`, { level: 'view' });
  ok('no new links', r.status === 403 && /switched off/.test(r.data.error.message), r.data);
  ok('existing links stop working', (await new Client().get(`/public/boards/${token}`)).status === 404);
  const dash = (await admin.post('/bi/dashboards', { title: `Sec ${stamp}` })).data;
  ok('dashboards too', (await admin.post(`/bi/dashboards/${dash.id}/links`, {})).status === 403);
  await admin.put('/settings', { public_links: true });
  ok('back on, the old link works again', (await new Client().get(`/public/boards/${token}`)).status === 200);
  await admin.del(`/bi/dashboards/${dash.id}`);

  // ---- chat retention ----
  const chat = (await eve.post('/im/chats', { title: `Old talk ${stamp}`, members: [hal.id] })).data;
  await eve.post(`/im/chats/${chat.id}/messages`, { body: 'ancient' });
  await eve.post(`/im/chats/${chat.id}/messages`, { body: 'recent' });
  await db.query(`UPDATE im_messages SET created_at = now() - interval '3 days' WHERE chat_id = $1 AND body = 'ancient'`, [chat.id]);
  ok('the limit is checked', (await admin.put('/settings', { chat_retention_days: 0 })).status === 400);
  await admin.put('/settings', { chat_retention_days: 2 });
  await db.query(`INSERT INTO jobs (kind, payload) VALUES ('im.maintenance', '{}')`);
  let left: string[] = [];
  for (let i = 0; i < 30; i++) {
    left = (await db.query('SELECT body FROM im_messages WHERE chat_id = $1 AND user_id IS NOT NULL', [chat.id])).rows.map((x) => x.body);
    if (!left.includes('ancient')) break;
    await sleep(500);
  }
  ok('older messages are deleted company-wide', !left.includes('ancient') && left.includes('recent'), left);
  await reset();

  // ---- invite permission ----
  ok('inviting needs a permission', (await hal.post('/users', { email: `x.${stamp}@acme.test`, first_name: 'X', last_name: 'Y', send_invite: true })).status === 403);
  await role('inviter', ['users.invite'], hal);
  r = await hal.post('/users', { email: `newbie.${stamp}@acme.test`, first_name: 'Newbie', last_name: `Sec${stamp}`, send_invite: true });
  ok('with it, people are invited by email', r.status === 200 && r.data.invite_pending, r.data);
  ok('but not given a password or roles', (await hal.post('/users', { email: `y.${stamp}@acme.test`, first_name: 'Y', last_name: 'Z', password: 'password123' })).status === 403
    && (await hal.post('/users', { email: `z.${stamp}@acme.test`, first_name: 'Z', last_name: 'Z', send_invite: true, role_ids: [empRole.id] })).status === 403);
  ok('the invitation arrives', !!(await mailTo(`newbie.${stamp}@acme.test`, 'invited you')));

  // ---- links to join ----
  ok('making join links needs it too', (await eve.get('/join-links')).status === 403);
  const team = (await admin.post('/departments', { name: `Joiners ${stamp}` })).data;
  r = await hal.post('/join-links', { note: `Spring ${stamp}`, department_id: team.id, email_domains: ['@Acme.test', 'nope'], needs_approval: true });
  ok('domains are checked', r.status === 400, r.data);
  r = await hal.post('/join-links', { note: `Spring ${stamp}`, department_id: team.id, email_domains: ['@Acme.test'], needs_approval: true, max_uses: 2 });
  const jl = r.data;
  const jt = jl.url.split('/join/')[1];
  ok('a join link', r.status === 200 && jl.email_domains[0] === 'acme.test' && jl.usable && jl.department === `Joiners ${stamp}`, r.data);
  const anon = new Client();
  r = await anon.get(`/public/join/${jt}`);
  ok('anyone sees what it is for', r.data.company_name === 'Acme Ltd' && r.data.needs_approval === true, r.data);
  r = await anon.post(`/public/join/${jt}`, { email: `ann.${stamp}@gmail.com`, first_name: 'Ann', last_name: `Sec${stamp}` });
  ok('only work emails', r.status === 400 && /acme\.test/.test(r.data.error.message), r.data);
  r = await anon.post(`/public/join/${jt}`, { email: `ann.${stamp}@acme.test`, first_name: 'Ann', last_name: `Sec${stamp}` });
  ok('asked to join', r.status === 200 && r.data.needs_approval === true, r.data);
  r = await anon.post(`/public/join/${jt}`, { email: eve.email, first_name: 'Eve', last_name: 'Again' });
  ok('someone who has an account hears the same', r.status === 200 && r.data.ok, r.data);
  r = await hal.get('/join-requests');
  const ann = r.data.items.find((x: any) => x.email === `ann.${stamp}@acme.test`);
  ok('the request waits', !!ann && ann.department === `Joiners ${stamp}` && ann.link_note === `Spring ${stamp}`, r.data);
  ok('it is not in the directory yet', !(await admin.get(`/users?q=${encodeURIComponent(`Ann Sec${stamp}`)}`)).data.items.some((u: any) => u.id === ann.id));
  ok('whoever made the link is told', JSON.stringify((await hal.get('/notifications')).data).includes('asks to join'));
  r = await anon.post(`/public/join/${jt}`, { email: `bob.${stamp}@acme.test`, first_name: 'Bob', last_name: `Sec${stamp}` });
  ok('the second (last) use', r.status === 200);
  ok('then the link is used up', (await anon.get(`/public/join/${jt}`)).status === 404 && (await anon.post(`/public/join/${jt}`, { email: `cy.${stamp}@acme.test`, first_name: 'Cy', last_name: 'X' })).status === 404);
  r = await hal.post(`/join-requests/${ann.id}/approve`);
  ok('approved', r.status === 200 && !r.data.items.some((x: any) => x.id === ann.id), r.data);
  ok('and invited by email', !!(await mailTo(`ann.${stamp}@acme.test`, 'invited you')));
  r = await admin.get(`/users/${ann.id}`);
  ok('now an employee, in the department', r.data.is_active && r.data.invite_pending && r.data.departments?.some((d: any) => d.id === team.id), r.data);
  const bob = (await hal.get('/join-requests')).data.items.find((x: any) => x.email === `bob.${stamp}@acme.test`);
  r = await hal.post(`/join-requests/${bob.id}/reject`);
  ok('a turned-down request is gone', r.status === 200 && (await db.query('SELECT 1 FROM users WHERE id = $1', [bob.id])).rowCount === 0);
  r = await admin.post('/join-links', { note: `Open ${stamp}`, needs_approval: false });
  const open = r.data.url.split('/join/')[1];
  r = await anon.post(`/public/join/${open}`, { email: `dee.${stamp}@elsewhere.test`, first_name: 'Dee', last_name: `Sec${stamp}` });
  ok('without approval: invited at once', r.status === 200 && r.data.needs_approval === false && !!(await mailTo(`dee.${stamp}@elsewhere.test`, 'invited you')), r.data);
  await admin.patch(`/join-links/${(await admin.get('/join-links')).data.items.find((l: any) => l.note === `Open ${stamp}`).id}`, { disabled: true });
  ok('a link turned off stops working', (await anon.get(`/public/join/${open}`)).status === 404);

  // ---- feed moderation ----
  r = await eve.post('/feed', { body: `Just for Hal ${stamp}`, to_all: false, users: [hal.id] });
  const priv = r.data;
  await hal.post(`/feed/${priv.id}/comments`, { body: 'thanks!' });
  ok('others don’t see a post for a few people', (await mod.get(`/feed/${priv.id}`)).status === 404);
  await role('moderator', ['feed.moderate'], mod);
  ok('moderators do', (await mod.get(`/feed/${priv.id}`)).status === 200);
  r = await mod.get(`/feed?all=true&q=${encodeURIComponent(`Just for Hal ${stamp}`)}`);
  ok('and find it with “every post”', r.data.items.some((p: any) => p.id === priv.id), r.data);
  ok('not without it', !(await mod.get(`/feed?q=${encodeURIComponent(`Just for Hal ${stamp}`)}`)).data.items.some((p: any) => p.id === priv.id));
  r = await mod.patch(`/feed/${priv.id}`, { body: `Just for Hal (edited) ${stamp}` });
  ok('moderators edit others’ posts', r.status === 200 && r.data.body.includes('(edited)'), r.data);
  const comment = (await hal.get(`/feed/${priv.id}/comments`)).data.items[0];
  ok('others may not edit a comment', (await eve.patch(`/feed/comments/${comment.id}`, { body: 'hacked' })).status === 403);
  r = await mod.patch(`/feed/comments/${comment.id}`, { body: 'thanks! (tidied)' });
  ok('moderators may', r.status === 200 && r.data.items[0].body === 'thanks! (tidied)', r.data);
  r = await hal.patch(`/feed/comments/${comment.id}`, { body: 'thanks again' });
  ok('and so may its author', r.status === 200, r.data);

  // ---- menu and background ----
  r = await eve.put('/me/ui', { menu: { hidden: ['/boards'], pinned: ['/crm/deal'] }, background: 'sky' });
  ok('one’s own menu and background', r.data.menu.hidden[0] === '/boards' && r.data.background === 'sky' && r.data.own_menu, r.data);
  ok('backgrounds are checked', (await eve.put('/me/ui', { background: 'neon' })).status === 400);
  ok('menu items are app paths', (await eve.put('/me/ui', { menu: { hidden: ['https://evil.test'] } })).status === 400);
  ok('only administrators set the company’s', (await eve.put('/settings/ui-default', { background: 'mint' })).status === 403);
  await admin.put('/settings/ui-default', { menu: { hidden: ['/lists'], pinned: [] }, background: 'mint' });
  r = await hal.get('/me/ui');
  ok('people who chose nothing get the company’s', r.data.background === 'mint' && r.data.menu.hidden[0] === '/lists' && !r.data.own_menu, r.data);
  ok('those who chose keep theirs', (await eve.get('/me/ui')).data.background === 'sky');
  await admin.put('/settings/ui-default', { menu: { hidden: ['/lists'], pinned: [] }, background: 'mint', apply_to_all: true });
  ok('applied to everyone, it replaces their choice', (await eve.get('/me/ui')).data.background === 'mint');
  await eve.put('/me/ui', { background: 'dusk' });
  r = await eve.put('/me/ui', { menu: null, background: null });
  ok('back to the company’s', r.data.background === 'mint' && !r.data.own_background, r.data);
  await admin.put('/settings/ui-default', { menu: { hidden: [], pinned: [] }, background: 'plain' });

  // ---- storage ----
  ok('storage is for file administrators', (await eve.get('/admin/storage')).status === 403);
  const up = async (text: string) => { const fd = new FormData(); fd.append('file', new Blob([text], { type: 'text/plain' }), `notes-${stamp}.txt`); return (await admin.req('POST', '/files', fd)).data.id; };
  let node: any;
  for (const t of ['one', 'two two', 'three three three']) node = (await admin.post('/drive/files', { file_id: await up(t), name: `notes-${stamp}.txt`, space: 'personal' })).data;
  r = await admin.get('/admin/storage');
  ok('an overview', r.status === 200 && r.data.files > 0 && r.data.bytes > 0 && r.data.areas.some((a: any) => a.key === 'drive_versions') && r.data.largest.length > 0 && r.data.disk?.free > 0, r.data);
  const before = r.data.cleanup.old_versions.count;
  ok('clean-ups are checked', (await admin.post('/admin/storage/cleanup', { what: 'everything' })).status === 400);
  r = await admin.post('/admin/storage/cleanup', { what: 'old_versions', keep: 1, older_than_days: 0 });
  ok('old versions beyond the last one deleted', r.status === 200 && r.data.removed >= 1, r.data);
  const versions = (await db.query('SELECT version FROM drive_versions WHERE node_id = $1 ORDER BY version', [node.id])).rows.map((x) => x.version);
  ok('the current one and the one before stay', JSON.stringify(versions) === '[2,3]', versions);
  ok('the overview follows', (await admin.get('/admin/storage')).data.cleanup.old_versions.count < before);
  await admin.del(`/drive/nodes/${node.id}`);
  r = await admin.post('/admin/storage/cleanup', { what: 'drive_trash' });
  ok('the trash emptied', r.status === 200 && (await db.query('SELECT 1 FROM drive_nodes WHERE id = $1', [node.id])).rowCount === 0, r.data);
} finally {
  await reset();
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
