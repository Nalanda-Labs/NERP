// Drive: My drive and the company drive (folders, uploads, names, versions), sharing (with
// people and departments, view and edit, inside shared folders), public links (expiry,
// removal), file access, moving, the trash (restore, delete for good), search.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Drive' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');
const upload = async (c: Client, name: string, text: string, where: any) => {
  const fd = new FormData(); fd.append('file', new Blob([text], { type: 'text/plain' }), name);
  const f = (await c.req('POST', '/files', fd)).data;
  return c.post('/drive/files', { file_id: f.id, name, ...where });
};
const body = async (c: Client, path: string) => { const r = await c.req('GET', path, undefined, true); return r.status === 200 ? await r.text() : `status ${r.status}`; };

try {
  // ---- My drive ----
  let r = await ann.post('/drive/folders', { name: `Projects ${stamp}` });
  const projects = r.data;
  ok('a folder', r.status === 200 && projects.kind === 'folder' && projects.level === 'own');
  ok('names unique in a folder', (await ann.post('/drive/folders', { name: `projects ${stamp}` })).status === 400);
  ok('no slashes', (await ann.post('/drive/folders', { name: 'a/b' })).status === 400);
  r = await upload(ann, `plan-${stamp}.txt`, 'version one', { parent_id: projects.id });
  const plan = r.data;
  ok('a file', plan.version === 1 && plan.size_bytes === 11 && plan.versions.length === 1);
  r = await upload(ann, `plan-${stamp}.txt`, 'version two!', { parent_id: projects.id });
  ok('same name: a new version', r.data.id === plan.id && r.data.version === 2 && r.data.versions.length === 2, r.data);
  ok('download: the newest', (await body(ann, `/drive/nodes/${plan.id}/download`)) === 'version two!');
  ok('an older version', (await body(ann, `/drive/nodes/${plan.id}/download?version=1`)) === 'version one');
  r = await ann.post(`/drive/nodes/${plan.id}/versions/1/restore`, {});
  ok('restoring makes it the newest', r.data.version === 3 && (await body(ann, `/drive/nodes/${plan.id}/download`)) === 'version one');
  ok('My drive is private', (await bob.get(`/drive/nodes/${plan.id}`)).status === 404 && (await body(bob, `/drive/nodes/${plan.id}/download`)) === 'status 404');
  const listing = (await ann.get(`/drive?folder=${projects.id}`)).data;
  ok('browsing a folder', listing.items.length === 1 && listing.path.map((p: any) => p.name).join() === `Projects ${stamp}`, listing);
  {
    const fd = new FormData(); fd.append('file', new Blob(['once']), `once-${stamp}.txt`);
    const f = (await ann.req('POST', '/files', fd)).data;
    const first = await ann.post('/drive/files', { file_id: f.id, name: `once-${stamp}.txt` });
    ok('an upload is added once', first.status === 200 && (await ann.post('/drive/files', { file_id: f.id, name: `twice-${stamp}.txt` })).status === 400);
    const theirs = await ann.post('/drive/files', { file_id: f.id, name: 'x' });
    const bobsTry = await bob.post('/drive/files', { file_id: f.id, name: `steal-${stamp}.txt` });
    ok('only your own uploads', theirs.status === 400 && bobsTry.status === 400);
  }

  // ---- sharing ----
  const team = (await admin.post('/departments', { name: `Team ${stamp}` })).data;
  await admin.post(`/departments/${team.id}/members`, { user_id: cat.id });
  ok('can\'t share with yourself', (await ann.put(`/drive/nodes/${projects.id}/shares`, [{ user_id: ann.id, level: 'view' }])).status === 400);
  r = await ann.put(`/drive/nodes/${projects.id}/shares`, [{ user_id: bob.id, level: 'edit' }, { department_id: team.id, level: 'view' }]);
  ok('shared', r.data.shares.length === 2);
  ok('it shows under Shared with me', (await bob.get('/drive?space=shared')).data.items.some((n: any) => n.id === projects.id));
  ok('what is inside is shared too', (await body(bob, `/drive/nodes/${plan.id}/download`)) === 'version one' && (await body(cat, `/drive/nodes/${plan.id}/download`)) === 'version one');
  r = await upload(bob, `notes-${stamp}.txt`, 'bob was here', { parent_id: projects.id });
  ok('editors add files', r.status === 200 && r.data.level === 'edit');
  ok('viewers don\'t', (await upload(cat, `cat-${stamp}.txt`, 'x', { parent_id: projects.id })).status === 403);
  ok('editors don\'t delete or rename', (await bob.req('DELETE', `/drive/nodes/${plan.id}`)).status === 403 && (await bob.patch(`/drive/nodes/${plan.id}`, { name: 'x' })).status === 403);
  await ann.put(`/drive/nodes/${projects.id}/shares`, []);
  ok('unshared', (await bob.get(`/drive/nodes/${plan.id}`)).status === 404);

  // ---- public links ----
  ok('links are for files', (await ann.post(`/drive/nodes/${projects.id}/links`, {})).status === 400);
  r = await ann.post(`/drive/nodes/${plan.id}/links`, { expires_days: 7 });
  const token = r.data.url.split('/d/')[1];
  ok('a link', r.data.links.length === 1 && token.length >= 30);
  const pub = new Client();
  ok('anyone with it sees the name', (await pub.get(`/public/drive/${token}`)).data.name === `plan-${stamp}.txt`);
  const dl = await fetch(`http://localhost:8080/api/public/drive/${token}/download`);
  ok('and downloads it', dl.status === 200 && (await dl.text()) === 'version one' && /attachment/.test(dl.headers.get('content-disposition') ?? ''));
  ok('downloads counted', (await ann.get(`/drive/nodes/${plan.id}`)).data.links[0].downloads === 1);
  await ann.req('DELETE', `/drive/links/${r.data.links[0].id}`);
  ok('removed links stop working', (await pub.get(`/public/drive/${token}`)).status === 404);

  // ---- the company drive ----
  r = await bob.post('/drive/folders', { space: 'company', name: `Handbooks ${stamp}` });
  const hb = r.data;
  ok('company folder', hb.space === 'company' && hb.level === 'own');
  ok('everyone adds to it', (await upload(cat, `rules-${stamp}.txt`, 'be nice', { parent_id: hb.id })).data.level === 'own');
  ok('but only the creator (or a drive manager) renames it', (await cat.patch(`/drive/nodes/${hb.id}`, { name: 'x' })).status === 403 && (await admin.patch(`/drive/nodes/${hb.id}`, { name: `Handbooks 2 ${stamp}` })).status === 200);
  ok('company drive can\'t be "shared"', (await bob.put(`/drive/nodes/${hb.id}/shares`, [{ user_id: ann.id, level: 'view' }])).status === 400);

  // ---- moving ----
  const sub = (await ann.post('/drive/folders', { name: `Archive ${stamp}`, parent_id: projects.id })).data;
  ok('into itself', (await ann.patch(`/drive/nodes/${projects.id}`, { parent_id: sub.id })).status === 400);
  ok('into another drive', (await ann.patch(`/drive/nodes/${plan.id}`, { parent_id: hb.id })).status === 400);
  r = await ann.patch(`/drive/nodes/${plan.id}`, { parent_id: sub.id });
  ok('moved', r.data.parent_id === sub.id);

  // ---- trash ----
  await ann.req('DELETE', `/drive/nodes/${projects.id}`);
  ok('deleting a folder takes its contents', (await ann.get(`/drive?folder=${sub.id}`)).status === 404 || (await ann.get('/drive')).data.items.every((n: any) => n.id !== projects.id));
  r = await ann.get('/drive/trash');
  ok('the trash shows the folder (not each thing inside)', r.data.items.length === 1 && r.data.items[0].id === projects.id, r.data);
  await ann.post(`/drive/nodes/${projects.id}/restore`, {});
  ok('put back with its contents', (await body(ann, `/drive/nodes/${plan.id}/download`)) === 'version one');
  await ann.req('DELETE', `/drive/nodes/${plan.id}`);
  ok('delete for good', (await ann.req('DELETE', `/drive/trash/${plan.id}`)).status === 200 && (await ann.get(`/drive/nodes/${plan.id}`)).status === 404);

  // ---- search ----
  r = await cat.get(`/drive/search?q=rules-${stamp}`);
  ok('search finds what you can see', r.data.items.length === 1);
  ok('not other people\'s files', (await cat.get(`/drive/search?q=notes-${stamp}`)).data.items.length === 0);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
