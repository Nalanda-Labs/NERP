// Moving over from another CRM: a HubSpot export (companies, contacts, deals, notes, tasks)
// and a Salesforce data export ZIP (with its users file) are recognised, mapped and brought
// over with their links, owners, a new pipeline with its stages, notes and activities, quietly;
// a second run updates instead of duplicating; undo removes what a migration made; only CRM
// administrators can do it.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { makeZip } from './zip.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 1200)); };
const s = Date.now().toString(36);
const admin = new Client();
const me = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user;
const annEmail = `ann.${s}@acme.test`;
const annUser = (await admin.post('/users', { email: annEmail, password: 'password123', first_name: 'Ann', last_name: `Mig${s}` })).data;
const ann = new Client();
await ann.post('/auth/login', { email: annEmail, password: 'password123' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const migrations: string[] = [];
const csv = (rows: (string | number)[][]) => rows.map((r) => r.map((v) => /[",\n]/.test(String(v)) ? `"${String(v).replace(/"/g, '""')}"` : String(v)).join(',')).join('\r\n') + '\r\n';
const upload = async (name: string, bytes: Uint8Array | string, type: string) => {
  const fd = new FormData(); fd.append('file', new Blob([bytes], { type }), name);
  return (await admin.req('POST', '/files', fd)).data.id as string;
};
const finished = async (id: string) => waitFor(async () => { const m = (await admin.get(`/crm/migrations/${id}`)).data; return m.status === 'done' || m.status === 'failed' ? m : null; }, 120_000);
const planOf = (a: any, fileIds: string[], extra: Record<string, unknown> = {}) => ({
  file_ids: fileIds, source: a.source,
  files: a.files.map((f: any) => ({ name: f.name, kind: f.kind, mapping: f.mapping })),
  pipelines: Object.fromEntries(a.pipelines.map((p: any) => [p.value, p.category_id ?? 'new'])),
  stages: Object.fromEntries(a.stages.filter((x: any) => x.stage_id).map((x: any) => [`${x.kind}|${x.pipeline}|${x.value}`, x.stage_id])),
  owners: Object.fromEntries(a.owners.filter((o: any) => o.user_id).map((o: any) => [o.value, o.user_id])),
  ...extra,
});
const items = async (code: string, q: string) => (await admin.get(`/crm/${code}/items?q=${encodeURIComponent(q)}&limit=50`)).data.items as any[];
const detail = async (code: string, id: string) => (await admin.get(`/crm/${code}/items/${id}`)).data;

try {
  // ---- a HubSpot export ----
  const hub = makeZip({
    'companies.csv': csv([
      ['Record ID', 'Company name', 'Company Domain Name', 'Phone Number', 'City', 'Country/Region', 'Industry', 'Company owner', 'Create Date'],
      ['c1', `Acme ${s}`, `acme-${s}.test`, '+1 555 0100', 'Berlin', 'DE', 'Software', 'admin@acme.test', '2024-03-01 10:00'],
      ['c2', `Globex ${s}`, '', '', '', '', '', `Ann Mig${s}`, ''],
    ]),
    'contacts.csv': csv([
      ['Record ID', 'First Name', 'Last Name', 'Email', 'Phone Number', 'Job Title', 'Associated Company IDs', 'Contact owner'],
      ['p1', 'Ada', `Mig${s}`, `ada.${s}@client.test`, '+1 555 0101', 'CTO', 'c1', 'admin@acme.test'],
      ['p2', 'Bo', `Mig${s}`, `bo.${s}@client.test`, '', '', 'c2', `Ann Mig${s}`],
      ['p3', 'Cy', `Mig${s}`, '', '+1 555 0103', '', '', 'Zed Nobody'],
    ]),
    'deals.csv': csv([
      ['Record ID', 'Deal Name', 'Amount', 'Pipeline', 'Deal Stage', 'Close Date', 'Associated Company IDs', 'Associated Contact IDs', 'Deal owner'],
      ['d1', `Big deal ${s}`, '5000', `Partners ${s}`, 'Negotiation', '2026-05-01', 'c1', 'p1;p2', `Ann Mig${s}`],
      ['d2', `Lost deal ${s}`, '100', `Partners ${s}`, 'Closed lost', '', 'c2', '', 'admin@acme.test'],
      ['d3', `Plain deal ${s}`, '50', '', 'Closed won', '', '', 'p3', ''],
    ]),
    'notes.csv': csv([
      ['Record ID', 'Note body', 'Associated Deal IDs', 'Created Date'],
      ['n1', '<p>Called &amp; <b>agreed</b> on terms</p>', 'd1', '2024-04-02 09:30'],
      ['n2', 'Lost its record', '999', ''],
    ]),
    'tasks.csv': csv([
      ['Record ID', 'Task Title', 'Due Date', 'Task Status', 'Task Type', 'Associated Contact IDs'],
      ['t1', 'Send proposal', '2099-06-01', 'NOT_STARTED', 'TODO', 'p1'],
      ['t2', 'Intro call', '2024-01-10', 'COMPLETED', 'CALL', 'p2'],
    ]),
  });
  const hubFile = await upload(`hubspot-${s}.zip`, hub, 'application/zip');
  ok('only CRM administrators', (await ann.post('/crm/migrations/analyze', { file_ids: [hubFile] })).status === 403 && (await ann.get('/crm/migrations')).status === 403);
  let r = await admin.post('/crm/migrations/analyze', { file_ids: [hubFile] });
  const a = r.data;
  ok('recognised as a HubSpot export', r.status === 200 && a.source === 'hubspot', r.data);
  const kinds = Object.fromEntries((a.files ?? []).map((f: any) => [f.name, f.kind]));
  ok('each file\'s kind', kinds['companies.csv'] === 'company' && kinds['contacts.csv'] === 'contact' && kinds['deals.csv'] === 'deal' && kinds['notes.csv'] === 'note' && kinds['tasks.csv'] === 'activity', kinds);
  const cm = a.files?.find((f: any) => f.name === 'contacts.csv')?.mapping ?? {};
  ok('…and columns', cm['Record ID'] === 'id' && cm['Associated Company IDs'] === 'company_ref' && cm['Contact owner'] === 'owner' && cm['Email'] === 'email', cm);
  const partners = a.pipelines?.find((p: any) => p.value === `Partners ${s}`);
  const plainPipe = a.pipelines?.find((p: any) => p.value === '');
  ok('pipelines: a new one, and the default for deals without one', partners?.category_id === 'new' && typeof plainPipe?.category_id === 'string' && plainPipe.category_id !== 'new', a.pipelines);
  const won = a.stages?.find((x: any) => x.value === 'Closed won');
  ok('stages: "Closed won" goes to the default pipeline\'s won stage', won?.semantics === 'success' && /^[0-9a-f-]{36}$/.test(won?.stage_id ?? ''), a.stages);
  const ownerOf = (v: string) => a.owners?.find((o: any) => o.value === v)?.user_id;
  ok('owners matched by email and by name', ownerOf('admin@acme.test') === me.id && ownerOf(`Ann Mig${s}`) === annUser.id && ownerOf('Zed Nobody') === null, a.owners);

  r = await admin.post('/crm/migrations', planOf(a, [hubFile]));
  ok('started', r.status === 200 && r.data.status === 'queued', r.data);
  migrations.push(r.data.id);
  let m = await finished(r.data.id);
  const c = m?.progress?.counts ?? {};
  ok('done: companies, contacts, deals, notes, activities', m?.status === 'done' && c.company?.created === 2 && c.contact?.created === 3 && c.deal?.created === 3 && c.note?.created === 1 && c.activity?.created === 2, m);
  ok('a note whose record isn\'t there is reported', c.note?.failed === 1 && m.progress.errors.some((e: any) => e.file === 'notes.csv' && e.line === 3), m?.progress?.errors);

  const [ada] = await items('contact', `ada.${s}@client.test`);
  const acme = (await items('company', `Acme ${s}`))[0];
  const adaD = ada && await detail('contact', ada.id);
  ok('contact linked to its company, with its details', adaD?.company_id === acme?.id && adaD.job_title === 'CTO' && adaD.multifields.some((x: any) => x.kind === 'email' && x.value === `ada.${s}@client.test`), adaD);
  const acmeD = acme && await detail('company', acme.id);
  ok('company: website, address, industry noted, its original date', acmeD?.multifields.some((x: any) => x.kind === 'web') && acmeD.address?.city === 'Berlin' && acmeD.address?.country === 'DE' && /Industry: Software/.test(acmeD.comments ?? '') && acmeD.created_at.startsWith('2024-03-01'), acmeD);
  const bo = (await items('contact', `bo.${s}@client.test`))[0];
  const cy = (await items('contact', `Cy Mig${s}`))[0];
  ok('owners: by name to Ann, unknown to whoever ran it', bo?.assigned_to === annUser.id && cy?.assigned_to === me.id, [bo?.assigned_to, cy?.assigned_to]);

  const cats = (await admin.get('/crm/config')).data.types.find((t: any) => t.code === 'deal').categories;
  const pipe = cats.find((x: any) => x.name === `Partners ${s}`);
  const stageNames = pipe?.stages.map((x: any) => `${x.name}:${x.semantics}`);
  ok('a new pipeline with the stages found (and a won stage)', !!pipe && stageNames.includes('Negotiation:process') && stageNames.includes('Closed lost:failure') && pipe.stages.some((x: any) => x.semantics === 'success'), stageNames);
  const big = (await items('deal', `Big deal ${s}`))[0];
  const bigD = big && await detail('deal', big.id);
  ok('deal: pipeline, stage, amount, close date, company and both contacts', bigD?.category_id === pipe?.id && bigD.stage_id === pipe?.stages.find((x: any) => x.name === 'Negotiation')?.id
    && Number(bigD.opportunity) === 5000 && bigD.close_date === '2026-05-01' && bigD.company_id === acme?.id && bigD.contacts.length === 2 && bigD.assigned_to === annUser.id, bigD);
  const plainDeal = (await items('deal', `Plain deal ${s}`))[0];
  ok('a deal without a pipeline: the default one, at its won stage', plainDeal?.stage_id === won?.stage_id, [plainDeal?.stage_id, won?.stage_id]);
  const tl = (await admin.get(`/crm/items/${big?.id}/timeline`)).data.items;
  const note = tl.find((e: any) => e.kind === 'comment');
  ok('the note on the deal\'s timeline, as text, with its date', note && /Called & \*\*agreed\*\* on terms/.test(note.body) && note.created_at.startsWith('2024-04-02'), tl.map((e: any) => [e.kind, e.body, e.created_at]));
  ok('the timeline says where it came from', tl.some((e: any) => e.kind === 'created' && e.data?.imported_from === 'hubspot'));
  const acts = (await db.query('SELECT a.subject, a.is_completed, a.kind, a.deadline FROM crm_activities a WHERE a.item_id = ANY($1) ORDER BY a.subject', [[ada?.id, bo?.id]])).rows;
  ok('activities: an open task and a done call', acts.length === 2 && acts[0].subject === 'Intro call' && acts[0].is_completed && acts[0].kind === 'call'
    && acts[1].subject === 'Send proposal' && !acts[1].is_completed && acts[1].kind === 'todo', acts);
  const notified = (await ann.get('/notifications')).data.items.filter((n: any) => /Mig|deal/.test(`${n.title} ${n.body}`) && n.kind?.startsWith('crm'));
  ok('quietly: nobody is notified of records handed to them', notified.length === 0, notified);

  // ---- the same export again ----
  r = await admin.post('/crm/migrations', planOf(a, [hubFile]));
  migrations.push(r.data.id);
  m = await finished(r.data.id);
  const c2 = m?.progress?.counts ?? {};
  ok('a second run updates, it doesn\'t duplicate', m?.status === 'done' && c2.company?.updated === 2 && c2.contact?.updated === 3 && c2.deal?.updated === 3 && !c2.deal?.created && c2.note?.skipped === 1 && c2.activity?.skipped === 2, c2);
  ok('…still one of each', (await items('deal', `Big deal ${s}`)).length === 1 && (await items('contact', `Mig${s}`)).length === 3);

  // ---- a Salesforce data export ----
  const defaultCat = cats.find((x: any) => x.is_default);
  const firstOpen = defaultCat.stages.find((x: any) => x.semantics === 'process');
  const sf = makeZip({
    'Account.csv': csv([['Id', 'Name', 'Phone', 'BillingCity', 'OwnerId'], ['001A', `Initech ${s}`, '+1 555 0200', 'Austin', '005A']]),
    'Contact.csv': csv([['Id', 'AccountId', 'FirstName', 'LastName', 'Email', 'Title', 'OwnerId'], ['003A', '001A', 'Pat', `Sf${s}`, `pat.${s}@initech.test`, 'Buyer', '005A']]),
    'Opportunity.csv': csv([['Id', 'AccountId', 'Name', 'StageName', 'Amount', 'CloseDate', 'OwnerId', 'CurrencyIsoCode', 'Probability'], ['006A', '001A', `Renewal ${s}`, 'Prospecting', '1200.50', '2026-09-30', '005A', 'XYZ', '0.4']]),
    'Task.csv': csv([['Id', 'WhoId', 'WhatId', 'Subject', 'ActivityDate', 'Status', 'OwnerId'], ['00TA', '003A', '006A', 'Follow up', '2099-01-02', 'Not Started', '005A']]),
    'User.csv': csv([['Id', 'Email', 'FirstName', 'LastName'], ['005A', annEmail, 'Ann', `Mig${s}`]]),
  });
  const sfFile = await upload(`salesforce-${s}.zip`, sf, 'application/zip');
  const b = (await admin.post('/crm/migrations/analyze', { file_ids: [sfFile] })).data;
  ok('recognised as a Salesforce export', b.source === 'salesforce' && b.files.find((f: any) => f.name === 'User.csv')?.kind === 'user', b.files?.map((f: any) => [f.name, f.kind]));
  const o = b.owners?.find((x: any) => x.value === '005A');
  ok('owners through its users file', o?.via === annEmail && o.user_id === annUser.id, b.owners);
  ok('a currency that isn\'t enabled is pointed out', b.currencies?.some((x: any) => x.code === 'XYZ' && !x.enabled), b.currencies);
  r = await admin.post('/crm/migrations', planOf(b, [sfFile], { stages: { 'deal||Prospecting': firstOpen.id } }));
  migrations.push(r.data.id);
  m = await finished(r.data.id);
  ok('brought over', m?.status === 'done' && m.progress.counts.company?.created === 1 && m.progress.counts.contact?.created === 1 && m.progress.counts.deal?.created === 1 && m.progress.counts.activity?.created === 1, m?.progress);
  const ren = (await items('deal', `Renewal ${s}`))[0];
  const renD = ren && await detail('deal', ren.id);
  ok('the stage chosen, probability as a percentage, the base currency, linked', renD?.stage_id === firstOpen.id && renD.probability === 40 && Number(renD.opportunity) === 1200.5
    && renD.currency !== 'XYZ' && renD.company_title === `Initech ${s}` && renD.assigned_to === annUser.id, renD);
  const pat = (await items('contact', `pat.${s}@initech.test`))[0];
  ok('the task on the contact (its first link)', (await db.query('SELECT 1 FROM crm_activities WHERE item_id = $1 AND subject = $2', [pat?.id, 'Follow up'])).rowCount === 1);

  // ---- undo ----
  ok('undo needs a finished migration', (await admin.post(`/crm/migrations/${crypto.randomUUID()}/undo`)).status === 404);
  r = await admin.post(`/crm/migrations/${migrations[2]}/undo`);
  ok('undo: what it made goes to the recycle bin', r.status === 200 && r.data.records === 3 && r.data.activities === 1 && (await items('deal', `Renewal ${s}`)).length === 0, r.data);
  ok('…once', (await admin.post(`/crm/migrations/${migrations[2]}/undo`)).status === 409);
  r = await admin.post(`/crm/migrations/${migrations[0]}/undo`);
  ok('undoing the first run (its records, notes and activities)', r.status === 200 && r.data.records === 8 && r.data.notes === 1 && r.data.activities === 2 && (await items('contact', `Mig${s}`)).length === 0, r.data);
  const list = (await admin.get('/crm/migrations')).data.items;
  ok('the list of migrations', list.find((x: any) => x.id === migrations[0])?.status === 'undone' && list.find((x: any) => x.id === migrations[1])?.status === 'done');
} finally {
  await db.query(`DELETE FROM crm_items WHERE title ILIKE $1 OR last_name ILIKE $1 OR company_name ILIKE $1`, [`%${s}%`]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM crm_categories WHERE name = $1', [`Partners ${s}`]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM crm_migrations WHERE id = ANY($1)', [migrations]).catch((e) => console.log('cleanup:', e.message));
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
