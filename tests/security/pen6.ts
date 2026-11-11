// Permissions, part 2: the newer private things (a board, a private workgroup and its posts,
// a dashboard, app passwords and sessions, drive links, an absence request, a post for one
// colleague) against every route with one id AND every route with two ids, tried by a
// colleague with every method. Then a guest from outside the company tries every route of
// the API: only the guest's own areas may answer, and nothing about the company may leak.
import { routes, fill, type Method } from './routes.ts';
import { admin, person, raw, ok, finish, stamp, BASE } from './lib.ts';
import { Client } from '../e2e/lib.ts';

const ann = await person('Ann', ['bi.manage']);
const bob = await person('Bob');
const cat = await person('Cat');

// ---- Ann's newer private things ----
const upload = async (c: Client, name: string, text: string) => { const f = new FormData(); f.append('file', new Blob([text]), name); return (await c.post('/files', f)).data.id; };
const board = (await ann.post('/boards', { title: `Ann board ${stamp}` })).data;
const group = (await ann.post('/projects', { name: `Ann vault ${stamp}`, kind: 'group', visibility: 'private', member_ids: [cat.id] })).data;
const groupPost = (await ann.post('/feed', { body: `Vault plan ${stamp}`, project_id: group.id })).data;
const catPost = (await ann.post('/feed', { body: `Only for Cat ${stamp}`, to_all: false, users: [cat.id] })).data;
const dash = (await ann.post('/bi/dashboards', { title: `Ann dash ${stamp}` })).data;
const appPw = (await ann.post('/auth/app-passwords', { name: `Ann phone ${stamp}` })).data;
const session = (await ann.get('/auth/sessions')).data;
const sessionId = (session.items ?? session)?.[0]?.id;
const node = (await ann.post('/drive/files', { file_id: await upload(ann, 'linked.txt', 'linked secret'), name: `linked-${stamp}.txt` })).data;
const link = (await ann.post(`/drive/nodes/${node.id}/links`, {})).data;
const linkId = (link.links ?? []).at(-1)?.id;
const absence = (await ann.post('/hr/absences', { kind: 'vacation', starts_on: '2027-04-05', ends_on: '2027-04-06' })).data;
const groupTask = (await ann.post('/tasks', { title: `Vault task ${stamp}`, project_id: group.id })).data;
const ids: Record<string, string | undefined> = {
  board: board?.id, group: group?.id, groupPost: groupPost?.id, catPost: catPost?.id, dash: dash?.id, appPassword: appPw?.id,
  session: sessionId, driveNode: node?.id, driveLink: linkId, absence: absence?.id, groupTask: groupTask?.id,
};
ok('Ann has newer private things to protect', Object.values(ids).every(Boolean), ids);

// ---- Bob tries every route with one or two of Ann's ids ----
// Routes Bob may legitimately call with someone's id: profiles, joining an open group (this
// one is private), and per-person filters that answer ok for any id.
const SKIP = /^\/api\/(auth\/(login|refresh|logout|forgot|reset|accept|invite|providers|sso|identities)|realtime|public|setup)\b|^\/api\/users\/\{[^}]+\}(\/badges)?$|^\/api\/crm\/filters\/\{id\}$/;
const bodyFor = (m: Method) => (m === 'GET' || m === 'DELETE' ? undefined : { title: 'pwned by bob', name: 'pwned by bob', body: 'pwned by bob', members: [bob.id], member_ids: [bob.id], user_id: bob.id, level: 'edit', role: 'owner', status: 'approved' });
const leaks: string[] = [];
const values = Object.entries(ids).filter(([, v]) => v) as [string, string][];
for (const route of routes()) {
  const params = route.path.match(/\{[^}]+\}/g) ?? [];
  if (!params.length || params.length > 2 || SKIP.test(route.path)) continue;
  const combos: [string, string[]][] = params.length === 1
    ? values.map(([k, v]) => [k, [v]])
    : values.flatMap(([k1, v1]) => values.map(([k2, v2]) => [`${k1}+${k2}`, [v1, v2]] as [string, string[]]));
  for (const [what, vs] of combos) {
    let path = route.path.replace('/api', '');
    params.forEach((p, i) => (path = path.replace(p, vs[i])));
    for (const m of route.methods) {
      const res = (await bob.req(m, path, bodyFor(m))) as any;
      if (res.status >= 200 && res.status < 300) leaks.push(`${m} ${route.path} with Ann's ${what} → ${res.status} ${JSON.stringify(res.data).slice(0, 120)}`);
    }
  }
}
ok("Bob reaches none of Ann's newer private things through any route (one or two ids)", leaks.length === 0, leaks.slice(0, 25));

// ---- what Ann left is as she left it ----
ok('her board is untouched', (await ann.get(`/boards/${ids.board}`)).data?.title === `Ann board ${stamp}`);
ok('her private group has no Bob', !(await ann.get(`/projects/${ids.group}`)).data?.members?.some?.((m: any) => (m.id ?? m.user_id) === bob.id));
ok('her sessions are still there', (await ann.get('/auth/me')).status === 200);
ok('her app password still exists', ((await ann.get('/auth/app-passwords')).data.items ?? []).some((p: any) => p.id === ids.appPassword));
ok('her drive link still works', (await raw('GET', `/public/drive/${link.url?.split('/d/')[1]}`)).status === 200);
ok('her absence request is unchanged', JSON.stringify((await ann.get('/hr/absences/mine')).data).includes(ids.absence!));

// ---- lists and searches don't leak them either ----
const lookups: [string, string][] = [
  ['/boards', `Ann board ${stamp}`], ['/projects', `Ann vault ${stamp}`], [`/projects?q=${stamp}`, `Ann vault ${stamp}`], ['/feed', `Vault plan ${stamp}`],
  ['/feed', `Only for Cat ${stamp}`], [`/tasks?q=${stamp}`, `Vault task ${stamp}`], [`/search?q=${stamp}`, 'Vault'], [`/search?q=${stamp}`, 'Ann board'],
  ['/bi/dashboards', `Ann dash ${stamp}`], [`/drive?space=shared&owner=${ann.id}`, `linked-${stamp}`],
];
const seen: string[] = [];
for (const [path, text] of lookups) {
  const r = (await bob.get(path)) as any;
  if (JSON.stringify(r.data ?? '').includes(text)) seen.push(`${path} shows “${text}”`);
}
ok('no list or search shows them to Bob', seen.length === 0, seen);

// ---- a guest tries everything ----
const chat = (await ann.post('/im/chats', { title: `Partners ${stamp}`, members: [bob.id] })).data;
const company = (await ann.post('/im/chats', { title: `Staff only ${stamp}`, members: [bob.id] })).data;
await ann.post(`/im/chats/${company.id}/messages`, { body: `staff secret ${stamp}` });
const gEmail = `guest.${stamp}@outside.test`;
const gid = (await ann.post(`/im/chats/${chat.id}/guests`, { email: gEmail, first_name: 'Gus', last_name: `Out${stamp}` })).data.user_id;
// The guest sets a password from the invitation email (Mailpit), as a real guest would.
let invite: string | undefined;
for (let i = 0; i < 40 && !invite; i++) {
  const m = (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent('to:' + gEmail)}&limit=5`)).json()).messages?.[0];
  if (m) invite = (await (await fetch(`http://localhost:8026/api/v1/message/${m.ID}`)).json()).Text.match(/accept-invite\?token=([\w-]+)/)?.[1];
  else await new Promise((r) => setTimeout(r, 250));
}
const guest = new Client();
await guest.post('/auth/accept-invite', { token: invite, password: 'guestpass123' });
const gl = await guest.post('/auth/login', { email: gEmail, password: 'guestpass123' });
ok('the guest can sign in', gl.status === 200 && !!guest.token, gl.data);
void gid;

// (Before the sweep below, which may make the guest leave their own chat.)
const gChats = (await guest.get('/im/chats')).data;
ok('a guest sees only the chat they were invited to', !JSON.stringify(gChats).includes(`Staff only ${stamp}`) && JSON.stringify(gChats).includes(`Partners ${stamp}`), JSON.stringify(gChats).slice(0, 400));
// The guest's own areas: what api72 shows a guest uses. Anything else answering 2xx is a leak.
const GUEST_OK = /^\/api\/(auth\/|me\/|profile|notifications|realtime\/|files|download-links|im\/|calls|projects|tasks|feed|boards|settings$|users\/me)/;
const all = Object.values(ids).concat([company.id, ann.id, chat.id]) as string[];
const reached: string[] = [];
for (const route of routes()) {
  if (/^\/api\/(public|setup|auth\/(login|refresh|logout|forgot|reset|accept|invite|providers|sso)|crm\/mailboxes\/oauth\/callback|crm\/automation\/webhook)\b/.test(route.path) || route.path === '/api/health' || route.path === '/api/openapi.json') continue;
  const params = route.path.match(/\{[^}]+\}/g) ?? [];
  const candidates = params.length === 0 ? [route.path] : params.length === 1 ? all.map((v) => route.path.replace(params[0], v)) : [fill(route.path, all[0])];
  for (const p of candidates) for (const m of route.methods) {
    const res = (await guest.req(m, p.replace('/api', ''), m === 'GET' || m === 'DELETE' ? undefined : { title: 'guest', body: 'guest', name: 'guest' })) as any;
    if (res.status >= 200 && res.status < 300 && !GUEST_OK.test(route.path)) reached.push(`${m} ${route.path} → ${res.status}`);
    if (res.status >= 200 && res.status < 300 && JSON.stringify(res.data ?? '').includes(`staff secret ${stamp}`)) reached.push(`${m} ${p} shows a staff-only message`);
  }
}
ok('a guest reaches nothing outside their own areas', reached.length === 0, [...new Set(reached)].slice(0, 30));
const gUsers = (await guest.get('/users')) as any;
ok('a guest cannot list the employees', gUsers.status >= 400 || !(gUsers.data.items ?? gUsers.data ?? []).some?.((u: any) => u.email === ann.email), gUsers.status);
ok('a guest cannot read the staff chat', (await guest.get(`/im/chats/${company.id}/messages`)).status >= 400);
ok('a guest cannot invite themselves anywhere', (await guest.post(`/im/chats/${company.id}/guests`, { email: `x.${stamp}@outside.test` })).status >= 400);
ok('a guest cannot open a chat with employees of their choosing', (await guest.post('/im/chats', { title: 'x', members: [bob.id, cat.id] })).status >= 400
  || !JSON.stringify((await cat.get('/im/chats')).data).includes('"title":"x"'));
await finish();
