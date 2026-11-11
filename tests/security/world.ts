// The cast for the per-endpoint sweeps: Ann owns private things in most modules, Bob is an
// ordinary colleague who attacks them, Cat is the one person Ann shares some with, and Gus
// is a guest from outside the company invited into one chat. Every name carries the run's
// stamp, so the usual cleanup removes what is made here.
import { admin, person, stamp } from './lib.ts';
import { Client } from '../e2e/lib.ts';

export type Person = Client & { id: string; email: string };

export async function world() {
  const ann = await person('Ann', ['bi.manage', 'kb.manage', 'lists.manage']);
  const bob = await person('Bob');
  const cat = await person('Cat');
  const up = async (c: Client, name: string, text: string) => { const f = new FormData(); f.append('file', new Blob([text], { type: 'image/png' }), name); return (await c.post('/files', f)).data?.id as string; };
  const start = new Date(Date.now() + 3 * 86400e3);
  const end = new Date(+start + 3600e3);
  const ids: Record<string, string | undefined> = {};
  const keep = async (key: string, f: () => Promise<any>, pick: (d: any) => string | undefined = (d) => d?.id) => {
    try { ids[key] = pick(await f()); } catch { ids[key] = undefined; }
  };

  // ---- tasks, files, drive ----
  await keep('file', async () => ({ id: await up(ann, `ann-${stamp}.png`, 'ann private') }));
  await keep('task', async () => (await ann.post('/tasks', { title: `Ann private task ${stamp}` })).data);
  await keep('driveNode', async () => (await ann.post('/drive/files', { file_id: await up(ann, `drive-${stamp}.txt`, 'drive secret'), name: `drive-${stamp}.txt` })).data);
  await keep('driveFolder', async () => (await ann.post('/drive/folders', { name: `Ann folder ${stamp}`, space: 'personal' })).data);
  await keep('driveLink', async () => (await ann.post(`/drive/nodes/${ids.driveNode}/links`, {})).data, (d) => (d.links ?? []).at(-1)?.id);
  // ---- messenger, feed ----
  await keep('chat', async () => (await ann.post('/im/chats', { title: `Ann and Cat ${stamp}`, members: [cat.id] })).data);
  await keep('message', async () => (await ann.post(`/im/chats/${ids.chat}/messages`, { body: `just between us ${stamp}` })).data, (d) => d?.id ?? d?.message?.id);
  await keep('catPost', async () => (await ann.post('/feed', { body: `Only for Cat ${stamp}`, to_all: false, users: [cat.id] })).data);
  await keep('comment', async () => (await ann.post(`/feed/${ids.catPost}/comments`, { body: `private comment ${stamp}` })).data, (d) => d?.id ?? d?.comment?.id);
  // ---- workgroups ----
  await keep('group', async () => (await ann.post('/projects', { name: `Ann vault ${stamp}`, kind: 'group', visibility: 'private', member_ids: [cat.id] })).data);
  await keep('groupPost', async () => (await ann.post('/feed', { body: `Vault plan ${stamp}`, project_id: ids.group })).data);
  await keep('groupTask', async () => (await ann.post('/tasks', { title: `Vault task ${stamp}`, project_id: ids.group })).data);
  await keep('scrum', async () => (await ann.post('/projects', { name: `Ann scrum ${stamp}`, kind: 'scrum', visibility: 'private', member_ids: [cat.id] })).data);
  // ---- calendar, meetings, help desk ----
  await keep('event', async () => (await ann.post('/calendar/events', { title: `Ann private event ${stamp}`, timezone: 'UTC', start: start.toISOString(), end: end.toISOString(), is_private: true })).data);
  await keep('meeting', async () => (await ann.post('/meetings', { title: `Ann meeting ${stamp}`, starts_at: start.toISOString(), duration_minutes: 30, timezone: 'UTC', members: [{ user_id: cat.id }] })).data);
  await keep('agenda', async () => (await ann.post(`/meetings/${ids.meeting}/agenda`, { title: `Ann topic ${stamp}`, responsible_ids: [cat.id] })).data, (d) => d?.agenda?.[0]?.id);
  await keep('ticket', async () => (await ann.post('/support/tickets', { title: `Ann ticket ${stamp}`, body: 'help' })).data);
  // ---- boards, BI, knowledge base, lists ----
  await keep('board', async () => (await ann.post('/boards', { title: `Ann board ${stamp}` })).data);
  await keep('boardLink', async () => (await ann.post(`/boards/${ids.board}/links`, { level: 'view' })).data, (d) => d?.id ?? d?.links?.at?.(-1)?.id);
  await keep('dash', async () => (await ann.post('/bi/dashboards', { title: `Ann dash ${stamp}` })).data);
  await keep('kbSpace', async () => (await ann.post('/kb/spaces', { name: `Ann space ${stamp}`, to_all: false, readers: { users: [cat.id] } })).data);
  await keep('kbPage', async () => (await ann.post(`/kb/spaces/${ids.kbSpace}/pages`, { title: `Ann page ${stamp}`, body: 'secret handbook' })).data);
  await keep('list', async () => (await ann.post('/lists', { name: `Ann list ${stamp}`, to_all: false, access: { read: { users: [cat.id] } }, fields: [{ label: 'Amount', kind: 'number' }] })).data);
  await keep('listItem', async () => (await ann.post(`/lists/${ids.list}/items`, { title: `Ann item ${stamp}`, values: {} })).data);
  // ---- account things ----
  await keep('appPassword', async () => (await ann.post('/auth/app-passwords', { name: `Ann phone ${stamp}` })).data);
  await keep('session', async () => (await ann.get('/auth/sessions')).data, (d) => (d.items ?? d)?.[0]?.id);
  await keep('absence', async () => (await ann.post('/hr/absences', { kind: 'vacation', starts_on: '2027-05-03', ends_on: '2027-05-04' })).data);
  await keep('report', async () => (await ann.get('/hr/reports/current')).data, (d) => d?.id ?? d?.report?.id);

  const owned = Object.fromEntries(Object.entries(ids).filter(([, v]) => v)) as Record<string, string>;
  return { admin, ann, bob, cat, ids: owned, missing: Object.keys(ids).filter((k) => !ids[k]) };
}

/** A guest from outside the company, invited by Ann into one chat, signed in. */
export async function guest(ann: Person, chatWith: string[]) {
  const chat = (await ann.post('/im/chats', { title: `Partners ${stamp}`, members: chatWith })).data;
  const email = `guest.${stamp}@outside.test`;
  await ann.post(`/im/chats/${chat.id}/guests`, { email, first_name: 'Gus', last_name: `Out${stamp}` });
  let token: string | undefined;
  for (let i = 0; i < 40 && !token; i++) {
    const m = (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent('to:' + email)}&limit=5`)).json()).messages?.[0];
    if (m) token = (await (await fetch(`http://localhost:8026/api/v1/message/${m.ID}`)).json()).Text.match(/accept-invite\?token=([\w-]+)/)?.[1];
    else await new Promise((r) => setTimeout(r, 250));
  }
  const g = new Client() as Person;
  await g.post('/auth/accept-invite', { token, password: 'guestpass123' });
  await g.post('/auth/login', { email, password: 'guestpass123' });
  g.email = email;
  return { guest: g, chat };
}
