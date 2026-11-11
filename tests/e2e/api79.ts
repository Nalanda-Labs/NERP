// Guided tours (offered once, played again on request), guests working on their workgroups'
// files (and only those) including editing them in Collabora, and restoring a backup from the
// app (with the password and the backup's name; the app pauses and comes back on the
// backup's data, brought up to date).
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const waitFor = async <T>(f: () => Promise<T | null | undefined | false>, ms = 20000): Promise<T | null> => { const end = Date.now() + ms; while (Date.now() < end) { const v = await f().catch(() => null); if (v) return v as T; await sleep(500); } return null; };
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Gd${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const up = async (c: Client, name: string, text: string) => { const f = new FormData(); f.append('file', new Blob([text]), name); return (await c.req('POST', '/files', f)).data.id as string; };

try {
  // ---- tours ----
  const ann = await user('Ann');
  let r = await ann.get('/me/tours');
  ok('a new person has seen no tour', r.status === 200 && r.data.done.length === 0 && r.data.tours.includes('welcome'), r.data);
  r = await ann.post('/me/tours/welcome');
  ok('a tour seen is remembered', r.data.done.includes('welcome'));
  await ann.post('/me/tours/welcome');
  ok('…once', (await ann.get('/me/tours')).data.done.filter((t: string) => t === 'welcome').length === 1);
  ok('unknown tours are refused', (await ann.post('/me/tours/nope')).status === 404);
  ok('"Take the tour" starts over', (await ann.del('/me/tours')).data.done.length === 0);

  // ---- a guest and the workgroup's files ----
  const group = (await ann.post('/projects', { name: `Partner work ${stamp}`, kind: 'group', visibility: 'closed' })).data;
  const other = (await ann.post('/projects', { name: `Inside ${stamp}`, kind: 'group', visibility: 'open' })).data;
  const gEmail = `guest.${stamp}@outside.test`;
  await ann.post(`/projects/${group.id}/guests`, { email: gEmail, first_name: 'Gus', last_name: `Out${stamp}` });
  const token = await waitFor(async () => {
    const m = (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent('to:' + gEmail)}&limit=5`)).json()).messages?.[0];
    return m ? (await (await fetch(`http://localhost:8026/api/v1/message/${m.ID}`)).json()).Text.match(/accept-invite\?token=([\w-]+)/)?.[1] : null;
  });
  const gus = new Client();
  await gus.post('/auth/accept-invite', { token, password: 'guestpass123' });
  await gus.post('/auth/login', { email: gEmail, password: 'guestpass123' });
  ok('the guest is in', !!gus.token);
  const doc = (await ann.post('/drive/files', { file_id: await up(ann, 'plan.rtf', '{\\rtf1 Plan}'), name: `plan-${stamp}.rtf`, space: 'group', project_id: group.id })).data;
  const secret = (await ann.post('/drive/files', { file_id: await up(ann, 'mine.txt', 'mine'), name: `mine-${stamp}.txt` })).data;
  const company = (await admin.post('/drive/files', { file_id: await up(admin, 'all.txt', 'company'), name: `company-${stamp}.txt`, space: 'company' })).data;
  const open = (await ann.post('/drive/files', { file_id: await up(ann, 'open.txt', 'open group'), name: `open-${stamp}.txt`, space: 'group', project_id: other.id })).data;
  r = await gus.get(`/drive?space=group&project_id=${group.id}`);
  ok('the guest lists their workgroup\'s files', r.status === 200 && r.data.items.some((i: any) => i.id === doc.id), r.data);
  ok('…and downloads them', (await gus.req('GET', `/drive/nodes/${doc.id}/download`, undefined, true) as Response).status === 200);
  r = await gus.post('/drive/files', { file_id: await up(gus, 'notes.txt', 'from outside'), name: `notes-${stamp}.txt`, space: 'group', project_id: group.id });
  ok('…and adds files there', r.status === 200, r.data);
  const blocked = [
    ['GET', '/drive'], ['GET', '/drive?space=company'], ['GET', '/drive?space=personal'], ['GET', `/drive?space=group&project_id=${other.id}`],
    ['GET', `/drive/nodes/${secret.id}`], ['GET', `/drive/nodes/${company.id}/download`], ['GET', `/drive/nodes/${open.id}`], ['GET', '/drive/search?q=company'],
    ['POST', '/drive/files', { file_id: '00000000-0000-4000-8000-000000000000', name: 'x', space: 'company' }],
    ['POST', '/drive/folders', { name: `x ${stamp}`, space: 'personal' }],
    ['POST', `/drive/nodes/${doc.id}/links`, {}], ['PUT', `/drive/nodes/${doc.id}/shares`, []], ['GET', '/drive/trash'],
  ] as [string, string, unknown?][];
  const leaks: string[] = [];
  for (const [m, p, b] of blocked) {
    const x = (await gus.req(m, p, b)) as any;
    if (x.status < 400) leaks.push(`${m} ${p} → ${x.status}`);
  }
  ok('nothing else of the drive: company and personal files, open groups, search, links, shares', leaks.length === 0, leaks);
  const st = (await gus.get('/office/status')).data;
  if (st.editing) {
    r = await gus.post(`/drive/nodes/${doc.id}/office`, {});
    ok('the guest opens the workgroup\'s document in the editor', r.status === 200 && r.data.can_edit === true, r.data);
    const info = await fetch(`http://localhost:8080/api/wopi/files/${doc.id}?access_token=${encodeURIComponent(r.data.token)}`).then((x) => x.json());
    ok('…as themselves, able to write', info.UserCanWrite === true && info.UserFriendlyName === `Gus Out${stamp}`, info);
    ok('…but not someone\'s own file', (await gus.post(`/drive/nodes/${secret.id}/office`, {})).status === 404);
  } else console.log('skip editing: no COLLABORA_URL');

  // ---- restoring a backup from the app ----
  const bk = await admin.get('/admin/backups');
  if (!bk.data.restore_from_app || !bk.data.scripts) console.log('skip restore: BACKUP_RESTORE or the scripts are not set up');
  else {
    const before = (bk.data.items[0]?.name ?? '') as string;
    await admin.post('/admin/backups');
    const made = await waitFor(async () => {
      const x = (await admin.get('/admin/backups')).data;
      return !x.running && x.items[0]?.name !== before ? x.items[0].name : null;
    }, 120000);
    ok('a fresh backup', !!made);
    const marker = (await admin.post('/tasks', { title: `After the backup ${stamp}` })).data;
    ok('only system administrators restore', (await ann.post(`/admin/backups/${made}/restore`, { password: 'password123', confirm: made })).status === 403);
    ok('the backup\'s name must be typed', (await admin.post(`/admin/backups/${made}/restore`, { password: 'supersecret1', confirm: 'yes' })).status === 400);
    ok('the password must be right', (await admin.post(`/admin/backups/${made}/restore`, { password: 'wrong', confirm: made })).status === 400);
    ok('unknown backups are not found', (await admin.post('/admin/backups/2020-01-01T000000Z/restore', { password: 'supersecret1', confirm: '2020-01-01T000000Z' })).status === 404);
    r = await admin.post(`/admin/backups/${made}/restore`, { password: 'supersecret1', confirm: made });
    ok('the restore starts', r.status === 200 && r.data.started, r.data);
    const paused = await waitFor(async () => (await fetch('http://localhost:8080/api/health').then((x) => x.json())).status === 'restoring', 5000);
    const busy = paused ? await fetch('http://localhost:8080/api/tasks', { headers: { authorization: `Bearer ${admin.token}` } }) : null;
    ok('the app pauses while it restores', !!paused && busy?.status === 503, paused);
    const back = await waitFor(async () => (await fetch('http://localhost:8080/api/health').then((x) => x.json())).status === 'ok', 240000);
    ok('and comes back', !!back);
    await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
    const last = (await admin.get('/admin/backups')).data.last_restore;
    ok('the restore succeeded', last?.ok === true && last.backup === made, last);
    ok('the data is the backup\'s: what came after it is gone', (await admin.get(`/tasks/${marker.id}`)).status === 404);
    ok('people who existed then still sign in', (await new Client().post('/auth/login', { email: `ann.${stamp}@acme.test`, password: 'password123' })).status === 200);
  }
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
