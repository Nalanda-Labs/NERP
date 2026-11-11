// CRM depth: tunnels between pipelines (copy or move on entering a stage), catalog sections
// with their own editors, document templates limited to people and departments, a CRM
// record's chat, Excel and vCard exports, and the company's own traffic sources.
import { inflateRawSync } from 'node:zlib';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const waitFor = async <T>(f: () => Promise<T | null | undefined | false>, ms = 15000): Promise<T | null> => { const end = Date.now() + ms; while (Date.now() < end) { const v = await f(); if (v) return v as T; await sleep(300); } return null; };
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const roles = (await admin.get('/roles')).data;
const user = async (first: string, perms: string[] = []) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Crm${stamp}` })).data;
  if (perms.length) {
    const role = (await admin.post('/roles', { code: `crm_${first.toLowerCase()}_${stamp}`, name: `Crm ${first} ${stamp}`, permissions: perms })).data;
    await admin.put(`/users/${u.id}/roles`, { role_ids: [roles.find((r: any) => r.code === 'employee').id, role.id] });
  }
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
/** The text of a stored entry of a zip (xlsx) file. */
const unzipEntry = (buf: Buffer, name: string) => {
  let i = 0;
  while ((i = buf.indexOf('PK\u0003\u0004', i, 'latin1')) >= 0) {
    const method = buf.readUInt16LE(i + 8), size = buf.readUInt32LE(i + 18), nlen = buf.readUInt16LE(i + 26), xlen = buf.readUInt16LE(i + 28);
    const fname = buf.toString('utf8', i + 30, i + 30 + nlen);
    const start = i + 30 + nlen + xlen;
    if (fname === name) {
      const data = buf.subarray(start, start + (size || buf.length));
      return method === 8 ? inflateRawSync(data).toString('utf8') : data.toString('utf8');
    }
    i = start;
  }
  return '';
};
const [sam, cat, ida] = await Promise.all([user('Sam'), user('Cat'), user('Ida')]);
const made = { categories: [] as string[] };

try {
  // ---- tunnels ----
  const mk = async (name: string) => (await admin.post('/crm/deal/categories', { name })).data.categories.find((c: any) => c.name === name);
  const sales = await mk(`Sales ${stamp}`), delivery = await mk(`Delivery ${stamp}`), service = await mk(`Service ${stamp}`);
  const open = (c: any) => c.stages.filter((s: any) => s.semantics === 'process');
  const [s1, s2] = open(sales), [d1] = open(delivery), [v1] = open(service);
  ok('a tunnel must lead to another pipeline', (await admin.post('/crm/robots', { category_id: sales.id, stage_id: s2.id, action: 'tunnel', params: { stage_id: s1.id } })).status === 400);
  let r = await admin.post('/crm/robots', { category_id: sales.id, stage_id: s2.id, name: 'To delivery', action: 'tunnel', params: { stage_id: d1.id, mode: 'copy' } });
  ok('a copying tunnel', r.status === 200, r.data);
  r = await admin.post('/crm/robots', { category_id: delivery.id, stage_id: open(delivery)[1].id, name: 'To service', action: 'tunnel', params: { stage_id: v1.id, mode: 'move' } });
  ok('a moving tunnel', r.status === 200, r.data);
  const base = (await admin.get('/crm/config')).data.base_currency;
  const deal = (await admin.post('/crm/deal/items', { title: `Order ${stamp}`, category_id: sales.id, stage_id: s1.id, opportunity: '900', currency: base, assigned_to: sam.id })).data;
  await admin.patch(`/crm/deal/items/${deal.id}`, { stage_id: s2.id });
  const copy = await waitFor(async () => ((await admin.get(`/crm/deal/items?category_id=${delivery.id}&q=${stamp}`)).data.items ?? []).find((i: any) => i.title === `Order ${stamp}`));
  ok('entering the stage copies the deal into the other pipeline, with its amount', !!copy && Number((copy as any).opportunity) === 900 && (copy as any).stage_id === d1.id, copy);
  ok('…and the original stays', (await admin.get(`/crm/deal/items/${deal.id}`)).data.category_id === sales.id);
  await admin.patch(`/crm/deal/items/${(copy as any).id}`, { stage_id: open(delivery)[1].id });
  const moved = await waitFor(async () => (await admin.get(`/crm/deal/items/${(copy as any).id}`)).data.category_id === service.id);
  ok('a moving tunnel moves it on', !!moved, (await admin.get(`/crm/deal/items/${(copy as any).id}`)).data);

  // ---- catalog section editors ----
  const shoes = (await admin.post('/shop/categories', { name: `Shoes ${stamp}` })).data.items.find((c: any) => c.name === `Shoes ${stamp}`);
  const hats = (await admin.post('/shop/categories', { name: `Hats ${stamp}` })).data.items.find((c: any) => c.name === `Hats ${stamp}`);
  made.categories.push(shoes.id, hats.id);
  ok('only catalog managers name section editors', (await sam.put(`/crm/catalog/sections/${shoes.id}/editors`, { users: [sam.id] })).status === 403);
  r = await admin.put(`/crm/catalog/sections/${shoes.id}/editors`, { users: [sam.id] });
  ok('Sam edits shoes', r.status === 200 && r.data.users[0]?.id === sam.id, r.data);
  ok('he knows which sections are his', JSON.stringify((await sam.get('/crm/catalog/my-sections')).data) === JSON.stringify({ all: false, sections: [shoes.id] }));
  r = await sam.post('/crm/products', { name: `Boot ${stamp}`, price: '50', currency: base, shop_category_id: shoes.id });
  ok('…adds a product there', r.status === 200, r.data);
  const boot = r.data;
  ok('…but not in another section, or without one', (await sam.post('/crm/products', { name: `Cap ${stamp}`, price: '5', currency: base, shop_category_id: hats.id })).status === 403
    && (await sam.post('/crm/products', { name: `Loose ${stamp}`, price: '5', currency: base })).status === 403);
  ok('…changes his product', (await sam.patch(`/crm/products/${boot.id}`, { name: `Boot ${stamp}`, price: '55', currency: base, shop_category_id: shoes.id })).status === 200);
  ok('…but can\'t move it to a section that isn\'t his', (await sam.patch(`/crm/products/${boot.id}`, { name: `Boot ${stamp}`, price: '55', currency: base, shop_category_id: hats.id })).status === 403);
  const capByAdmin = (await admin.post('/crm/products', { name: `Hat ${stamp}`, price: '9', currency: base, shop_category_id: hats.id })).data;
  ok('…or change or remove products elsewhere', (await sam.patch(`/crm/products/${capByAdmin.id}`, { name: 'x', price: '1', currency: base })).status === 403 && (await sam.del(`/crm/products/${capByAdmin.id}`)).status === 403);
  const team = (await admin.post('/departments', { name: `Hat team ${stamp}` })).data;
  await admin.post(`/departments/${team.id}/members`, { user_id: cat.id });
  await admin.put(`/crm/catalog/sections/${hats.id}/editors`, { departments: [team.id] });
  ok('a department\'s members edit its section', (await cat.patch(`/crm/products/${capByAdmin.id}`, { name: `Hat ${stamp}`, price: '10', currency: base, shop_category_id: hats.id })).status === 200);

  // ---- document templates for chosen people ----
  const tpl = (await admin.get('/crm/doc-templates?type=deal')).data.items.find((t: any) => t.builtin_key === 'typst.invoice');
  r = await admin.post('/crm/doc-templates', { name: `Secret offer ${stamp}`, engine: 'typst', entity_types: ['deal'], source: (await admin.get(`/crm/doc-templates/${tpl.id}`)).data.source, user_ids: [sam.id], department_ids: [team.id] });
  const secret = r.data;
  ok('a template for chosen people', r.status === 200 && secret.user_ids[0] === sam.id, r.data);
  const listFor = async (c: Client) => (await c.get('/crm/doc-templates?type=deal')).data.items.map((t: any) => t.id);
  ok('the people and departments named see it', (await listFor(sam)).includes(secret.id) && (await listFor(cat)).includes(secret.id));
  ok('others don\'t', !(await listFor(ida)).includes(secret.id));
  const idaDeal = (await admin.post('/crm/deal/items', { title: `Ida deal ${stamp}`, assigned_to: ida.id })).data;
  ok('…and can\'t make documents with it', (await ida.post(`/crm/items/${idaDeal.id}/documents`, { template_id: secret.id })).status === 403);
  await admin.patch(`/crm/deal/items/${idaDeal.id}`, { assigned_to: sam.id });
  ok('someone named can', (await sam.post(`/crm/items/${idaDeal.id}/documents`, { template_id: secret.id })).status === 200);

  // ---- the record's chat ----
  r = await sam.post(`/crm/items/${deal.id}/chat`);
  const chatId = r.data.chat_id;
  ok('the responsible person opens the deal\'s chat', r.status === 200 && !!chatId, r.data);
  // As someone in it (the admin, who made the deal, always is).
  const members = async () => ((await admin.get(`/im/chats/${chatId}`)).data.people ?? []).map((m: any) => m.id);
  ok('its members: the responsible person and whoever made it', (await members()).includes(sam.id));
  await sam.post(`/im/chats/${chatId}/messages`, { body: `About the order ${stamp}` });
  r = await ida.post(`/crm/items/${deal.id}/chat`);
  ok('a colleague who may read the deal joins, as an observer', r.data.chat_id === chatId && (await members()).includes(ida.id) && (await admin.get(`/crm/deal/items/${deal.id}`)).data.observers.some((o: any) => o.id === ida.id), r.data);
  await admin.patch(`/crm/deal/items/${deal.id}`, { assigned_to: cat.id });
  const follows = await waitFor(async () => (await members()).includes(cat.id) && !(await members()).includes(sam.id));
  ok('the chat follows a new responsible person', !!follows, await members());
  ok('nobody renames or fills it by hand', (await admin.patch(`/im/chats/${chatId}`, { title: 'x' })).status === 400);

  // ---- exports ----
  const contact = (await admin.post('/crm/contact/items', { first_name: 'Vera', last_name: `Card${stamp}`, job_title: 'Buyer', multifields: [{ kind: 'email', value_type: 'work', value: `vera.${stamp}@example.com` }, { kind: 'phone', value_type: 'work', value: '+1 555 0199' }] })).data;
  let res = await admin.req('GET', `/crm/contact/export.vcf?q=${stamp}`, undefined, true) as Response;
  const vcf = await res.text();
  ok('contacts as vCards', res.status === 200 && /text\/vcard/.test(res.headers.get('content-type') ?? '') && vcf.includes('BEGIN:VCARD') && vcf.includes(`N:Card${stamp};Vera;;;`) && vcf.includes(`EMAIL;TYPE=INTERNET:vera.${stamp}@example.com`) && vcf.includes('TITLE:Buyer'), vcf.slice(0, 400));
  ok('deals aren\'t vCards', (await admin.req('GET', '/crm/deal/export.vcf', undefined, true) as Response).status === 400);
  res = await admin.req('GET', `/crm/deal/export.xlsx?q=${stamp}`, undefined, true) as Response;
  const xlsx = Buffer.from(await res.arrayBuffer());
  const strings = unzipEntry(xlsx, 'xl/sharedStrings.xml');
  ok('deals as an Excel workbook', res.status === 200 && xlsx.subarray(0, 2).toString() === 'PK' && strings.includes(`Order ${stamp}`) && strings.includes('Amount'), strings.slice(0, 300));
  void contact;

  // ---- the company's own traffic sources ----
  ok('only CRM admins add sources', (await sam.post('/crm/traffic-sources', { name: `Ads ${stamp}` })).status === 403);
  r = await admin.post('/crm/traffic-sources', { name: `Google Ads ${stamp}`, utm: ['Google', 'adwords'] });
  const src = r.data.items.find((s: any) => s.name === `Google Ads ${stamp}`);
  ok('a traffic source with its utm_source values', r.status === 200 && JSON.stringify(src.utm) === JSON.stringify(['adwords', 'google']) && src.code.startsWith('c_'), r.data);
  const cfg = (await admin.get('/crm/config')).data;
  ok('it is a source like the built-in ones', cfg.options.source.includes(src.code) && cfg.source_labels[src.code] === `Google Ads ${stamp}`);
  const lead = (await admin.post('/crm/lead/items', { title: `From ads ${stamp}`, utm: { source: 'GOOGLE', campaign: 'spring' } })).data;
  ok('a record arriving from google gets it', (await admin.get(`/crm/lead/items/${lead.id}`)).data.source === src.code);
  const lead2 = (await admin.post('/crm/lead/items', { title: `Told ${stamp}`, source: 'call', utm: { source: 'google' } })).data;
  ok('a source given is kept', (await admin.get(`/crm/lead/items/${lead2.id}`)).data.source === 'call');
  ok('it can be set by hand', (await admin.patch(`/crm/lead/items/${lead2.id}`, { source: src.code })).status === 200);
  ok('unknown sources are still refused', (await admin.patch(`/crm/lead/items/${lead2.id}`, { source: 'nonsense' })).status === 400);
  await admin.del(`/crm/traffic-sources/${src.id}`);
} finally {
  for (const id of made.categories) await admin.del(`/shop/categories/${id}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
