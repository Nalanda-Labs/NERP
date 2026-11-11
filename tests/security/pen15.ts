// A guest from outside the company, signed in, against every endpoint: once with no ids
// filled in and once with each of Ann's private things. Outside the guest's own areas
// (their chats, workgroups, tasks, posts and boards shared with them, their profile)
// nothing may answer, and nothing of the company may show in what does.
import { ok, check, finish, counts, stamp } from './lib.ts';
import { world, guest as makeGuest } from './world.ts';
import { endpoints, isPublic, SELF_HARM, fillPath, params, reach, ok2xx, NIL } from './sweep.ts';

const { ann, bob, ids } = await world();
const { guest, chat } = await makeGuest(ann, [bob.id]);
ok('the guest is signed in', !!guest.token);
// Their workgroups' files are theirs too (the drive keeps them to those).
const AREAS = /^\/api\/(auth\/|me\/|profile|notifications|realtime\/|files|download-links|im\/|calls|projects|tasks|feed|boards|drive|office\/status|settings$|users\/me)/;
const SECRETS = [`Ann private task ${stamp}`, `just between us ${stamp}`, `Only for Cat ${stamp}`, `Vault plan ${stamp}`, `Ann private event ${stamp}`, `Ann board ${stamp}`, `Ann dash ${stamp}`, 'secret handbook', `Ann item ${stamp}`, ann.email, bob.email];
const values = [NIL, ...Object.values(ids), chat.id, ann.id];
let swept = 0;
for (const e of endpoints()) {
  if (isPublic(e.path) || SELF_HARM.test(e.path)) continue;
  swept++;
  const k = params(e.path);
  const tries = k === 0 ? [e.path] : values.map((v) => fillPath(e.path, [v, v]));
  const answered: string[] = [], leaked: string[] = [];
  for (const path of tries) {
    const r = await reach(e.method, path, guest.token, { title: `guest ${stamp}`, body: `guest ${stamp}`, name: `guest ${stamp}` });
    if (ok2xx(r.status) && !AREAS.test(e.path)) answered.push(`${path.slice(0, 80)} → ${r.status}`);
    const hit = ok2xx(r.status) ? SECRETS.find((x) => r.text.includes(x)) : undefined;
    if (hit) leaked.push(`${path.slice(0, 80)} shows “${hit}”`);
  }
  check(`${e.method} ${e.path}: nothing for a guest outside their areas`, answered.length === 0, answered.slice(0, 3));
  check(`${e.method} ${e.path}: nothing of the company shown to a guest`, leaked.length === 0, leaked.slice(0, 3));
}
ok(`(${swept} endpoints tried by a guest)`, swept > 800);
console.log(`checks: ${JSON.stringify(counts())}`);
await finish();
