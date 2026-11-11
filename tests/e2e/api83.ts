// Duplicate search over a whole record type: groups of records sharing a phone, an email or a
// name (among those the person may read), "not duplicates" hiding a group until another record
// joins it, and merging several groups at once into the oldest (or chosen) record.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const digits = String(Date.now()).slice(-7);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Dp${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const made: string[] = [];
const contact = async (who: Client, first: string, last: string, mf: any[]) => {
  const r = await who.post('/crm/contact/items', { first_name: first, last_name: last, assigned_to: who.id, multifields: mf });
  if (r.status !== 200) throw new Error(JSON.stringify(r.data));
  made.push(r.data.id);
  return r.data.id as string;
};
const groups = async (c: Client, by: string, q: string) => (await c.get(`/crm/contact/duplicate-groups?by=${by}&q=${encodeURIComponent(q)}`)).data;

try {
  const ann = await mk('Ann'), bob = await mk('Bob');
  const phone = `+1 555 ${digits}`, email = `twin.${stamp}@example.test`;
  const a1 = await contact(ann, 'Tom', `Twin${stamp}`, [{ kind: 'phone', value: phone }]);
  const a2 = await contact(ann, 'Thomas', `Other${stamp}`, [{ kind: 'phone', value: phone.replace(/ /g, '-') }, { kind: 'email', value: email }]);
  const a3 = await contact(bob, 'Tom', `Twin${stamp}`, [{ kind: 'email', value: email.toUpperCase() }]);
  const lone = await contact(ann, 'Lone', `Single${stamp}`, [{ kind: 'phone', value: `+1 444 ${digits}` }]);

  let g = await groups(admin, 'phone', digits);
  const byPhone = g.groups.find((x: any) => x.kind === 'phone');
  ok('records sharing a phone (written differently) form a group', byPhone && byPhone.count === 2 && byPhone.items.map((i: any) => i.id).sort().join() === [a1, a2].sort().join(), g);
  ok('…oldest first', byPhone?.items[0].id === a1);
  ok('a record alone isn\'t a group', !g.groups.some((x: any) => x.items.some((i: any) => i.id === lone)));
  g = await groups(admin, 'email', `twin.${stamp}`);
  ok('same email, any case', g.groups.length === 1 && g.groups[0].count === 2 && g.groups[0].items.some((i: any) => i.id === a3), g);
  g = await groups(admin, 'name', `twin${stamp}`);
  ok('same name', g.groups.length === 1 && g.groups[0].items.map((i: any) => i.id).sort().join() === [a1, a3].sort().join(), g);
  g = await groups(admin, 'phone,email,name', stamp);
  ok('all three at once', g.groups.length >= 2 && g.total >= 2, g.total);
  ok('unknown criteria are refused', (await admin.get('/crm/contact/duplicate-groups?by=fax')).status === 400);

  // With employees reading only their own contacts, Bob sees no group made with Ann's.
  const emp = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
  const perms = (await admin.get('/crm/permissions')).data;
  const original = Object.entries(perms.matrix[emp.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
  try {
    await admin.put(`/crm/permissions/${emp.id}`, { entries: original.map((e) => (e.entity_type === 3 && ['read', 'write', 'delete'].includes(e.action) ? { ...e, scope: 'own' } : e)) });
    ok('only records the person may read are grouped', (await groups(bob, 'email,name', stamp)).groups.length === 0);
    ok('Ann sees her own pair', (await groups(ann, 'phone', digits)).groups.length === 1);
    let r = await bob.post('/crm/contact/duplicate-groups/ignore', { kind: 'phone', value: byPhone.value, item_ids: [a1, a2] });
    ok('…and can\'t dismiss groups of others\' records', r.status === 403, r.data);
    r = await bob.post('/crm/contact/duplicate-groups/merge', { groups: [{ item_ids: [a1, a3] }] });
    ok('…nor merge them', r.status === 200 && r.data.merged.length === 0 && r.data.failed.length === 1, r.data);
  } finally {
    await admin.put(`/crm/permissions/${emp.id}`, { entries: original });
  }

  // Not duplicates.
  let r = await admin.post('/crm/contact/duplicate-groups/ignore', { kind: 'phone', value: byPhone.value, item_ids: [a1, a2] });
  ok('"not duplicates"', r.status === 200, r.data);
  ok('…hides the group', (await groups(admin, 'phone', digits)).groups.length === 0);
  const a4 = await contact(ann, 'Tim', `Third${stamp}`, [{ kind: 'phone', value: phone }]);
  g = await groups(admin, 'phone', digits);
  ok('…until another record joins it', g.groups.length === 1 && g.groups[0].count === 3, g);

  // Merging groups at once.
  const nameGroup = (await groups(admin, 'name', `twin${stamp}`)).groups[0];
  r = await admin.post('/crm/contact/duplicate-groups/merge', { groups: [{ item_ids: nameGroup.items.map((i: any) => i.id) }, { item_ids: [a2, a4], target_id: a4 }] });
  ok('several groups merge in one go', r.status === 200 && r.data.merged.length === 2 && r.data.failed.length === 0, r.data);
  ok('…each into its oldest record', r.data.merged.includes(a1) && (await admin.get(`/crm/contact/items/${a3}`)).status === 404);
  const kept = (await admin.get(`/crm/contact/items/${a1}`)).data;
  ok('…which takes in the others\' details', kept.multifields.some((m: any) => m.kind === 'email' && m.value.toLowerCase() === email), kept.multifields);
  ok('…or into the record chosen', r.data.merged.includes(a4) && (await admin.get(`/crm/contact/items/${a2}`)).status === 404);
  r = await admin.post('/crm/contact/duplicate-groups/merge', { groups: [{ item_ids: [a1, a3] }] });
  ok('a group already merged is reported, not an error', r.status === 200 && r.data.failed.length === 1, r.data);
  // The two kept records still share a phone and an email, so they're found again.
  const left = (await groups(admin, 'phone,email,name', stamp)).groups.concat((await groups(admin, 'phone', digits)).groups);
  ok('what\'s left: the two records kept, which still look alike', left.length > 0 && left.every((x: any) => x.items.map((i: any) => i.id).sort().join() === [a1, a4].sort().join()), left);
} finally {
  for (const id of made) await admin.del(`/crm/contact/items/${id}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
