// Whiteboards: making boards (blank, from a drawing, from a template, inside a task or a
// workgroup); who may open or edit them (sharing, departments, everyone, task and workgroup
// people, locking); saving together (element-by-element merge, versions, who's looking, a
// live event to others looking); images; public links (view, edit, password, turned off);
// templates; the board's chat; deleting.
import { Client, sleep } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const B = 'http://localhost:8080/api';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Board${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat, dan] = await Promise.all(['Ann', 'Bob', 'Cat', 'Dan'].map(user));
const el = (id: string, version = 1, nonce = 1, extra: object = {}) => ({ id, type: 'rectangle', x: 0, y: 0, width: 10, height: 10, version, versionNonce: nonce, isDeleted: false, ...extra });
const pub = async (method: string, path: string, body?: unknown, pw?: string) => {
  const headers: Record<string, string> = {};
  if (body) headers['content-type'] = 'application/json';
  if (pw) headers['x-board-password'] = pw;
  const r = await fetch(B + path, { method, headers, body: body ? JSON.stringify(body) : undefined });
  const t = await r.text();
  let data: any; try { data = JSON.parse(t); } catch { data = t; }
  return { status: r.status, data };
};
const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

try {
  // ---- making boards ----
  ok('a board needs a name', (await ann.post('/boards', { title: ' ' })).status === 400);
  let r = await ann.post('/boards', { title: `Plan ${stamp}`, scene: { elements: [el('a'), el('b')], appState: { viewBackgroundColor: '#fffbe6', zoom: { value: 3 } } } });
  const plan = r.data;
  ok('a board from a drawing', r.status === 200 && plan.level === 'owner' && plan.version === 0, r.data);
  r = await ann.get(`/boards/${plan.id}`);
  ok('it opens with its drawing; only lasting settings are kept', r.data.scene.elements.length === 2 && r.data.scene.appState.viewBackgroundColor === '#fffbe6' && !r.data.scene.appState.zoom && Array.isArray(r.data.access), r.data.scene);
  ok('shapes need ids', (await ann.post('/boards', { title: `Bad ${stamp}`, scene: { elements: [{ type: 'x' }] } })).status === 400);
  ok('others can\'t see it', (await bob.get(`/boards/${plan.id}`)).status === 404 && !(await bob.get('/boards?scope=shared')).data.items.some((b: any) => b.id === plan.id));

  // ---- saving together ----
  r = await ann.put(`/boards/${plan.id}/scene`, { elements: [el('a', 2, 5, { x: 50 })] });
  ok('a save raises the version', r.status === 200 && r.data.version === 1 && r.data.elements.find((e: any) => e.id === 'a').x === 50, r.data);
  await ann.put(`/boards/${plan.id}/access`, { grants: [{ kind: 'user', id: bob.id, level: 'edit' }] });
  const sock = await ann.ws();
  await ann.get(`/boards/${plan.id}/scene`);
  r = await bob.put(`/boards/${plan.id}/scene`, { elements: [el('a', 1, 0, { x: 999 }), el('c', 1, 1)] });
  ok('an older change to a shape is ignored; a new shape is added', r.data.version === 2 && r.data.elements.find((e: any) => e.id === 'a').x === 50 && r.data.elements.some((e: any) => e.id === 'c'), r.data);
  await sleep(500);
  ok('people looking get a live note', sock.events.some((e: any) => e.type === 'board.changed' && e.data.id === plan.id && e.data.version === 2), sock.events.map((e: any) => e.type));
  sock.close();
  r = await bob.put(`/boards/${plan.id}/scene`, { elements: [el('a', 2, 9, { x: 7 })] });
  ok('same version, higher nonce: the stored one stays', r.data.version === 2 && r.data.elements.find((e: any) => e.id === 'a').x === 50, r.data);
  r = await bob.put(`/boards/${plan.id}/scene`, { elements: [el('a', 3, 9, { x: 70, isDeleted: true })] });
  ok('a newer change (even a delete) wins', r.data.version === 3 && r.data.elements.find((e: any) => e.id === 'a').isDeleted === true, r.data);
  await bob.get(`/boards/${plan.id}/scene?since=3`);
  r = await ann.get(`/boards/${plan.id}/scene?since=3`);
  ok('nothing newer: just the version and who\'s looking', r.data.unchanged === true && !r.data.scene && r.data.viewers.some((v: any) => v.name === `Ann Board${stamp}`) && r.data.viewers.some((v: any) => v.name === `Bob Board${stamp}`), r.data);
  r = await ann.get(`/boards/${plan.id}/scene?since=1`);
  ok('…or the whole drawing when there is', r.data.version === 3 && r.data.scene.elements.length === 3, r.data);
  r = await ann.put(`/boards/${plan.id}/scene`, { elements: [], appState: { viewBackgroundColor: '#e7f5ff' } });
  ok('the background colour is saved too', r.data.version === 4 && (await ann.get(`/boards/${plan.id}`)).data.scene.appState.viewBackgroundColor === '#e7f5ff');

  // ---- who may do what ----
  await ann.put(`/boards/${plan.id}/access`, { grants: [{ kind: 'user', id: bob.id, level: 'view' }] });
  ok('view only: can open, can\'t save', (await bob.get(`/boards/${plan.id}`)).data.level === 'view' && (await bob.put(`/boards/${plan.id}/scene`, { elements: [el('d')] })).status === 403);
  ok('only the owner shares', (await bob.put(`/boards/${plan.id}/access`, { grants: [] })).status === 403);
  const dept = (await admin.post('/departments', { name: `Design ${stamp}` })).data;
  await admin.post(`/departments/${dept.id}/members`, { user_id: cat.id });
  await ann.put(`/boards/${plan.id}/access`, { grants: [{ kind: 'user', id: bob.id, level: 'edit' }, { kind: 'department', id: dept.id, level: 'edit' }] });
  ok('a department can edit', (await cat.put(`/boards/${plan.id}/scene`, { elements: [el('e')] })).status === 200);
  ok('shared boards are listed', (await cat.get('/boards?scope=shared')).data.items.some((b: any) => b.id === plan.id));
  await ann.patch(`/boards/${plan.id}`, { locked: true });
  ok('a locked board is read-only to others', (await bob.get(`/boards/${plan.id}`)).data.level === 'view' && (await bob.put(`/boards/${plan.id}/scene`, { elements: [el('f')] })).status === 403);
  ok('but not to its owner', (await ann.put(`/boards/${plan.id}/scene`, { elements: [el('f')] })).status === 200);
  ok('only the owner locks it', (await bob.patch(`/boards/${plan.id}`, { locked: false })).status === 403);
  await ann.patch(`/boards/${plan.id}`, { locked: false });
  ok('editors may rename it', (await bob.patch(`/boards/${plan.id}`, { title: `Plan B ${stamp}` })).data.title === `Plan B ${stamp}`);
  await ann.put(`/boards/${plan.id}/access`, { grants: [{ kind: 'user', id: bob.id, level: 'edit' }, { kind: 'everyone', level: 'view' }] });
  ok('everyone may view', (await dan.get(`/boards/${plan.id}`)).data.level === 'view');

  // ---- task and workgroup boards ----
  const task = (await ann.post('/tasks', { title: `Draw it ${stamp}`, responsible_id: dan.id })).data;
  r = await ann.post('/boards', { title: `Task board ${stamp}`, task_id: task.id });
  const tb = r.data;
  ok('a board inside a task', r.status === 200 && tb.task?.number === task.number, r.data);
  ok('the task\'s people may edit it', (await dan.get(`/boards/${tb.id}`)).data.level === 'edit' && (await dan.put(`/boards/${tb.id}/scene`, { elements: [el('t')] })).status === 200);
  ok('it is listed with the task', (await dan.get(`/boards?scope=task&task_id=${task.id}`)).data.items.some((b: any) => b.id === tb.id));
  ok('others can\'t open it', (await cat.get(`/boards/${tb.id}`)).status === 404);
  ok('a board in someone else\'s task can\'t be made', (await cat.post('/boards', { title: `X ${stamp}`, task_id: task.id })).status === 404);
  const group = (await ann.post('/projects', { name: `Group ${stamp}`, visibility: 'private', member_ids: [cat.id] })).data;
  r = await ann.post('/boards', { title: `Group board ${stamp}`, project_id: group.id });
  ok('a board inside a workgroup', r.status === 200 && r.data.project?.name === `Group ${stamp}`, r.data);
  const gb = r.data;
  ok('its members may edit it', (await cat.get(`/boards/${gb.id}`)).data.level === 'edit');
  ok('people outside a private group can\'t open it', (await dan.get(`/boards/${gb.id}`)).status === 404);

  // ---- images ----
  r = await ann.post(`/boards/${plan.id}/files`, { key: 'img1', data_url: PNG });
  ok('an image goes on the board as a file', r.status === 200 && !!r.data.file_id, r.data);
  let f = await ann.req('GET', `/boards/${plan.id}/files/img1`, undefined, true);
  ok('it is served safely', f.status === 200 && f.headers.get('content-type') === 'image/png' && f.headers.get('x-content-type-options') === 'nosniff');
  ok('people who may open the board may see it', (await dan.req('GET', `/boards/${plan.id}/files/img1`, undefined, true)).status === 200);
  ok('only images', (await ann.post(`/boards/${plan.id}/files`, { key: 'x', data_url: 'data:text/html;base64,PGI+aGk8L2I+' })).status === 400);
  const other = (await cat.post('/boards', { title: `Cat's ${stamp}` })).data;
  ok('a file id must be one\'s own', (await cat.post(`/boards/${other.id}/files`, { key: 'k', file_id: r.data.file_id })).status === 403);

  // ---- templates and copies ----
  await ann.patch(`/boards/${plan.id}`, { is_template: true });
  ok('a template is offered to everyone', (await cat.get('/boards?scope=templates')).data.items.some((b: any) => b.id === plan.id));
  r = await cat.post('/boards', { title: `From template ${stamp}`, from: plan.id });
  ok('a board from a template copies the drawing and images', r.status === 200 && (await cat.get(`/boards/${r.data.id}`)).data.files.some((x: any) => x.key === 'img1'), r.data);
  ok('…as its own board', (await cat.get(`/boards/${r.data.id}`)).data.level === 'owner');

  // ---- public links ----
  ok('only the owner makes links', (await bob.post(`/boards/${plan.id}/links`, { level: 'view' })).status === 403);
  ok('an expiry in the past is refused', (await ann.post(`/boards/${plan.id}/links`, { level: 'view', expires_at: '2020-01-01T00:00:00Z' })).status === 400);
  const view = (await ann.post(`/boards/${plan.id}/links`, { level: 'view' })).data;
  const vt = view.url.split('/b/')[1];
  r = await pub('GET', `/public/boards/${vt}`);
  ok('a view link opens it for anyone', r.status === 200 && r.data.level === 'view' && r.data.scene.elements.length >= 3, r.data);
  ok('…but can\'t change it', (await pub('PUT', `/public/boards/${vt}/scene`, { elements: [el('g')] })).status === 403);
  ok('…and serves its images', (await fetch(`${B}/public/boards/${vt}/files/img1`)).status === 200);
  const edit = (await ann.post(`/boards/${plan.id}/links`, { level: 'edit', expires_at: new Date(Date.now() + 86400e3).toISOString(), password: 'open sesame' })).data;
  const et = edit.url.split('/b/')[1];
  ok('a password link asks for it', (await pub('GET', `/public/boards/${et}`)).status === 401);
  ok('a wrong password is refused', (await pub('GET', `/public/boards/${et}`, undefined, 'nope')).status === 401);
  r = await pub('PUT', `/public/boards/${et}/scene`, { elements: [el('guest', 1, 1)] }, 'open sesame');
  ok('with it, a guest edits', r.status === 200 && r.data.elements.some((e: any) => e.id === 'guest'), r.data);
  ok('…and adds images', (await pub('POST', `/public/boards/${et}/files`, { key: 'gimg', data_url: PNG }, 'open sesame')).status === 200);
  ok('a guest can\'t use NERP file ids', (await pub('POST', `/public/boards/${et}/files`, { key: 'k2', file_id: r.data.elements[0]?.id }, 'open sesame')).status !== 200);
  r = await pub('GET', `/public/boards/${et}/scene?since=0`, undefined, 'open sesame');
  ok('guests see who is looking', r.data.viewers.some((v: any) => v.name === 'Guest'), r.data.viewers);
  const list = (await ann.get(`/boards/${plan.id}/links`)).data.items;
  ok('links are listed without their address', list.length === 2 && list.some((l: any) => l.password && l.level === 'edit') && !JSON.stringify(list).includes(et), list);
  await ann.del(`/boards/${plan.id}/links/${list.find((l: any) => l.level === 'edit').id}`);
  ok('a link turned off is gone', (await pub('GET', `/public/boards/${et}`, undefined, 'open sesame')).status === 404);
  ok('a made-up link finds nothing', (await pub('GET', '/public/boards/nope')).status === 404);

  // ---- chat ----
  r = await ann.post(`/boards/${plan.id}/chat`);
  ok('the board has a chat', r.status === 200 && !!r.data.chat_id, r.data);
  ok('for the people it is shared with by name', (await bob.post(`/boards/${plan.id}/chat`)).data.chat_id === r.data.chat_id);
  ok('not for everyone', (await dan.post(`/boards/${plan.id}/chat`)).status === 400);

  // ---- deleting ----
  ok('only the owner deletes', (await bob.del(`/boards/${plan.id}`)).status === 403);
  ok('the owner does', (await ann.del(`/boards/${plan.id}`)).status === 200 && (await ann.get(`/boards/${plan.id}`)).status === 404);
  ok('and its links stop', (await pub('GET', `/public/boards/${vt}`)).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
