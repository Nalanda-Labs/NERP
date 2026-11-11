// HR documents to sign: templates (HR only); documents for several employees at once, filled
// in with each one's details; steps in order (approver, company signer, employee) with each
// told in turn; signing (typed name, drawn signature, agreement); the signed copy with its
// certificate page; the public fingerprint check; declining; invalidating; due dates; from a
// file; who may see what; each person's own documents.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { writeFileSync } from 'node:fs';
import pg from 'pg';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const B = 'http://localhost:8080/api';
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string, position?: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Sign${stamp}`, position })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [apr, boss, eve, max, out] = await Promise.all([user('Apr'), user('Boss'), user('Eve', 'Designer'), user('Max', 'Driver'), user('Out')]);
const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
const pdfText = async (c: Client, id: string) => {
  const r = await c.req('GET', `/hr/docs/${id}/file`, undefined, true);
  const buf = Buffer.from(await r.arrayBuffer());
  const f = `doc-${stamp}.pdf`;
  writeFileSync(f, buf);
  return { status: r.status, text: execFileSync('pdftotext', [f, '-']).toString(), sha: createHash('sha256').update(buf).digest('hex') };
};
const notes = async (c: Client) => JSON.stringify((await c.get('/notifications')).data);

try {
  // ---- templates ----
  ok('only HR makes templates', (await eve.post('/hr/doc-templates', { name: 'x', body: 'y' })).status === 403);
  let r = await admin.post('/hr/doc-templates', { name: `Contract ${stamp}`, body: '# Employment contract\n\n{{company}} employs {{name}} as {{position}} from {{date}}.\n\nSigned by both parties.' });
  const tpl = r.data.items.find((t: any) => t.name === `Contract ${stamp}`);
  ok('a template', !!tpl);

  // ---- making documents ----
  ok('only HR sends documents', (await eve.post('/hr/docs', { title: 'x', template_id: tpl.id, employee_ids: [eve.id] })).status === 403);
  ok('a due date in the past is refused', (await admin.post('/hr/docs', { title: 'x', template_id: tpl.id, employee_ids: [eve.id], due_on: '2020-01-01' })).status === 400);
  r = await admin.post('/hr/docs', { title: `Contract ${stamp}`, template_id: tpl.id, employee_ids: [eve.id, max.id], approver_ids: [apr.id], company_signer_id: boss.id, due_on: '2099-12-31' });
  ok('one document per employee', r.status === 200 && r.data.ids.length === 2, r.data);
  const [d1, d2] = r.data.ids;
  let d = (await admin.get(`/hr/docs/${d1}`)).data;
  ok('waiting for approval; steps in order', d.status === 'approval' && d.steps.map((s: any) => s.role).join(',') === 'approver,company,employee' && d.steps[0].status === 'pending', d);
  ok('the approver is asked', (await notes(apr)).includes(`Please approve “Contract ${stamp}” for Eve Sign${stamp}`));
  let t = await pdfText(admin, d1);
  ok('filled in with Eve\'s details', t.status === 200 && t.text.includes(`Eve Sign${stamp}`) && t.text.includes('Designer') && t.text.includes('Employment contract'), t.text.slice(0, 400));
  t = await pdfText(admin, d2);
  ok('…and Max\'s with his', t.text.includes(`Max Sign${stamp}`) && t.text.includes('Driver'));
  ok('colleagues can\'t see it', (await out.get(`/hr/docs/${d1}`)).status === 404);
  ok('not Eve\'s turn yet', (await eve.post(`/hr/docs/${d1}/sign`, { signed_name: 'Eve', signature: PNG, agree: true })).status === 400);
  ok('the approver has it to do', (await apr.get('/hr/docs?scope=todo')).data.items.some((x: any) => x.id === d1 && x.my_turn === 'approver'));

  // ---- the steps ----
  r = await apr.post(`/hr/docs/${d1}/approve`, { approve: true, comment: 'Fine' });
  ok('approved: the company signs next', r.data.status === 'signing' && r.data.steps[1].status === 'pending', r.data);
  ok('the company signer is asked', (await notes(boss)).includes(`Please sign “Contract ${stamp}” for the company`));
  ok('signing needs agreement', (await boss.post(`/hr/docs/${d1}/sign`, { signed_name: 'Boss', signature: PNG, agree: false })).status === 400);
  ok('and a drawn signature', (await boss.post(`/hr/docs/${d1}/sign`, { signed_name: 'Boss', signature: 'data:image/png;base64,AAAA', agree: true })).status === 400);
  r = await boss.post(`/hr/docs/${d1}/sign`, { signed_name: `Boss Sign${stamp}`, signature: PNG, agree: true });
  ok('the company signed; the employee is next', r.data.steps[1].status === 'done' && r.data.steps[2].status === 'pending', r.data);
  ok('Eve is asked', (await notes(eve)).includes(`Please sign “Contract ${stamp}”`));
  r = await eve.post(`/hr/docs/${d1}/sign`, { signed_name: `Eve Sign${stamp}`, signature: PNG, agree: true });
  ok('Eve signed: done', r.data.status === 'signed' && r.data.signed === true && !!r.data.signed_sha256, r.data);
  t = await pdfText(eve, d1);
  ok('the signed copy has the certificate with both signatures', t.text.includes('Signature certificate') && t.text.includes(`Boss Sign${stamp}`) && t.text.includes(`Eve Sign${stamp}`) && t.text.includes(d.sha256), t.text.slice(-600));
  ok('it is the fingerprint published', t.sha === r.data.signed_sha256);
  let v = await (await fetch(`${B}/public/verify/${t.sha}`)).json();
  ok('anyone can check it is valid', v.found && v.valid === true && v.title === `Contract ${stamp}`, v);
  ok('an unknown fingerprint is not found', (await (await fetch(`${B}/public/verify/${'0'.repeat(64)}`)).json()).found === false);
  ok('Eve and HR are told it\'s signed', (await notes(eve)).includes('is signed'));
  ok('Eve\'s own documents list it', (await eve.get(`/hr/docs?scope=employee&employee_id=${eve.id}`)).data.items.some((x: any) => x.id === d1 && x.status === 'signed'));
  ok('others can\'t list Eve\'s documents', (await out.get(`/hr/docs?scope=employee&employee_id=${eve.id}`)).status === 403);

  // ---- invalidating ----
  ok('only HR invalidates', (await boss.post(`/hr/docs/${d1}/revoke`, { reason: 'x' })).status === 403);
  r = await admin.post(`/hr/docs/${d1}/revoke`, { reason: 'Replaced by the new contract' });
  ok('invalidated with a reason', r.data.status === 'revoked' && r.data.revoke_reason === 'Replaced by the new contract', r.data);
  v = await (await fetch(`${B}/public/verify/${t.sha}`)).json();
  ok('the check now says it is not valid', v.found && v.valid === false && v.status === 'revoked', v);
  ok('Eve is told', (await notes(eve)).includes('is no longer valid'));

  // ---- declining ----
  await apr.post(`/hr/docs/${d2}/approve`, { approve: true });
  await boss.post(`/hr/docs/${d2}/sign`, { signed_name: 'Boss', signature: PNG, agree: true });
  r = await max.post(`/hr/docs/${d2}/decline`, { reason: 'The salary is wrong' });
  ok('Max declines: the document stops', r.data.status === 'declined' && r.data.steps[2].status === 'declined', r.data);
  ok('HR is told why', (await notes(admin)).includes('declined to sign') );
  ok('nothing more can happen to it', (await max.post(`/hr/docs/${d2}/sign`, { signed_name: 'Max', signature: PNG, agree: true })).status === 400);

  // ---- an approver sends it back ----
  r = await admin.post('/hr/docs', { title: `Policy ${stamp}`, template_id: tpl.id, employee_ids: [eve.id], approver_ids: [apr.id] });
  const d3 = r.data.ids[0];
  r = await apr.post(`/hr/docs/${d3}/approve`, { approve: false, comment: 'Wrong template' });
  ok('an approver can send it back', r.data.status === 'declined', r.data);

  // ---- from a file, no company signer, a due date ----
  const fd = new FormData();
  fd.append('file', new Blob([`{\\rtf1\\ansi Handbook ${stamp}\\par}`]), `handbook-${stamp}.rtf`);
  const up = (await admin.req('POST', '/files', fd)).data;
  r = await admin.post('/hr/docs', { title: `Handbook ${stamp}`, file_id: up.id, employee_ids: [max.id], due_on: '2099-01-01' });
  const d4 = r.data.ids[0];
  ok('a document from a Word/RTF file goes straight to the employee', r.status === 200 && (await max.get(`/hr/docs/${d4}`)).data.my_turn === 'employee', r.data);
  ok('…as a PDF of the file', (await pdfText(max, d4)).text.includes(`Handbook ${stamp}`));
  await db.query('UPDATE hr_docs SET due_on = current_date - 1 WHERE id = $1', [d4]);
  ok('past its due date it can\'t be signed', (await max.post(`/hr/docs/${d4}/sign`, { signed_name: 'Max', signature: PNG, agree: true })).status === 400);
  ok('and shows as overdue', (await admin.get(`/hr/docs/${d4}`)).data.overdue === true);
  ok('outside systems can hear about signed documents', (await admin.get('/webhooks/events')).data.items.some((e: any) => e.code === 'hr.document.signed'));
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
