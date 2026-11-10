// Record card layouts: defaults, personal and shared layouts, validation, new and deleted
// custom fields.
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client(); await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const email = `card.${stamp}@acme.test`;
await admin.post('/users', { email, password: 'password123', first_name: 'Cara', last_name: 'Card' });
const sam = new Client(); await sam.post('/auth/login', { email, password: 'password123' });
// Start from a clean slate for deals.
await admin.del('/crm/deal/card-layout?scope=common'); await admin.del('/crm/deal/card-layout?scope=mine');

let r: any = await admin.get('/crm/deal/card-layout');
const titles = (l) => l.sections.map((s) => s.title);
ok('deal default layout', r.status === 200 && r.data.scope === 'default' && titles(r.data.layout).join() === 'About,Comment' && r.data.layout.sections[0].fields.includes('close_date') && r.data.layout.sections[0].fields.includes('probability'), r.data.layout);
ok('field catalogue with labels', r.data.fields.some((f) => f.code === 'close_date' && f.label === 'Expected close date') && r.data.fields.some((f) => f.code === 'utm') && !r.data.fields.some((f) => f.code === 'contact_details'), r.data.fields);
ok('admin may edit the shared card', r.data.can_edit_common === true);
r = await admin.get('/crm/company/card-layout');
ok('company default has contact details', titles(r.data.layout).join() === 'About,Contact details,Comment' && r.data.layout.sections[1].fields.join() === 'contact_details,address' && r.data.layout.sections[0].fields.includes('industry'), r.data.layout);
r = await admin.get('/crm/invoice/card-layout');
ok('invoice uses its due date label', r.data.fields.find((f) => f.code === 'close_date')?.label === 'Due date', r.data.fields);

// ---- shared layout ----
const shared = { sections: [{ id: 'money', title: 'Money', fields: ['probability', 'close_date'] }, { id: 'origin', title: 'Origin', fields: ['source', 'source_details'] }], hidden: ['utm'], hide_empty: true };
ok('employee cannot set the shared card', (await sam.put('/crm/deal/card-layout', { layout: shared, scope: 'common' })).status === 403);
r = await admin.put('/crm/deal/card-layout', { layout: shared, scope: 'common' });
ok('admin sets the shared card', r.status === 200 && r.data.scope === 'common' && titles(r.data.layout).join() === 'Money,Origin' && r.data.layout.hide_empty === true && r.data.layout.hidden.join() === 'utm', r.data);
ok('unplaced fields go to the last section', r.data.layout.sections[1].fields.join() === 'source,source_details,comments', r.data.layout.sections[1]);
r = await sam.get('/crm/deal/card-layout');
ok('everyone gets it', r.data.scope === 'common' && titles(r.data.layout).join() === 'Money,Origin' && r.data.can_edit_common === false, r.data);

// ---- personal layout ----
const mine = { sections: [{ title: 'Mine', fields: ['source'] }], hidden: ['probability', 'utm'], hide_empty: false };
r = await sam.put('/crm/deal/card-layout', { layout: mine });
ok('employee keeps a personal card', r.status === 200 && r.data.scope === 'mine' && r.data.has_common && titles(r.data.layout).join() === 'Mine' && r.data.layout.sections[0].id === 's0', r.data);
ok('others still get the shared card', (await admin.get('/crm/deal/card-layout')).data.scope === 'common');
r = await sam.del('/crm/deal/card-layout?scope=mine');
ok('back to the shared card', r.data.scope === 'common' && titles(r.data.layout).join() === 'Money,Origin');
ok('employee cannot reset the shared card', (await sam.del('/crm/deal/card-layout?scope=common')).status === 403);

// ---- validation ----
const bad = async (layout) => (await admin.put('/crm/deal/card-layout', { layout, scope: 'common' })).status;
ok('unknown field rejected', (await bad({ sections: [{ title: 'A', fields: ['nope'] }] })) === 400);
ok('field placed twice rejected', (await bad({ sections: [{ title: 'A', fields: ['source'] }, { title: 'B', fields: ['source'] }] })) === 400);
ok('untitled section rejected', (await bad({ sections: [{ title: ' ', fields: [] }] })) === 400);
ok('no sections rejected', (await bad({ sections: [] })) === 400);
ok('bad scope rejected', (await admin.put('/crm/deal/card-layout', { layout: shared, scope: 'team' })).status === 400);
ok('identity fields are not card fields', (await bad({ sections: [{ title: 'A', fields: ['title'] }] })) === 400);

// ---- custom fields come and go ----
const fd = (await admin.post('/crm/deal/fields', { label: `Tier ${stamp}`, kind: 'string', is_required: true })).data;
r = await admin.get('/crm/deal/card-layout');
const code = `uf.${fd.code}`;
ok('a new field appears at the end', r.data.layout.sections.at(-1).fields.at(-1) === code, r.data.layout);
ok('a required field cannot be hidden', (await bad({ sections: [{ title: 'A', fields: [] }], hidden: [code] })) === 400);
r = await admin.put('/crm/deal/card-layout', { layout: { sections: [{ title: 'Top', fields: [code, 'close_date'] }], hidden: [] }, scope: 'common' });
ok('custom fields can be placed anywhere', r.data.layout.sections[0].fields.slice(0, 2).join() === `${code},close_date`, r.data.layout);
await admin.del(`/crm/deal/fields/${fd.id}`);
r = await admin.get('/crm/deal/card-layout');
ok('a deleted field drops out of the layout', r.status === 200 && !r.data.layout.sections.some((s) => s.fields.includes(code)), r.data.layout);

// ---- reset ----
r = await admin.del('/crm/deal/card-layout?scope=common');
ok('reset to the default', r.data.scope === 'default' && titles(r.data.layout).join() === 'About,Comment');
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
