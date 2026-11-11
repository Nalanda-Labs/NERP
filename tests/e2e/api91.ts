// Live co-editing of knowledge base pages: one editor seeds the shared document, updates are
// kept (and merged) for later joiners and passed on to the others in real time, cursors too;
// only editors take part; bad updates are refused; a change outside the live document resets it.
import * as Y from 'yjs';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Co${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const b64 = (u: Uint8Array) => Buffer.from(u).toString('base64');
const unb64 = (s: string) => new Uint8Array(Buffer.from(s, 'base64'));
/** Real-time events a person receives (over their WebSocket). */
const listen = async (c: Client) => {
  const { ticket } = (await c.post('/realtime/ticket')).data;
  const ws = new WebSocket(`ws://localhost:8080/api/realtime/ws?ticket=${encodeURIComponent(ticket)}`);
  const events: any[] = [];
  ws.onmessage = (e) => events.push(JSON.parse(String(e.data)));
  await new Promise((r) => (ws.onopen = r));
  return { events, close: () => ws.close() };
};
let spaceId = '';

try {
  const ann = await mk('Ann'), bob = await mk('Bob'), cat = await mk('Cat');
  const space = (await admin.post('/kb/spaces', { name: `Live ${stamp}` })).data;
  spaceId = space.id;
  await admin.patch(`/kb/spaces/${space.id}`, { to_all: true, editors: { users: [ann.id, bob.id], departments: [] } });
  const page = (await ann.post(`/kb/spaces/${space.id}/pages`, { title: `Plan ${stamp}`, body: 'Saved text' })).data;
  const P = `/kb/pages/${page.id}/collab`;

  const annDoc = new Y.Doc(), bobDoc = new Y.Doc();
  let r = await ann.post(P, { client: String(annDoc.clientID) });
  ok('the first editor seeds the live document', r.status === 200 && r.data.seed === true && r.data.state === null, r.data);
  r = await bob.post(P, { client: String(bobDoc.clientID) });
  ok('…the next one doesn\'t (no double text)', r.data.seed === false && r.data.state === null && r.data.people.some((p: any) => p.name === `Ann Co${stamp}`), r.data);
  ok('readers who can\'t edit aren\'t let in', (await cat.post(P, { client: '123' })).status === 403);

  const bobEars = await listen(bob);
  // Ann puts the saved text in and types.
  const updates: Uint8Array[] = [];
  annDoc.on('update', (u: Uint8Array) => updates.push(u));
  annDoc.getText('t').insert(0, 'Saved text');
  annDoc.getText('t').insert(10, ', and more');
  for (const u of updates) {
    r = await ann.post(`${P}/update`, { client: String(annDoc.clientID), update: b64(u) });
    if (r.status !== 200) break;
  }
  ok('updates are taken', r.status === 200, r.data);
  await sleep(500);
  const got = bobEars.events.filter((e) => e.type === 'kb.collab' && e.data.page_id === page.id);
  for (const e of got) Y.applyUpdate(bobDoc, unb64(e.data.update));
  ok('…and reach the other editor live', got.length === 2 && bobDoc.getText('t').toString() === 'Saved text, and more', [got.length, bobDoc.getText('t').toString()]);

  // Bob types at the same time; both converge.
  const bobUpdates: Uint8Array[] = [];
  bobDoc.on('update', (u: Uint8Array, origin: unknown) => origin !== 'remote' && bobUpdates.push(u));
  bobDoc.getText('t').insert(0, 'Bob: ');
  for (const u of bobUpdates) await bob.post(`${P}/update`, { client: String(bobDoc.clientID), update: b64(u) });
  // A late joiner gets everything, merged.
  const late = new Y.Doc();
  r = await admin.post(P, { client: String(late.clientID) });
  Y.applyUpdate(late, unb64(r.data.state));
  ok('someone joining later gets the whole document', late.getText('t').toString() === 'Bob: Saved text, and more', late.getText('t').toString());

  // Many updates are merged into one (and nothing is lost).
  for (let i = 0; i < 70; i++) {
    const u: Uint8Array[] = [];
    const h = (x: Uint8Array) => u.push(x);
    annDoc.on('update', h);
    annDoc.getText('t').insert(annDoc.getText('t').length, '.');
    annDoc.off('update', h);
    await ann.post(`${P}/update`, { client: String(annDoc.clientID), update: b64(u[0]!) });
  }
  const again = new Y.Doc();
  r = await admin.post(P, { client: String(again.clientID) });
  Y.applyUpdate(again, unb64(r.data.state));
  ok('after merging, still everything', again.getText('t').toString().endsWith('.'.repeat(70)) && again.getText('t').toString().startsWith('Saved text') === false, again.getText('t').toString().slice(0, 60));

  ok('garbage isn\'t an update', (await ann.post(`${P}/update`, { client: '1', update: b64(new Uint8Array([9, 9, 9, 9])) })).status === 400);
  ok('nor is nonsense base64', (await ann.post(`${P}/update`, { client: '1', update: '%%%' })).status === 400);
  ok('client ids are checked', (await ann.post(P, { client: '<script>' })).status === 400);

  // Cursors.
  r = await ann.post(`${P}/awareness`, { client: String(annDoc.clientID), update: b64(new Uint8Array([1, 2, 3])) });
  await sleep(300);
  ok('cursor updates reach the others', r.status === 200 && bobEars.events.some((e) => e.type === 'kb.collab.awareness' && e.data.client === String(annDoc.clientID)));

  // A save from the live document keeps it; a Markdown save resets it.
  r = await bob.patch(`/kb/pages/${page.id}`, { title: `Plan ${stamp}`, body: 'Bob: Saved text, and more', collab: true, version: 1 });
  ok('saving from the live document skips the version check', r.status === 200 && r.data.version === 2, r.data);
  ok('…and the live document carries on', (await admin.post(P, { client: String(late.clientID) })).data.state !== null);
  r = await ann.patch(`/kb/pages/${page.id}`, { title: `Plan ${stamp}`, body: 'Rewritten in Markdown', version: 2 });
  await sleep(300);
  ok('a Markdown save resets the live document for its editors', r.status === 200 && bobEars.events.some((e) => e.type === 'kb.collab.reset' && e.data.page_id === page.id));
  r = await bob.post(P, { client: String(bobDoc.clientID) });
  ok('…who start again from the saved page', r.data.state === null && r.data.seed === true, r.data);
  await bob.post(`${P}/leave`, { client: String(bobDoc.clientID) });
  r = await ann.post(P, { client: String(annDoc.clientID) });
  ok('people who left aren\'t listed', !r.data.people.some((p: any) => p.client === String(bobDoc.clientID)), r.data.people);
  bobEars.close();
} finally {
  if (spaceId) await admin.del(`/kb/spaces/${spaceId}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
