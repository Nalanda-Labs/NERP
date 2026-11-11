// Mass assignment, endpoint by endpoint: fields a person may not set. Bob sends every create
// and change endpoint — on his own things — fields that would make them Ann's (owner,
// creator, author) or move them into Ann's private places (her workgroup, folder, chat,
// knowledge base space, list, board, dashboard, calendar), plus privilege flags. Where the
// endpoint accepts the request, nothing in the answer may carry those values, and Ann's
// places must not contain Bob's things afterwards.
import { ok, check, finish, counts, stamp } from './lib.ts';
import { world } from './world.ts';
import { endpoints, SELF_HARM, fillPath, params, reach, ok2xx } from './sweep.ts';

const { ann, bob, ids } = await world();
const up = async (name: string) => { const f = new FormData(); f.append('file', new Blob(['bob']), name); return (await bob.post('/files', f)).data?.id as string; };
const start = new Date(Date.now() + 5 * 86400e3);
const mine: Record<string, string | undefined> = {
  task: (await bob.post('/tasks', { title: `Bob task ${stamp}` })).data?.id,
  board: (await bob.post('/boards', { title: `Bob board ${stamp}` })).data?.id,
  event: (await bob.post('/calendar/events', { title: `Bob event ${stamp}`, timezone: 'UTC', start: start.toISOString(), end: new Date(+start + 3600e3).toISOString() })).data?.id,
  drive: (await bob.post('/drive/files', { file_id: await up(`bob-${stamp}.txt`), name: `bob-${stamp}.txt` })).data?.id,
  chat: (await bob.post('/im/chats', { title: `Bob chat ${stamp}`, members: [] })).data?.id,
  post: (await bob.post('/feed', { body: `Bob post ${stamp}` })).data?.id,
  group: (await bob.post('/projects', { name: `Bob group ${stamp}`, kind: 'group' })).data?.id,
};
const own = Object.entries(mine).filter(([, v]) => v) as [string, string][];
ok('Bob has his own things to work with', own.length >= 6, mine);
// Ann's private places and Ann herself, by the field names that would point at them.
const PLANT: Record<string, string> = {
  owner_id: ann.id, created_by: ann.id, author_id: ann.id, creator_id: ann.id, updated_by: ann.id, sender_id: ann.id, uploaded_by: ann.id, requested_by: ann.id, approved_by: ann.id,
  calendar_id: ann.id, project_id: ids.group, group_id: ids.group, parent_id: ids.driveFolder, folder_id: ids.driveFolder, chat_id: ids.chat, space_id: ids.kbSpace,
  list_id: ids.list, board_id: ids.board, dashboard_id: ids.dash, meeting_id: ids.meeting, ticket_id: ids.ticket, task_id: ids.task, post_id: ids.catPost,
};
const FLAGS = { is_admin: true, is_guest: false, permissions: ['*'], role: 'owner', roles: ['admin'], is_system: true, verified: true };
const planted = new Set(Object.values(PLANT).filter(Boolean));
const carries = (json: unknown) => {
  const hits: string[] = [];
  const walk = (v: unknown, key = '') => {
    if (Array.isArray(v)) v.forEach((x) => walk(x, key));
    else if (v && typeof v === 'object') for (const [k, x] of Object.entries(v)) walk(x, k);
    else if (typeof v === 'string' && planted.has(v) && key in PLANT && v === PLANT[key]) hits.push(`${key}=${v.slice(0, 8)}`);
  };
  walk(json);
  return hits;
};
let landed = 0, swept = 0;
for (const e of endpoints()) {
  if (!['POST', 'PUT', 'PATCH'].includes(e.method) || SELF_HARM.test(e.path) || /^\/api\/(public|users\/\{)/.test(e.path)) continue;
  const k = params(e.path);
  if (k > 1) continue;
  swept++;
  const targets: [string, string | undefined][] = k === 0 ? [['(new)', undefined]] : own;
  for (const [what, id] of targets) {
    const path = id ? fillPath(e.path, [id]) : e.path;
    const r = await reach(e.method, path, bob.token, { title: `Bob ${stamp}`, name: `Bob ${stamp}`, body: `Bob ${stamp}`, ...PLANT, ...FLAGS });
    if (ok2xx(r.status)) landed++;
    const hits = ok2xx(r.status) ? carries(r.json) : [];
    check(`${e.method} ${e.path} on Bob's ${what}: no field makes it Ann's or puts it in her places`, hits.length === 0, hits);
  }
}
ok(`(${swept} write endpoints tried; ${landed} accepted the request)`, swept > 300);

// ---- Ann's places hold nothing of Bob's ----
const annSees = async (p: string) => JSON.stringify((await ann.get(p)).data ?? '');
ok('her private workgroup has no task of Bob\'s', !(await annSees(`/tasks?project_id=${ids.group}`)).includes(`Bob ${stamp}`) && !(await annSees(`/tasks?project_id=${ids.group}`)).includes(`Bob task ${stamp}`));
ok('her folder has no file of Bob\'s', !(await annSees(`/drive?parent_id=${ids.driveFolder}`)).includes(`bob-${stamp}`));
ok('her chat has no message of Bob\'s', !(await annSees(`/im/chats/${ids.chat}/messages`)).includes(`Bob ${stamp}`));
ok('her calendar has no event of Bob\'s', !(await annSees(`/calendar/events?from=${new Date(Date.now() - 86400e3).toISOString()}&to=${new Date(Date.now() + 30 * 86400e3).toISOString()}`)).includes(`Bob`));
ok('her knowledge base space has no page of Bob\'s', !(await annSees(`/kb/spaces/${ids.kbSpace}`)).includes(`Bob ${stamp}`));
const me = (await bob.get('/auth/me')).data;
ok('and Bob gained no permission', !(me.permissions ?? []).some((p: string) => p === '*' || p.includes('manage')) && !me.is_guest === true, me.permissions);
console.log(`checks: ${JSON.stringify(counts())}`);
await finish();
