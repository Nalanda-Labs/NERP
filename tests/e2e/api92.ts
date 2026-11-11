// Drive links that let anyone edit an office file in the browser: only for files one may
// edit; visitors open it in the editor under their own name and save versions (made on the
// link maker's behalf); read-only links open read-only; removing the link (or trashing the
// file) stops it; passwords still apply.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const B = 'http://localhost:8080';
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Ln${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const up = async (c: Client, name: string, text: string) => { const f = new FormData(); f.append('file', new Blob([text]), name); return (await c.req('POST', '/files', f)).data.id as string; };
const pub = async (method: string, path: string, body?: unknown, headers: Record<string, string> = {}) => {
  const r = await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...headers }, body: body ? JSON.stringify(body) : undefined });
  return { status: r.status, data: await r.json().catch(() => null) };
};
const wopi = (node: string, token: string, rest = '') => `${B}/api/wopi/files/${node}${rest}?access_token=${encodeURIComponent(token)}`;

try {
  const ann = await mk('Ann'), bob = await mk('Bob');
  const status = (await ann.get('/office/status')).data;
  if (!status.editing) {
    console.log('skip: no COLLABORA_URL');
    process.exit(0);
  }
  const doc = (await ann.post('/drive/files', { file_id: await up(ann, 'brief.rtf', '{\\rtf1 Brief}'), name: `brief-${stamp}.rtf` })).data;
  const folder = (await ann.post('/drive/folders', { name: `F ${stamp}` })).data;
  // Bob may only look at Ann's document.
  await ann.put(`/drive/nodes/${doc.id}/shares`, [{ user_id: bob.id, level: 'view' }]);

  ok('folders can\'t be shared for editing', (await ann.post(`/drive/nodes/${folder.id}/links`, { can_edit: true })).status === 400);
  ok('nor files one may only look at', (await bob.post(`/drive/nodes/${doc.id}/links`, { can_edit: true })).status >= 400);
  let r = await ann.post(`/drive/nodes/${doc.id}/links`, { can_edit: true });
  ok('an editing link', r.status === 200 && r.data.url.includes('/d/'), r.data);
  const token = r.data.url.split('/d/')[1];
  r = await pub('GET', `/public/drive/${token}`);
  ok('its page says it can be edited in the browser', r.data.can_edit === true && r.data.office === true, r.data);

  r = await pub('POST', `/public/drive/${token}/office`, { name: 'Vera Visitor' });
  ok('a visitor opens it in the editor', r.status === 200 && r.data.can_edit === true && r.data.action === 'edit' && r.data.url.includes('WOPISrc='), r.data);
  const vt = r.data.token as string;
  let info = await fetch(wopi(doc.id, vt)).then((x) => x.json());
  ok('…as themselves, able to write', info.UserCanWrite === true && info.UserFriendlyName === 'Vera Visitor (link)' && info.UserId.startsWith('link-'), info);
  const put = await fetch(wopi(doc.id, vt, '/contents'), { method: 'POST', headers: { 'X-WOPI-Override': 'PUT' }, body: '{\\rtf1 Brief, edited by Vera}' });
  ok('…and saves', put.status === 200, await put.text());
  const node = (await ann.get(`/drive/nodes/${doc.id}`)).data;
  ok('the save is a new version, made on behalf of the link\'s maker', node.version === 2 && (node.updated_by === ann.id || node.updated_by_name?.startsWith('Ann')), node);
  const got = await fetch(wopi(doc.id, vt, '/contents')).then((x) => x.text());
  ok('…with the visitor\'s text', got.includes('edited by Vera'));

  // A read-only link.
  r = await ann.post(`/drive/nodes/${doc.id}/links`, {});
  const roToken = r.data.url.split('/d/')[1];
  ok('a plain link\'s page doesn\'t offer editing', (await pub('GET', `/public/drive/${roToken}`)).data.can_edit === false);
  r = await pub('POST', `/public/drive/${roToken}/office`, { name: 'Rita' });
  const rt = r.data.token as string;
  info = await fetch(wopi(doc.id, rt)).then((x) => x.json());
  ok('…it opens read-only', r.data.can_edit === false && info.UserCanWrite === false, [r.data, info]);
  ok('…and can\'t save', (await fetch(wopi(doc.id, rt, '/contents'), { method: 'POST', headers: { 'X-WOPI-Override': 'PUT' }, body: 'x' })).status === 403);
  ok('a token for one file opens no other', (await fetch(wopi(folder.id, vt))).status === 401);

  // Passwords still apply.
  r = await ann.post(`/drive/nodes/${doc.id}/links`, { can_edit: true, password: 'open-sesame' });
  const pwToken = r.data.url.split('/d/')[1];
  ok('a password link needs its password to open', (await pub('POST', `/public/drive/${pwToken}/office`, { name: 'X' })).status === 401);
  ok('…and opens with it', (await pub('POST', `/public/drive/${pwToken}/office`, { name: 'X' }, { 'X-Link-Password': 'open-sesame' })).status === 200);

  // Only owners make links; a file in the trash stops being edited through its link.
  ok('only the owner makes links', (await bob.post(`/drive/nodes/${doc.id}/links`, { can_edit: true })).status === 403);
  await ann.del(`/drive/nodes/${doc.id}`);
  ok('a file moved to the trash closes its link editors', (await fetch(wopi(doc.id, vt))).status === 401);
  await ann.post(`/drive/nodes/${doc.id}/restore`, {});
  ok('…and restored, works again', (await fetch(wopi(doc.id, vt))).status === 200);
  // Removing the link.
  for (const l of (await ann.get(`/drive/nodes/${doc.id}`)).data.links ?? []) await ann.del(`/drive/links/${l.id}`);
  ok('removing the link stops its editor at once', (await fetch(wopi(doc.id, vt))).status === 401);
  ok('…and its page', (await pub('GET', `/public/drive/${token}`)).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
