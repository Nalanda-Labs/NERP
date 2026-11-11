// Boards beyond tasks and workgroups: made for a feed post (its readers draw on it or look at
// it, as the author chose; others can't open it), and kept in a drive folder (it shows there and
// opens as the drive lets each person).
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Bd${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};

try {
  const ann = await mk('Ann'), bob = await mk('Bob'), cat = await mk('Cat');

  // ---- a board on a post ----
  let board = (await ann.post('/boards', { title: `Ideas ${stamp}` })).data;
  ok('Bob can\'t open Ann\'s new board', (await bob.get(`/boards/${board.id}`)).status === 404);
  ok('only one\'s own boards go on posts', (await bob.post('/feed', { body: `x ${stamp}`, board_id: board.id })).status === 400);
  let r = await ann.post('/feed', { body: `Brainstorm ${stamp}`, to_all: false, users: [bob.id], board_id: board.id, board_level: 'edit' });
  ok('a post to Bob with the board', r.status === 200 && r.data.board?.id === board.id && r.data.board.level === 'edit', r.data);
  ok('Bob, who reads the post, draws on the board', (await bob.get(`/boards/${board.id}`)).data.level === 'edit');
  ok('Cat, who doesn\'t, can\'t open it', (await cat.get(`/boards/${board.id}`)).status === 404);
  ok('a board goes on one post only', (await ann.post('/feed', { body: `again ${stamp}`, board_id: board.id })).status === 400);
  const look = (await ann.post('/boards', { title: `Plan ${stamp}` })).data;
  await ann.post('/feed', { body: `Our plan ${stamp}`, board_id: look.id, board_level: 'view' });
  ok('a board readers only look at', (await cat.get(`/boards/${look.id}`)).data.level === 'view');
  ok('…and can\'t change', (await cat.req('PUT', `/boards/${look.id}/scene`, { base_version: 0, elements: [{ id: 'x', type: 'rectangle', version: 1, versionNonce: 1 }] })).status === 403);
  const feed = (await bob.get('/feed')).data.items.find((p: any) => p.body === `Brainstorm ${stamp}`);
  ok('the feed shows the board on the post', feed?.board?.title === `Ideas ${stamp}`, feed);

  // ---- a board in the drive ----
  const folder = (await ann.post('/drive/folders', { name: `Workshop ${stamp}` })).data;
  r = await ann.post('/boards', { title: `Flow ${stamp}`, drive: { folder_id: folder.id } });
  ok('a board made in a drive folder', r.status === 200 && !!r.data.drive_node_id, r.data);
  board = r.data;
  const items = (await ann.get(`/drive?folder=${folder.id}`)).data.items;
  const item = items.find((i: any) => i.board_id === board.id);
  ok('it shows in the folder, as a board', item?.name === `Flow ${stamp}.board`, items);
  ok('Bob can\'t open it yet', (await bob.get(`/boards/${board.id}`)).status === 404);
  await ann.put(`/drive/nodes/${folder.id}/shares`, [{ user_id: bob.id, level: 'view' }, { user_id: cat.id, level: 'edit' }]);
  ok('shared folder: Bob looks', (await bob.get(`/boards/${board.id}`)).data.level === 'view');
  ok('…Cat draws', (await cat.get(`/boards/${board.id}`)).data.level === 'edit');
  ok('a board item has no download link', (await ann.post(`/drive/nodes/${item.id}/links`, {})).status === 400);
  await ann.del(`/drive/nodes/${item.id}`);
  ok('in the trash, the folder\'s people lose it', (await cat.get(`/boards/${board.id}`)).status === 404);
  ok('…its owner keeps it', (await ann.get(`/boards/${board.id}`)).status === 200);
  ok('boards can\'t go where one may not add things', (await bob.post('/boards', { title: 'x', drive: { folder_id: folder.id } })).status === 403);
  ok('…and a refused drive place makes no board', !(await bob.get('/boards?scope=mine')).data.items?.some((b: any) => b.title === 'x'));
  // Deleting the board removes its drive item.
  r = await ann.post('/boards', { title: `Temp ${stamp}`, drive: { space: 'personal' } });
  await ann.del(`/boards/${r.data.id}`);
  ok('deleting the board removes its drive item', !(await ann.get('/drive?space=personal')).data.items.some((i: any) => i.board_id === r.data.id));
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
