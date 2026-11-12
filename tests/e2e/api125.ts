// Attachments in the office editor: a document attached to a task, a chat message or a post
// opens in Collabora like a drive file. The task's people, the chat's members and the post's
// readers edit it; watchers only read; a save updates the attachment in place (everywhere it
// is shown) and keeps the earlier version.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const B = 'http://localhost:8080/api';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const made: { tasks: string[]; chats: string[]; posts: string[] } = { tasks: [], chats: [], posts: [] };

const status = (await admin.get('/office/status')).data;
if (!status.editing || !status.editable?.includes('docx')) {
  console.log('skipped: no document server (COLLABORA_URL)');
  process.exit(0);
}
try {
  const person = async (first: string) => {
    const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `O${s}` })).data;
    const c = new Client();
    await c.post('/auth/login', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123' });
    return { u, c };
  };
  const [rae, wes, oz] = [await person('Rae'), await person('Wes'), await person('Oz')];
  const upload = async (c: Client, name: string, text: string) => {
    const fd = new FormData(); fd.append('file', new Blob([text], { type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' }), name);
    return (await c.req('POST', '/files', fd) as any).data.id as string;
  };
  const open = (c: Client, id: string) => c.post(`/files/${id}/office`, {});
  const wopi = (id: string, token: string, path = '', init: RequestInit = {}) => fetch(`${B}/wopi/files/${id}${path}?access_token=${encodeURIComponent(token)}`, init);

  // ---- a task's attachment ----
  const doc = await upload(rae.c, `plan-${s}.docx`, `version one ${s}`);
  const task = (await rae.c.post('/tasks', { title: `Plan ${s}`, responsible_id: rae.u.id, watchers: [wes.u.id], file_ids: [doc] })).data;
  made.tasks.push(task.id);
  if (!(await db.query('SELECT 1 FROM task_files WHERE task_id = $1 AND file_id = $2', [task.id, doc])).rowCount) {
    await db.query('INSERT INTO task_files (task_id, file_id, added_by) VALUES ($1, $2, $3)', [task.id, doc, rae.u.id]);
  }
  if (!(await db.query('SELECT 1 FROM task_members WHERE task_id = $1 AND user_id = $2', [task.id, wes.u.id])).rowCount) {
    await db.query("INSERT INTO task_members (task_id, user_id, role) VALUES ($1, $2, 'watcher')", [task.id, wes.u.id]);
  }
  let r = await open(rae.c, doc);
  ok('the task\'s attachment opens for editing', r.status === 200 && r.data.can_edit === true && r.data.action === 'edit' && /WOPISrc=/.test(r.data.url), r.data);
  const token = r.data.token;
  let info = await (await wopi(doc, token)).json();
  ok('the editor is told who and what', info.BaseFileName === `plan-${s}.docx` && info.UserCanWrite === true && info.Version === '1', info);
  ok('…and reads it', (await (await wopi(doc, token, '/contents')).text()) === `version one ${s}`);
  const saved = await wopi(doc, token, '/contents', { method: 'POST', body: `version two ${s}`, headers: { 'X-WOPI-Override': 'PUT' } });
  ok('a save', saved.status === 200 && saved.headers.get('X-WOPI-ItemVersion') === '2');
  const content = await rae.c.req('GET', `/files/${doc}/content`, undefined, true) as Response;
  ok('…updates the attachment in place', (await content.text()) === `version two ${s}`);
  ok('…keeping the earlier version', (await db.query('SELECT version FROM file_versions WHERE file_id = $1', [doc])).rows.map((x) => x.version).join() === '1');
  r = await open(wes.c, doc);
  ok('a watcher only reads', r.status === 200 && r.data.can_edit === false, r.data);
  ok('…and can\'t save', (await wopi(doc, r.data.token, '/contents', { method: 'POST', body: 'nope', headers: { 'X-WOPI-Override': 'PUT' } })).status === 403);
  ok('someone outside the task can\'t open it', (await open(oz.c, doc)).status === 404);

  // ---- a chat's attachment ----
  const chat = (await rae.c.post('/im/chats', { title: `Docs ${s}`, members: [wes.u.id] })).data;
  made.chats.push(chat.id);
  const sheet = await upload(rae.c, `budget-${s}.docx`, `budget ${s}`);
  await rae.c.post(`/im/chats/${chat.id}/messages`, { body: 'The budget', file_ids: [sheet] });
  r = await open(wes.c, sheet);
  ok('a chat member edits a file in the chat', r.status === 200 && r.data.can_edit === true, r.data);
  ok('…people outside the chat don\'t', (await open(oz.c, sheet)).status === 404);

  // ---- a post's attachment ----
  const note = await upload(rae.c, `memo-${s}.docx`, `memo ${s}`);
  const post = (await rae.c.post('/feed', { body: `Memo ${s}`, to_all: false, users: [wes.u.id], file_ids: [note] })).data;
  made.posts.push(post.id ?? post.post?.id);
  r = await open(wes.c, note);
  ok('the people a post is for edit its file', r.status === 200 && r.data.can_edit === true, r.data);

  // Drive files are untouched by this.
  ok('editing a drive file still works', (await admin.get('/office/status')).data.editing === true);
} finally {
  for (const t of made.tasks) await db.query('DELETE FROM tasks WHERE id = $1', [t]).catch(() => {});
  for (const c of made.chats) await db.query('DELETE FROM im_chats WHERE id = $1', [c]).catch(() => {});
  for (const p of made.posts.filter(Boolean)) await db.query('DELETE FROM feed_posts WHERE id = $1', [p]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
