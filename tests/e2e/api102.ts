// Files on a record's timeline: a comment carries files (or is only files); whoever can see
// the record can download them, nobody else; only one's own uploads can be attached, at most
// 20; deleting the comment removes its files.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${s}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Tf${s}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await mk('Ann'); // the deal is hers
const bob = await mk('Bob');
// For this suite, employees see only their own deals (as api13 does; put back at the end).
const empRole = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
const perms = (await admin.get('/crm/permissions')).data;
const original = Object.entries(perms.matrix[empRole.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
await admin.put(`/crm/permissions/${empRole.id}`, { entries: original.map((e: any) => (e.entity_type === 2 && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
const upload = async (c: Client, name: string, text: string) => {
  const fd = new FormData(); fd.append('file', new Blob([text], { type: 'text/plain' }), name);
  return (await c.req('POST', '/files', fd)).data.id as string;
};
const download = async (c: Client, id: string) => (await c.req('GET', `/files/${id}/content`, undefined, true)).status;
let dealId = '';

try {
  const deal = (await admin.post('/crm/deal/items', { title: `Files deal ${s}`, assigned_to: ann.id })).data;
  dealId = deal.id;
  const seen = await bob.get(`/crm/deal/items/${deal.id}`);
  ok('Bob can\'t see the deal', seen.status >= 403, [seen.status, (await bob.get('/auth/me')).data?.roles]);
  const f1 = await upload(admin, `contract-${s}.txt`, 'the contract');
  const f2 = await upload(admin, `notes-${s}.txt`, 'some notes');
  let r = await admin.post(`/crm/items/${deal.id}/timeline`, { body: 'Signed copy attached', file_ids: [f1, f2] });
  ok('a comment with files', r.status === 200, r.data);
  r = await admin.post(`/crm/items/${deal.id}/timeline`, { file_ids: [await upload(admin, `only-${s}.txt`, 'just a file')] });
  ok('…or only files', r.status === 200, r.data);
  ok('without text or files: refused', (await admin.post(`/crm/items/${deal.id}/timeline`, { body: ' ' })).status === 400);
  const tl = (await admin.get(`/crm/items/${deal.id}/timeline`)).data.items;
  const c = tl.find((e: any) => e.body === 'Signed copy attached');
  ok('the timeline lists them, in order, with name and size', c?.files?.length === 2 && c.files[0].id === f1 && c.files[0].name === `contract-${s}.txt` && c.files[0].size === 12 && c.files[1].id === f2, c);
  ok('someone who can see the deal downloads them', (await download(ann, f1)) === 200);
  ok('someone who can\'t see it can\'t', [403, 404].includes(await download(bob, f1)));
  const bobs = await upload(bob, `bob-${s}.txt`, 'mine');
  ok('only one\'s own uploads can be attached', (await ann.post(`/crm/items/${deal.id}/timeline`, { body: 'x', file_ids: [bobs] })).status === 400);
  ok('at most 20', (await admin.post(`/crm/items/${deal.id}/timeline`, { body: 'x', file_ids: Array.from({ length: 21 }, () => crypto.randomUUID()) })).status === 400);
  ok('a comment needs access to the record', (await bob.post(`/crm/items/${deal.id}/timeline`, { body: 'x', file_ids: [bobs] })).status >= 403);
  ok('deleting the comment', (await admin.del(`/crm/timeline/${c.id}`)).status === 200);
  ok('…removes its files', (await admin.get(`/files/${f1}`)).status === 404 && (await admin.get(`/files/${f2}`)).status === 404);
} finally {
  if (dealId) await admin.del(`/crm/deal/items/${dealId}`).catch(() => {});
  await admin.put(`/crm/permissions/${empRole.id}`, { entries: original });
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
