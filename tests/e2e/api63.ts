// Office documents: PDF previews made by LibreOffice (cached, only for people who may read
// the file); editing through Collabora Online, tested by playing Collabora against NERP's
// WOPI host (discovery from a stand-in on :9988): opening a drive file, CheckFileInfo,
// GetFile, PutFile as a new version, the save conflict, view-only people, bad and foreign
// tokens, locks, and access taken away.
// The editing part needs the server started with COLLABORA_URL=http://127.0.0.1:9988.
import http from 'node:http';
import pg from 'pg';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const B = 'http://localhost:8080/api';
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Doc${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat] = await Promise.all(['Ann', 'Bob', 'Cat'].map(user));
const upload = async (c: Client, name: string, content: string | Uint8Array<ArrayBuffer>, type = 'application/octet-stream') => {
  const fd = new FormData();
  fd.append('file', new Blob([content], { type }), name);
  return (await c.req('POST', '/files', fd)).data;
};
const toDrive = async (c: Client, name: string, content: string) => (await c.post('/drive/files', { file_id: (await upload(c, name, content)).id, name, space: 'personal' })).data;
const RTF = `{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Helvetica;}}\\f0 Quarterly plan ${stamp}\\par}`;

// A stand-in for Collabora's discovery.
const disco = http.createServer((req, res) => {
  if (req.url === '/hosting/discovery') {
    res.writeHead(200, { 'content-type': 'text/xml' });
    return res.end(`<?xml version="1.0" encoding="utf-8"?><wopi-discovery><net-zone name="external-http">
      <app name="writer"><action default="true" ext="rtf" name="edit" urlsrc="http://127.0.0.1:9988/browser/x/cool.html?"/><action ext="odt" name="edit" urlsrc="http://127.0.0.1:9988/browser/x/cool.html?"/></app>
      <app name="calc"><action ext="csv" name="view" urlsrc="http://127.0.0.1:9988/browser/x/cool.html?"/></app></net-zone></wopi-discovery>`);
  }
  res.writeHead(404).end();
});
await new Promise<void>((r) => disco.listen(9988, '127.0.0.1', () => r()));
const wopi = (path: string, init: RequestInit = {}) => fetch(`${B}/wopi/files/${path}`, init);

try {
  // ---- previews ----
  const sheet = await upload(ann, `numbers-${stamp}.csv`, 'Item,Price\nTea,3\nCake,4\n', 'text/csv');
  let r: any = await ann.req('GET', `/files/${sheet.id}/preview`, undefined, true);
  const pdf = new Uint8Array(await r.arrayBuffer());
  ok('a spreadsheet gets a PDF preview', r.status === 200 && r.headers.get('content-type') === 'application/pdf' && new TextDecoder().decode(pdf.slice(0, 5)) === '%PDF-', r.status);
  ok('made once and kept', (await db.query('SELECT 1 FROM file_previews WHERE file_id = $1', [sheet.id])).rowCount === 1);
  const t0 = Date.now();
  r = await ann.req('GET', `/files/${sheet.id}/preview`, undefined, true);
  ok('…so it comes back quickly the second time', r.status === 200 && Date.now() - t0 < 1500, Date.now() - t0);
  ok('not for people who can\'t read the file', (await bob.req('GET', `/files/${sheet.id}/preview`, undefined, true)).status === 404);
  const png = await upload(ann, `pic-${stamp}.png`, new Uint8Array([137, 80, 78, 71]), 'image/png');
  ok('images need no preview', (await ann.req('GET', `/files/${png.id}/preview`, undefined, true)).status === 400);

  // ---- editing (Collabora) ----
  const st = (await ann.get('/office/status')).data;
  if (!st.editing) {
    console.log('skip editing: the server has no COLLABORA_URL (see README)');
  } else if (st.editable.includes('docx')) {
    // A real Collabora (its own list of kinds): ui67 covers it.
    console.log('skip editing: the server uses a real Collabora; see ui67');
  } else {
    ok('the server can edit what Collabora lists', st.editable.includes('rtf') && st.editable.includes('csv'), st);
    const node = await toDrive(ann, `Plan ${stamp}.rtf`, RTF);
    r = await ann.post(`/drive/nodes/${node.id}/office`, {});
    const op = r.data;
    const src = `http://localhost:8080/api/wopi/files/${node.id}`;
    ok('opening gives the editor\'s address with NERP\'s WOPI address, and a token', r.status === 200 && op.url.startsWith('http://127.0.0.1:9988/browser/x/cool.html?WOPISrc=') && op.url.includes(encodeURIComponent(src)) && op.action === 'edit' && op.can_edit && !!op.token, op);
    const tok = encodeURIComponent(op.token);
    let info = await (await wopi(`${node.id}?access_token=${tok}`)).json();
    ok('CheckFileInfo: name, size, version, who, may write', info.BaseFileName === `Plan ${stamp}.rtf` && info.Size === RTF.length && info.Version === '1' && info.UserId === ann.id && info.UserCanWrite === true && info.UserFriendlyName === `Ann Doc${stamp}` && !!info.LastModifiedTime, info);
    r = await wopi(`${node.id}/contents?access_token=${tok}`);
    ok('GetFile: the file itself', r.status === 200 && (await r.text()) === RTF);
    const edited = RTF.replace('Quarterly plan', 'Quarterly plan, revised');
    r = await wopi(`${node.id}/contents?access_token=${tok}`, { method: 'POST', headers: { 'X-WOPI-Override': 'PUT', 'X-COOL-WOPI-Timestamp': info.LastModifiedTime }, body: edited });
    const saved = await r.json();
    ok('PutFile saves', r.status === 200 && !!saved.LastModifiedTime && r.headers.get('x-wopi-itemversion') === '2', saved);
    const nd = (await ann.get(`/drive/nodes/${node.id}`)).data;
    ok('as a new version of the drive file', nd.version === 2 && nd.versions?.length === 2, nd);
    ok('the new content is what downloads', (await (await ann.req('GET', `/drive/nodes/${node.id}/download`, undefined, true)).text()) === edited);
    r = await wopi(`${node.id}/contents?access_token=${tok}`, { method: 'POST', headers: { 'X-WOPI-Override': 'PUT', 'X-COOL-WOPI-Timestamp': info.LastModifiedTime }, body: 'stale' });
    ok('a save over someone else\'s newer save is a conflict', r.status === 409 && (await r.json()).COOLStatusCode === 1010);
    r = await wopi(`${node.id}/contents?access_token=${tok}`, { method: 'POST', headers: { 'X-WOPI-Override': 'PUT' }, body: edited + ' ' });
    ok('without a timestamp (overwrite) it saves', r.status === 200);
    ok('locks are answered', (await wopi(`${node.id}?access_token=${tok}`, { method: 'POST', headers: { 'X-WOPI-Override': 'LOCK', 'X-WOPI-Lock': 'abc' } })).status === 200);

    // People who may only look.
    await ann.put(`/drive/nodes/${node.id}/shares`, [{ user_id: bob.id, level: 'view' }]);
    const asViewer = await bob.post(`/drive/nodes/${node.id}/office`, {});
    ok('a viewer gets the editor read-only (no view action for .rtf here)', asViewer.status === 200 && asViewer.data.can_edit === false, asViewer.data);
    const btok = encodeURIComponent(asViewer.data.token);
    info = await (await wopi(`${node.id}?access_token=${btok}`)).json();
    ok('…which may not write', info.UserCanWrite === false, info);
    ok('…and can\'t save', (await wopi(`${node.id}/contents?access_token=${btok}`, { method: 'POST', headers: { 'X-WOPI-Override': 'PUT' }, body: 'x' })).status === 403);
    ok('people without access can\'t open it', (await cat.post(`/drive/nodes/${node.id}/office`, {})).status === 404);

    // Tokens.
    ok('no token, no file', (await wopi(`${node.id}`)).status !== 200);
    ok('a changed token is refused', (await wopi(`${node.id}?access_token=${tok.slice(0, -3)}abc`)).status === 401);
    const other = await toDrive(ann, `Other ${stamp}.rtf`, RTF);
    ok('a token is for one file', (await wopi(`${other.id}?access_token=${tok}`)).status === 401);
    await ann.put(`/drive/nodes/${node.id}/shares`, []);
    ok('taking access away stops the token at once', (await wopi(`${node.id}?access_token=${btok}`)).status === 401);
    const csvNode = await toDrive(ann, `Prices ${stamp}.csv`, 'a,b\n1,2\n');
    const viewOnly = await ann.post(`/drive/nodes/${csvNode.id}/office`, {});
    ok('a kind Collabora only shows opens to view', viewOnly.status === 200 && viewOnly.data.action === 'view' && viewOnly.data.can_edit === false, viewOnly.data);
    const pngNode = (await ann.post('/drive/files', { file_id: (await upload(ann, `x-${stamp}.png`, new Uint8Array([1]))).id, space: 'personal' })).data;
    ok('kinds Collabora doesn\'t list are refused', (await ann.post(`/drive/nodes/${pngNode.id}/office`, {})).status === 400);
  }
} finally {
  disco.close();
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
