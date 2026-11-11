// Reaching a colleague's things, endpoint by endpoint and thing by thing. Ann owns private
// things in most modules (see world.ts); Bob, an ordinary colleague, calls every endpoint
// that takes ids, with every method, once per thing of Ann's (for endpoints with two ids,
// every pair), with a body completed until the handler answers. Each (endpoint, thing) is a
// check: nothing may succeed. Then everything of Ann's must be as she left it.
import { ok, check, finish, counts, stamp } from './lib.ts';
import { world } from './world.ts';
import { endpoints, SELF_HARM, call, fillPath, params, reach, ok2xx } from './sweep.ts';

const { ann, bob, ids, missing } = await world();
ok(`Ann has private things in most modules (${Object.keys(ids).length})`, Object.keys(ids).length >= 24, missing);
// Endpoints any colleague may call with any id: profiles and their badges (people, not
// things), saved CRM filters (per person: someone else's id finds nothing and says ok).
const ALLOWED = /^\/api\/users\/\{[^}]+\}(\/badges)?$|^\/api\/crm\/filters\/\{id\}$/;
const values = Object.entries(ids);
const start = { title: `pwned by bob ${stamp}`, name: `pwned by bob ${stamp}`, body: `pwned by bob ${stamp}`, members: [bob.id], member_ids: [bob.id], user_id: bob.id, level: 'edit', role: 'owner' };
let swept = 0;
for (const e of endpoints()) {
  const k = params(e.path);
  if (!k || k > 2 || SELF_HARM.test(e.path) || ALLOWED.test(e.path) || /^\/api\/public\//.test(e.path)) continue;
  swept++;
  for (const [what, id] of values) {
    if (k === 1) {
      const r = await reach(e.method, fillPath(e.path, [id]), bob.token, start);
      check(`${e.method} ${e.path} with Ann's ${what}: refused`, !ok2xx(r.status), `${r.status} ${r.text.slice(0, 140)}`);
    } else {
      const hits: string[] = [];
      for (const [w2, id2] of values) {
        const r = await call(e.method, fillPath(e.path, [id, id2]), { token: bob.token, body: e.method === 'GET' || e.method === 'DELETE' ? undefined : JSON.stringify(start) });
        if (ok2xx(r.status)) hits.push(`+${w2} → ${r.status} ${r.text.slice(0, 80)}`);
      }
      check(`${e.method} ${e.path} with Ann's ${what} (and each other thing): refused`, hits.length === 0, hits);
    }
  }
}
ok(`(${swept} endpoints with ids × ${values.length} things of Ann's)`, swept > 400);

// ---- everything of Ann's is as she left it ----
const as = (p: string) => ann.get(p).then((r: any) => r);
const intact: [string, () => Promise<boolean>][] = [
  ['task', async () => (await as(`/tasks/${ids.task}`)).data?.title === `Ann private task ${stamp}`],
  ['group task', async () => (await as(`/tasks/${ids.groupTask}`)).data?.title === `Vault task ${stamp}`],
  ['drive file', async () => (await (await ann.req('GET', `/drive/nodes/${ids.driveNode}/download`, undefined, true) as Response).text()) === 'drive secret'],
  ['file', async () => (await ann.req('GET', `/files/${ids.file}/content`, undefined, true) as Response).status === 200],
  ['chat', async () => !JSON.stringify((await as(`/im/chats/${ids.chat}`)).data).includes(bob.id) && JSON.stringify((await as(`/im/chats/${ids.chat}/messages`)).data).includes(`just between us ${stamp}`)],
  ['private group', async () => !JSON.stringify((await as(`/projects/${ids.group}`)).data?.members ?? []).includes(bob.id)],
  ['event', async () => (await as(`/calendar/events/${ids.event}`)).data?.title === `Ann private event ${stamp}`],
  ['meeting', async () => (await as(`/meetings/${ids.meeting}`)).data?.title === `Ann meeting ${stamp}`],
  ['ticket', async () => (await as(`/support/tickets/${ids.ticket}`)).data?.title === `Ann ticket ${stamp}` || (await as(`/support/tickets/${ids.ticket}`)).data?.ticket?.title === `Ann ticket ${stamp}`],
  ['board', async () => (await as(`/boards/${ids.board}`)).data?.title === `Ann board ${stamp}`],
  ['dashboard', async () => (await as(`/bi/dashboards/${ids.dash}`)).data?.title === `Ann dash ${stamp}` || (await as(`/bi/dashboards/${ids.dash}`)).data?.dashboard?.title === `Ann dash ${stamp}`],
  ['knowledge base page', async () => (await as(`/kb/pages/${ids.kbPage}`)).data?.body === 'secret handbook'],
  ['list item', async () => JSON.stringify((await as(`/lists/items/${ids.listItem}`)).data).includes(`Ann item ${stamp}`)],
  ['post for Cat', async () => JSON.stringify((await as('/feed')).data).includes(`Only for Cat ${stamp}`)],
  ['app password', async () => ((await as('/auth/app-passwords')).data.items ?? []).some((p: any) => p.id === ids.appPassword)],
  ['session', async () => (await as('/auth/me')).status === 200],
];
for (const [what, test] of intact) {
  let good = false;
  try { good = await test(); } catch { /* counts as changed */ }
  ok(`Ann's ${what} is as she left it`, good);
}
const pwned = JSON.stringify((await bob.get(`/search?q=${encodeURIComponent(`pwned by bob ${stamp}`)}`)).data ?? '');
ok('nothing of Ann\'s carries Bob\'s text', !JSON.stringify((await ann.get(`/search?q=${encodeURIComponent('pwned by bob')}`)).data ?? '').includes(`pwned by bob ${stamp}`) || pwned.length > 0);
console.log(`checks: ${JSON.stringify(counts())}`);
await finish();
