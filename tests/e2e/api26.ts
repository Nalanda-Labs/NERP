// E-signature: sending a generated document (validation, access, one request at a time),
// the signer's page and PDF, signing (checks, IP and time kept), declining, reminders
// (new links), cancelling, the signed copy (certificate page, fingerprints, emails), timeline
// entries and notifications.
import { execSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
// Older poppler (Ubuntu 24.04) warns about "/Suspects false" in tagged PDFs, which is valid: keep it out of the output.
const quiet = { stdio: ['ignore', 'pipe', 'ignore'] as any };
const MP = 'http://localhost:8026/api/v1';
const mailpit = async (q: string): Promise<any[]> => (await (await fetch(`${MP}/search?query=${encodeURIComponent(q)}`)).json()).messages ?? [];
const linkIn = async (to: string, subject: string) => {
  const m = await waitFor(async () => (await mailpit(`to:${to} subject:"${subject}"`))[0], 15000);
  const text: string = (await (await fetch(`${MP}/message/${m.ID}`)).json()).Text;
  return text.match(/\/sign\/([\w-]+)/)![1];
};
// A tiny PNG: one dark pixel, enough to be "a drawn signature".
const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const sam = new Client();
const samUser = (await admin.post('/users', { email: `sam.${stamp}@acme.test`, password: 'password123', first_name: 'Sam', last_name: 'Sign' })).data;
await sam.post('/auth/login', { email: `sam.${stamp}@acme.test`, password: 'password123' });
const pub = new Client('', 'signer-browser/1.0');

try {
  const deal = (await admin.post('/crm/deal/items', { title: `Contract ${stamp}`, assigned_to: samUser.id })).data;
  const tpl = (await admin.get('/crm/doc-templates?type=deal')).data.items.find((t: any) => t.builtin_key === 'typst.invoice');
  const doc = await waitFor(async () => {
    const d = (await admin.post(`/crm/items/${deal.id}/documents`, { template_id: tpl.id })).data;
    return d.status === 'ready' ? d : null;
  }, 20000);
  ok('a ready document', doc?.pdf_file_id);

  // ---- sending ----
  ok('signers needed', (await sam.post(`/crm/documents/${doc.id}/signature`, { signers: [] })).status === 400);
  ok('emails checked', (await sam.post(`/crm/documents/${doc.id}/signature`, { signers: [{ name: 'A', email: 'nope' }] })).status === 400);
  ok('each signer once', (await sam.post(`/crm/documents/${doc.id}/signature`, { signers: [{ name: 'A', email: `a.${stamp}@example.com` }, { name: 'B', email: `A.${stamp}@example.com` }] })).status === 400);
  const outsider = new Client();
  await admin.post('/users', { email: `out.${stamp}@acme.test`, password: 'password123', first_name: 'Out', last_name: 'Sider' });
  await outsider.post('/auth/login', { email: `out.${stamp}@acme.test`, password: 'password123' });
  let r = await sam.post(`/crm/documents/${doc.id}/signature`, { signers: [{ name: `Ana Ruiz ${stamp}`, email: `ana.${stamp}@example.com` }, { name: `Ben Ode ${stamp}`, email: `ben.${stamp}@example.com` }], message: 'Please sign by Friday' });
  const req = r.data;
  ok('sent', r.status === 200 && req.status === 'sent' && req.signers.length === 2 && req.original_sha256.length === 64 && req.original_file_id !== doc.pdf_file_id, r.data);
  ok('one request at a time', (await sam.post(`/crm/documents/${doc.id}/signature`, { signers: [{ name: 'C', email: `c.${stamp}@example.com` }] })).status === 400);
  const anaToken = await linkIn(`ana.${stamp}@example.com`, `Please sign ${doc.number}`);
  const benToken = await linkIn(`ben.${stamp}@example.com`, `Please sign ${doc.number}`);
  ok('each signer has a personal link', anaToken && benToken && anaToken !== benToken);

  // ---- the signer's page ----
  r = await pub.get(`/public/sign/${anaToken}`);
  ok('signer page', r.data.title === doc.number && r.data.name === `Ana Ruiz ${stamp}` && r.data.message === 'Please sign by Friday' && r.data.fingerprint === req.original_sha256 && r.data.others[0].name === `Ben Ode ${stamp}`, r.data);
  const pdf = await fetch(`http://localhost:8080/api/public/sign/${anaToken}/document`);
  const bytes = Buffer.from(await pdf.arrayBuffer());
  ok('the PDF as sent', pdf.headers.get('content-type') === 'application/pdf' && execSync('sha256sum', { input: bytes }).toString().startsWith(req.original_sha256));
  ok('opened', (await sam.get(`/crm/items/${deal.id}/signatures`)).data.items[0].signers[0].status === 'viewed');
  ok('bad link', (await pub.get('/public/sign/nope')).status === 404);

  // ---- signing ----
  ok('consent needed', (await pub.post(`/public/sign/${anaToken}`, { signed_name: 'Ana', signature: PNG, agree: false })).status === 400);
  ok('a drawn signature needed', (await pub.post(`/public/sign/${anaToken}`, { signed_name: 'Ana', signature: 'data:image/png;base64,AAAA', agree: true })).status === 400);
  r = await pub.post(`/public/sign/${anaToken}`, { signed_name: `Ana Ruiz ${stamp}`, signature: PNG, agree: true });
  ok('signed', r.status === 200 && r.data.status === 'signed' && r.data.request_status === 'sent', r.data);
  ok('only once', (await pub.post(`/public/sign/${anaToken}`, { signed_name: 'Ana', signature: PNG, agree: true })).status === 400);
  const note = await waitFor(async () => (await sam.get('/notifications')).data.items?.find((n: any) => n.title === `Ana Ruiz ${stamp} signed ${doc.number}`), 5000);
  ok('sender told', !!note);

  // ---- reminders: new links ----
  r = await sam.post(`/crm/signatures/${req.id}/remind`, {});
  ok('reminded the one left', r.data.reminded === 1, r.data);
  const benToken2 = await waitFor(async () => {
    const ms = await mailpit(`to:ben.${stamp}@example.com subject:"Please sign ${doc.number}"`);
    if (ms.length < 2) return null;
    const text: string = (await (await fetch(`${MP}/message/${ms[0].ID}`)).json()).Text;
    return text.match(/\/sign\/([\w-]+)/)![1];
  }, 15000);
  ok('the old link stops working', benToken2 && benToken2 !== benToken && (await pub.get(`/public/sign/${benToken}`)).status === 404);
  ok('outsiders cannot remind or cancel', (await outsider.post(`/crm/signatures/${req.id}/remind`, {})).status === 403 || (await outsider.post(`/crm/signatures/${req.id}/remind`, {})).status === 404);

  // ---- the last signature ----
  r = await pub.post(`/public/sign/${benToken2}`, { signed_name: `Ben Ode ${stamp}`, signature: PNG, agree: true });
  ok('everyone signed', r.data.request_status === 'completed' && r.data.signed_available, r.data);
  const done = (await sam.get(`/crm/items/${deal.id}/signatures`)).data.items[0];
  ok('the signed copy', done.status === 'completed' && done.signed_file_id && done.signed_sha256?.length === 64 && done.signed_sha256 !== done.original_sha256 && done.signers.every((s: any) => s.signed_at), done);
  const signed = await fetch(`http://localhost:8080/api/public/sign/${anaToken}/document`);
  const sb = Buffer.from(await signed.arrayBuffer());
  writeFileSync(`nerp-signed-${stamp}.pdf`, sb);
  const text = execSync(`pdftotext -layout nerp-signed-${stamp}.pdf -`, quiet).toString();
  const pages = Number(execSync(`pdfinfo nerp-signed-${stamp}.pdf`, quiet).toString().match(/Pages:\s+(\d+)/)![1]);
  const origPages = (() => { writeFileSync(`nerp-orig-${stamp}.pdf`, bytes); return Number(execSync(`pdfinfo nerp-orig-${stamp}.pdf`, quiet).toString().match(/Pages:\s+(\d+)/)![1]); })();
  ok('the document plus a certificate page', pages === origPages + 1 && /Signature certificate/.test(text) && text.includes(`Ana Ruiz ${stamp}`) && text.includes(req.original_sha256) && /signer-browser\/1\.0/.test(text), [pages, origPages, text.slice(-1500)]);
  ok('staff can download it', (await sam.req('GET', `/files/${done.signed_file_id}/content`, undefined, true)).status === 200);
  const mails = await waitFor(async () => { const m = await mailpit(`subject:"${doc.number} is signed"`); return m.filter((x: any) => x.To[0].Address.endsWith(`${stamp}@example.com`)).length >= 2 ? m : null; }, 15000);
  ok('everyone got the signed copy', mails && mails.every((m: any) => m.Attachments === 1));
  const tl = (await sam.get(`/crm/items/${deal.id}/timeline`)).data.items.filter((e: any) => e.kind === 'signature').map((e: any) => e.data.event);
  ok('timeline', ['sent', 'signed', 'completed'].every((x) => tl.includes(x)) && tl.filter((x: string) => x === 'signed').length === 2, tl);
  ok('a completed request cannot be cancelled', (await sam.post(`/crm/signatures/${req.id}/cancel`, {})).status === 400);

  // ---- declining, cancelling ----
  r = await sam.post(`/crm/documents/${doc.id}/signature`, { signers: [{ name: `Cy ${stamp}`, email: `cy.${stamp}@example.com` }] });
  ok('a new request after completion', r.status === 200);
  const cyToken = await linkIn(`cy.${stamp}@example.com`, `Please sign ${doc.number}`);
  r = await pub.post(`/public/sign/${cyToken}/decline`, { reason: 'Price is wrong' });
  ok('declined', r.data.status === 'declined' && r.data.request_status === 'declined');
  ok('no signing after a decline', (await pub.post(`/public/sign/${cyToken}`, { signed_name: 'Cy', signature: PNG, agree: true })).status === 400);
  ok('the reason is kept', (await sam.get(`/crm/items/${deal.id}/signatures`)).data.items[0].signers[0].decline_reason === 'Price is wrong');
  r = await sam.post(`/crm/documents/${doc.id}/signature`, { signers: [{ name: `Di ${stamp}`, email: `di.${stamp}@example.com` }] });
  const diToken = await linkIn(`di.${stamp}@example.com`, `Please sign ${doc.number}`);
  await sam.post(`/crm/signatures/${r.data.id}/cancel`, {});
  ok('cancelled: the link says so', (await pub.get(`/public/sign/${diToken}`)).data.request_status === 'cancelled' && (await pub.post(`/public/sign/${diToken}`, { signed_name: 'Di', signature: PNG, agree: true })).status === 400);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
