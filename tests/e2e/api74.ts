// Ready-made business processes (lists with an approval workflow), stages that ask for fields
// before records enter them, services in the product catalogue, and segments imported from
// a list of email addresses.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
admin.id = (await admin.get('/auth/me')).data.id;
const ann = new Client();
{
  const email = `ann.${stamp}@acme.test`;
  ann.id = (await admin.post('/users', { email, password: 'password123', first_name: 'Ann', last_name: `Qx${stamp}` })).data.id;
  await ann.post('/auth/login', { email, password: 'password123' });
}

try {
  // ---- ready-made solutions ----
  let r = await admin.get('/lists/solutions');
  const keys = r.data.items.map((s: any) => s.key);
  ok('a gallery of ready-made processes', ['expenses', 'trips', 'invoices', 'outgoing', 'hiring', 'purchases'].every((k) => keys.includes(k)), keys);
  ok('setting one up needs the lists permission', (await ann.post('/lists/solutions/expenses', {})).status === 403);
  ok('an unknown one is not found', (await admin.post('/lists/solutions/nope', {})).status === 404);
  r = await admin.post('/lists/solutions/expenses', { name: `Expenses ${stamp}`, finance: [admin.id] });
  ok('sets up a list', r.status === 200 && r.data.name === `Expenses ${stamp}`, r.data);
  const list = (await admin.get(`/lists/${r.data.id}`)).data;
  ok('with its fields', list.fields?.length >= 2, list.fields);
  const wf = (await admin.get(`/lists/${r.data.id}/workflows`)).data;
  const flows = wf.items ?? wf;
  ok('and an approval process that starts on new items', Array.isArray(flows) && flows.some((w: any) => w.trigger === 'create' && w.is_active), wf);
  for (const k of ['trips', 'invoices', 'outgoing', 'hiring', 'purchases']) {
    const x = await admin.post(`/lists/solutions/${k}`, { name: `${k} ${stamp}` });
    ok(`"${k}" sets up too`, x.status === 200, x.data);
  }

  // ---- required fields per stage ----
  const cfg = (await admin.get('/crm/config')).data;
  r = await admin.post('/crm/deal/categories', { name: `Rules ${stamp}` });
  const cat = r.data.categories.find((c: any) => c.name === `Rules ${stamp}`);
  const [first, second] = cat.stages.filter((s: any) => s.semantics === 'process');
  ok('unknown fields are refused', (await admin.patch(`/crm/stages/${second.id}`, { required_fields: ['nonsense'] })).status === 400);
  r = await admin.patch(`/crm/stages/${second.id}`, { required_fields: ['opportunity', 'close_date'] });
  ok('a stage asks for fields', r.status === 200, r.data);
  const fresh = (await admin.get('/crm/config')).data.types.find((t: any) => t.code === 'deal').categories.find((c: any) => c.id === cat.id).stages.find((s: any) => s.id === second.id);
  ok('and says so in the configuration', JSON.stringify(fresh.required_fields) === JSON.stringify(['opportunity', 'close_date']), fresh);
  const deal = (await admin.post('/crm/deal/items', { title: `Rule deal ${stamp}`, category_id: cat.id, stage_id: first.id, currency: cfg.base_currency })).data;
  r = await admin.patch(`/crm/deal/items/${deal.id}`, { stage_id: second.id });
  ok('a record without them stays where it is', r.status === 400 && /Amount/.test(r.data.error?.message ?? '') && /Expected close date/.test(r.data.error?.message ?? ''), r.data);
  r = await admin.patch(`/crm/deal/items/${deal.id}`, { stage_id: second.id, opportunity: '1500' });
  ok('one missing is still too few', r.status === 400 && !/Amount/.test(r.data.error?.message ?? ''), r.data);
  r = await admin.patch(`/crm/deal/items/${deal.id}`, { stage_id: second.id, opportunity: '1500', close_date: '2027-01-31' });
  ok('filled in, it moves', r.status === 200 && r.data.stage_id === second.id, r.data);
  r = await admin.patch(`/crm/deal/items/${deal.id}`, { title: `Rule deal ${stamp} renamed` });
  ok('other edits in that stage are not affected', r.status === 200);
  const other = (await admin.post('/crm/deal/items', { title: `Rule deal 2 ${stamp}`, category_id: cat.id, stage_id: first.id, currency: cfg.base_currency })).data;
  await admin.patch(`/crm/stages/${second.id}`, { required_fields: [] });
  ok('without the rule anything moves', (await admin.patch(`/crm/deal/items/${other.id}`, { stage_id: second.id })).status === 200);

  // ---- services ----
  r = await admin.post('/crm/products', { name: `Consulting ${stamp}`, price: '120', currency: cfg.base_currency, is_service: true, stock: '5' });
  ok('a product can be a service', r.status === 200 && r.data.is_service === true && r.data.stock === null, r.data);
  r = await admin.post('/crm/products', { name: `Widget ${stamp}`, price: '10', currency: cfg.base_currency });
  ok('products are goods by default', r.data.is_service === false, r.data);
  const widget = r.data;
  r = await admin.patch(`/crm/products/${widget.id}`, { name: widget.name, price: widget.price, currency: widget.currency, is_service: true });
  ok('a product without stock can become one', r.status === 200 && r.data.is_service === true, r.data);

  // ---- imported segments ----
  const mails = [`known.${stamp}@example.com`, `new.${stamp}@example.com`];
  const known = (await admin.post('/crm/contact/items', { first_name: 'Known', last_name: `Imp${stamp}`, multifields: [{ kind: 'email', value: mails[0].toUpperCase() }] })).data;
  ok('importing needs the marketing permission', (await ann.post('/marketing/segments/import', { name: `Imp ${stamp}`, rows: [{ email: mails[0] }] })).status === 403);
  r = await admin.post('/marketing/segments/import', {
    name: `Fair visitors ${stamp}`, create_missing: false,
    rows: [{ email: mails[0] }, { email: mails[0] }, { email: mails[1], first_name: 'Nova', last_name: `Imp${stamp}` }, { email: 'not an address' }],
  });
  ok('matches known addresses, leaves out the rest', r.status === 200 && r.data.imported === true && r.data.count === 1 && r.data.import.matched === 1 && r.data.import.not_found === 1 && r.data.import.invalid === 1, r.data);
  const seg1 = r.data;
  r = await admin.post('/marketing/segments/import', {
    name: `Fair visitors all ${stamp}`, create_missing: true,
    rows: [{ email: mails[0] }, { email: mails[1], first_name: 'Nova', last_name: `Imp${stamp}` }],
  });
  ok('or adds them as new contacts', r.status === 200 && r.data.count === 2 && r.data.import.created === 1 && r.data.reachable === 2, r.data);
  const found = (await admin.get(`/crm/contact/items?q=${encodeURIComponent(mails[1])}`)).data;
  ok('the new contact is in the CRM with its address', (found.items ?? found).some((i: any) => i.title?.includes('Nova')), found);
  await admin.post('/crm/contact/items', { first_name: 'Later', last_name: `Imp${stamp}`, multifields: [{ kind: 'email', value: `later.${stamp}@example.com` }] });
  ok('an imported segment does not grow by itself', (await admin.get(`/marketing/segments/${seg1.id}`)).data.count === 1);
  ok('its records cannot be filtered differently', (await admin.patch(`/marketing/segments/${seg1.id}`, { filter: { q: 'x' } })).status === 400);
  r = await admin.patch(`/marketing/segments/${seg1.id}`, { name: `Fair visitors ${stamp} (renamed)` });
  ok('but it can be renamed', r.status === 200 && r.data.count === 1 && r.data.imported === true, r.data);
  // a campaign to it reaches exactly those records
  const camp = (await admin.post('/marketing/campaigns', { name: `Imp camp ${stamp}` })).data;
  await admin.patch(`/marketing/campaigns/${camp.id}`, { segment_ids: [seg1.id], subject: 'Hi {{first_name}}', body: 'Hello {{first_name}}' });
  r = await admin.post(`/marketing/campaigns/${camp.id}/test`, { email: `test.${stamp}@example.com` });
  ok('a test send uses a record of the imported list', r.status === 200, r.data);
  void known;
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
