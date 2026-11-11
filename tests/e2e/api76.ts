// E-signature depth: signing templates (signers by role, filled in from each record), sending
// a record's document with one (made from a document template first), the company switch
// that limits e-signature to people with its permission, CRM triggers on "everyone signed"
// and a robot that sends for signature, and what happens after an HR document is signed
// (people told, a task, an item added to a list).
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const waitFor = async <T>(f: () => Promise<T | null | undefined | false>, ms = 15000): Promise<T | null> => { const end = Date.now() + ms; while (Date.now() < end) { const v = await f(); if (v) return v as T; await sleep(300); } return null; };
const MP = 'http://localhost:8026/api/v1';
const mailpit = async (q: string): Promise<any[]> => (await (await fetch(`${MP}/search?query=${encodeURIComponent(q)}`)).json()).messages ?? [];
const linkFor = async (to: string) => waitFor(async () => {
  const m = (await mailpit(`to:${to} subject:"Please sign"`))[0];
  return m ? (await (await fetch(`${MP}/message/${m.ID}`)).json()).Text.match(/\/sign\/([\w-]+)/)?.[1] : null;
});
const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
const pub = (method: string, path: string, body?: unknown) => fetch(`http://localhost:8080/api${path}`, { method, headers: body ? { 'content-type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined }).then(async (r) => ({ status: r.status, data: await r.json().catch(() => null) }));

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string, perms: string[] = []) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Sig${stamp}` })).data;
  if (perms.length) {
    const roles = (await admin.get('/roles')).data;
    const role = (await admin.post('/roles', { code: `sig_${first.toLowerCase()}_${stamp}`, name: `Sig ${first} ${stamp}`, permissions: perms })).data;
    await admin.put(`/users/${u.id}/roles`, { role_ids: [roles.find((r: any) => r.code === 'employee').id, role.id] });
  }
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const sam = await user('Sam');
const cat = await user('Cat');
const settingsBefore = (await admin.get('/settings')).data;

try {
  // ---- a signing template ----
  const docTpl = (await admin.get('/crm/doc-templates?type=deal')).data.items.find((t: any) => t.builtin_key === 'typst.invoice');
  ok('roles are checked', (await admin.post('/crm/sign-templates', { name: `X ${stamp}`, signers: [{ role: 'nobody' }] })).status === 400
    && (await admin.post('/crm/sign-templates', { name: `X ${stamp}`, signers: [{ role: 'person', name: 'A' }] })).status === 400);
  ok('only CRM admins (or e-signature people) make them', (await sam.post('/crm/sign-templates', { name: `X ${stamp}`, signers: [{ role: 'contact' }] })).status === 403);
  let r = await admin.post('/crm/sign-templates', {
    name: `Contract ${stamp}`, message: 'Please sign the contract.', doc_template_id: docTpl.id,
    signers: [{ role: 'contact' }, { role: 'responsible' }, { role: 'person', name: 'Legal', email: `legal.${stamp}@example.com` }], entity_types: [2],
  });
  const tpl = r.data;
  ok('a signing template', r.status === 200 && tpl.signers.length === 3 && tpl.doc_template_name === docTpl.name, r.data);

  const contact = (await admin.post('/crm/contact/items', { first_name: 'Cleo', last_name: `Client${stamp}`, multifields: [{ kind: 'email', value_type: 'work', value: `cleo.${stamp}@example.com` }] })).data;
  const deal = (await admin.post('/crm/deal/items', { title: `Deal ${stamp}`, contact_ids: [contact.id], assigned_to: sam.id })).data;
  r = await sam.get(`/crm/items/${deal.id}/sign-templates/${tpl.id}/signers`);
  ok('it is filled in for the record: its contact, its responsible person, the fixed signer', JSON.stringify(r.data.signers.map((s: any) => s.email)) === JSON.stringify([`cleo.${stamp}@example.com`, `sam.${stamp}@acme.test`, `legal.${stamp}@example.com`]), r.data);
  const bare = (await admin.post('/crm/deal/items', { title: `Bare ${stamp}`, assigned_to: sam.id })).data;
  r = await sam.post(`/crm/items/${bare.id}/sign`, { template_id: tpl.id });
  ok('a role that finds nobody is named, nothing is sent', r.status === 400 && /contact/.test(r.data.error.message), r.data);
  const lead = (await admin.post('/crm/lead/items', { title: `Lead ${stamp}` })).data;
  ok('a template for deals isn\'t used on leads', (await admin.post(`/crm/items/${lead.id}/sign`, { template_id: tpl.id })).status === 400);

  // ---- a trigger: everyone signed → won ----
  const pipeline = (await admin.post('/crm/deal/categories', { name: `Sign pipe ${stamp}` })).data.categories.find((c: any) => c.name === `Sign pipe ${stamp}`);
  const [first, second] = pipeline.stages.filter((s: any) => s.semantics === 'process');
  const won = pipeline.stages.find((s: any) => s.semantics === 'success');
  r = await admin.post('/crm/triggers', { category_id: pipeline.id, stage_id: won.id, name: 'Signed', event: 'signature_completed', params: { sign_template_id: tpl.id }, allow_backwards: false });
  ok('a trigger on "everyone signed"', r.status === 200, r.data);
  ok('…its template is checked', (await admin.post('/crm/triggers', { category_id: pipeline.id, stage_id: won.id, event: 'signature_declined', params: { sign_template_id: 'x' } })).status === 400);
  await admin.patch(`/crm/deal/items/${deal.id}`, { category_id: pipeline.id, stage_id: first.id });

  // ---- sending with the template ----
  r = await sam.post(`/crm/items/${deal.id}/sign`, { template_id: tpl.id });
  ok('the responsible person sends it: the document is made and goes out', r.status === 200 && r.data.status === 'sent' && !!r.data.request_id, r.data);
  const reqs = (await sam.get(`/crm/items/${deal.id}/signatures`)).data.items;
  ok('…to the three signers, with the template\'s message', reqs[0]?.signers?.length === 3 && reqs[0].message === 'Please sign the contract.', reqs[0]);
  for (const email of [`cleo.${stamp}@example.com`, `sam.${stamp}@acme.test`, `legal.${stamp}@example.com`]) {
    const token = await linkFor(email);
    await pub('GET', `/public/sign/${token}`);
    await pub('POST', `/public/sign/${token}`, { signed_name: 'Signer', signature: PNG, agree: true });
  }
  const done = await waitFor(async () => (await sam.get(`/crm/items/${deal.id}/signatures`)).data.items[0]?.status === 'completed');
  ok('everyone signed', !!done);
  const moved = await waitFor(async () => (await admin.get(`/crm/deal/items/${deal.id}`)).data.stage_id === won.id);
  ok('the trigger moved the deal to Won', !!moved, (await admin.get(`/crm/deal/items/${deal.id}`)).data.stage_id);

  // ---- a robot that sends for signature ----
  r = await admin.post('/crm/robots', { category_id: pipeline.id, stage_id: second.id, name: `Send ${stamp}`, action: 'sign', params: { sign_template_id: tpl.id } });
  ok('a "Send for signature" robot', r.status === 200, r.data);
  ok('…its template is checked', (await admin.post('/crm/robots', { category_id: pipeline.id, stage_id: second.id, action: 'sign', params: {} })).status === 400);
  const deal2 = (await admin.post('/crm/deal/items', { title: `Deal two ${stamp}`, contact_ids: [contact.id], assigned_to: sam.id, category_id: pipeline.id, stage_id: first.id })).data;
  await admin.patch(`/crm/deal/items/${deal2.id}`, { stage_id: second.id });
  const sent = await waitFor(async () => (await admin.get(`/crm/items/${deal2.id}/signatures`)).data.items[0]);
  ok('entering the stage sends the document for signature', (sent as any)?.status === 'sent' && (sent as any).signers.length === 3, sent);

  // ---- limiting e-signature to its permission ----
  await admin.put('/settings', { esign_restricted: true });
  ok('when limited, an employee without the permission can\'t send', (await sam.post(`/crm/items/${deal2.id}/sign`, { template_id: tpl.id })).status === 403);
  const req2 = (sent as any)?.id;
  ok('…or cancel', (await sam.post(`/crm/signatures/${req2}/cancel`, {})).status === 403);
  const eva = await user('Eva', ['crm.esign']);
  await admin.patch(`/crm/deal/items/${deal2.id}`, { assigned_to: eva.id });
  ok('someone with "Send for e-signature" can', (await eva.post(`/crm/signatures/${req2}/cancel`, {})).status === 200);
  await admin.put('/settings', { esign_restricted: false });

  // ---- after an HR document is signed ----
  const list = (await admin.post('/lists', { name: `Onboarding ${stamp}`, fields: [{ label: 'Employee', kind: 'user' }, { label: 'Signed on', kind: 'date' }, { label: 'Document', kind: 'text' }] })).data;
  ok('after-signing rules are checked', (await admin.post('/hr/doc-templates', { name: `X ${stamp}`, body: 'x', after_signing: { notify: ['nope'] } })).status === 400);
  r = await admin.post('/hr/doc-templates', {
    name: `NDA ${stamp}`, body: '# NDA\n\n{{name}} keeps secrets.',
    after_signing: { notify: [cat.id], task: { title: 'Welcome pack for {{name}}', responsible: 'hr' }, list_id: list.id },
  });
  const hrTpl = r.data.items.find((t: any) => t.name === `NDA ${stamp}`);
  ok('an HR template with after-signing rules', hrTpl?.after_signing?.list_id === list.id && hrTpl.after_signing.task.title === 'Welcome pack for {{name}}', hrTpl);
  const emp = await user('Emma');
  r = await admin.post('/hr/docs', { title: `NDA ${stamp}`, template_id: hrTpl.id, employee_ids: [emp.id] });
  const docId = r.data.ids?.[0];
  r = await emp.post(`/hr/docs/${docId}/sign`, { signed_name: 'Emma', signature: PNG, agree: true });
  ok('the employee signs', r.status === 200, r.data);
  const told = await waitFor(async () => ((await cat.get('/notifications')).data.items ?? []).some((n: any) => n.title.includes(`signed “NDA ${stamp}”`)));
  ok('the people named are told', !!told);
  const task = await waitFor(async () => ((await admin.get(`/tasks?q=${encodeURIComponent('Welcome pack for Emma')}`)).data.items ?? []).find((t: any) => t.title === `Welcome pack for Emma Sig${stamp}`));
  ok('a task is made for whoever sent it', !!task, (await admin.get(`/tasks?q=Welcome`)).data.items?.map((t: any) => t.title));
  const items = await waitFor(async () => ((await admin.get(`/lists/${list.id}/items`)).data.items ?? []).find((i: any) => i.title === `NDA ${stamp}: Emma Sig${stamp}`));
  ok('an item is added to the list, with the employee, the date and the document', !!items && JSON.stringify((items as any).values ?? {}).includes(emp.id) && JSON.stringify((items as any).values ?? {}).includes(`NDA ${stamp}`), items);
  await admin.del(`/tasks/${(task as any)?.id}`);
  await admin.del(`/crm/sign-templates/${tpl.id}`);
} finally {
  await admin.put('/settings', { esign_restricted: settingsBefore.esign_restricted ?? false });
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
