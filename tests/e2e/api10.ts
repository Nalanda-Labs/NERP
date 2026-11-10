// CRM email: mailboxes over IMAP (Dovecot) + SMTP (Mailpit), filing, threading, replies, leads.
import { execSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 600)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'Doc' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const sam = await mk('sam'), ivy = await mk('ivy');
const waitFor = async (fn, ms = 30000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(400); } return v; };
const IMAP = (user) => `imap://127.0.0.1/INBOX -u ${user}:nerptest`;
// Delivers a raw message into a Dovecot mailbox, as if it had arrived.
const deliver = (user, lines) => { const f = `nerp-mail-${Math.random().toString(36).slice(2)}.eml`; writeFileSync(f, lines.join('\r\n')); execSync(`curl -s imap://127.0.0.1/INBOX -u ${user}:nerptest -T ${f}`); };
const date = () => new Date().toUTCString().replace('GMT', '+0000');
const mailpit = async (q) => (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent(q)}`)).json()).messages ?? [];
const conn = { imap_host: '127.0.0.1', imap_port: 143, imap_security: 'none', smtp_host: '127.0.0.1', smtp_port: 1025, smtp_security: 'starttls', smtp_auth: false, allow_invalid_certs: true };

// ---- connecting ----
let r: any = await sam.post('/crm/mailboxes/test', { email: 'sales@nerp.test', password: 'wrong', ...conn });
ok('test: wrong password reported', r.data.imap.ok === false && /login failed/.test(r.data.imap.error) && r.data.smtp.ok === true, r.data);
r = await sam.post('/crm/mailboxes/test', { email: 'sales@nerp.test', password: 'nerptest', ...conn });
ok('test: both sides ok', r.data.imap.ok && r.data.smtp.ok, r.data);
ok('cannot save a failing mailbox', (await sam.post('/crm/mailboxes', { email: 'sales@nerp.test', password: 'bad', ...conn })).status === 400);
// Start clean: remove mailboxes left by earlier runs.
for (const c of [sam, ivy, admin]) for (const m of (await c.get('/crm/mailboxes')).data.items) if (m.is_owner) await c.del(`/crm/mailboxes/${m.id}`);
for (const m of (await admin.get('/crm/mailboxes')).data.items) await admin.del(`/crm/mailboxes/${m.id}`);
r = await sam.post('/crm/mailboxes', { email: 'sales@nerp.test', display_name: 'Sam Sales', password: 'nerptest', create_leads: true, ...conn });
ok('connect mailbox', r.status === 200 && r.data.email === 'sales@nerp.test' && !('secret' in r.data) && !('password' in r.data), r.data);
const mb = r.data;
await waitFor(async () => (await sam.get('/crm/mailboxes')).data.items[0]?.last_sync_at);
ok('first sync done without error', !(await sam.get('/crm/mailboxes')).data.items[0].last_error, (await sam.get('/crm/mailboxes')).data);
ok('colleague does not see it', !(await ivy.get('/crm/mailboxes')).data.items.some((m) => m.id === mb.id));

// ---- incoming mail is filed on contact and deal ----
const client = `client.${stamp}@example.com`;
const contact = (await sam.post('/crm/contact/items', { first_name: 'Clara', last_name: `Client${stamp}`, multifields: [{ kind: 'email', value: client.toUpperCase() }] })).data.id;
const deal = (await sam.post('/crm/deal/items', { title: `Mail deal ${stamp}`, contact_ids: [contact] })).data.id;
deliver('sales@nerp.test', [
  `From: Clara Client <${client}>`, 'To: Sam Sales <sales@nerp.test>', `Subject: Pricing question ${stamp}`, `Message-ID: <q1.${stamp}@example.com>`, `Date: ${date()}`,
  'MIME-Version: 1.0', 'Content-Type: multipart/mixed; boundary="b1"', '',
  '--b1', 'Content-Type: multipart/alternative; boundary="b2"', '',
  '--b2', 'Content-Type: text/plain; charset=utf-8', '', 'Hello, what does the premium plan cost? — Grüße', '',
  '--b2', 'Content-Type: text/html; charset=utf-8', '', '<p style="color:#333">Hello, <b>what does the premium plan cost?</b></p><script>alert(1)</script><img src="https://tracker.example/p.gif">', '',
  '--b2--', '',
  '--b1', 'Content-Type: text/plain; name="spec.txt"', 'Content-Disposition: attachment; filename="spec.txt"', '', 'Specification v1', '',
  '--b1--', '',
]);
await sam.post(`/crm/mailboxes/${mb.id}/sync`);
const inbound = await waitFor(async () => (await sam.get(`/crm/items/${deal}/emails`)).data.items[0]);
ok('incoming email filed on the deal', inbound?.direction === 'in' && inbound.subject === `Pricing question ${stamp}` && inbound.from_addr === client, inbound);
ok('…and on the contact', (await sam.get(`/crm/items/${contact}/emails`)).data.items.some((e) => e.id === inbound.id));
r = await sam.get(`/crm/emails/${inbound.id}`);
ok('html sanitised, text kept', r.data.body_html.includes('<b>what does') && !r.data.body_html.includes('script') && r.data.body_text.includes('Grüße'), r.data.body_html);
const att = r.data.attachments[0];
ok('attachment stored', att?.name === 'spec.txt', r.data.attachments);
let res = await sam.req('GET', `/files/${att.file_id}/content`, undefined, true);
ok('attachment readable via the record', res.status === 200 && (await res.text()).includes('Specification v1'));
ok('attachment needs login', (await new Client().req('GET', `/files/${att.file_id}/content`, undefined, true)).status === 401);
r = await sam.get(`/crm/activities?item_id=${deal}&state=open`);
const todo = r.data.items.find((a) => a.kind === 'email');
ok('open email activity on the deal', todo?.direction === 'incoming' && todo.responsible_id === sam.id, r.data.items);
r = await sam.get(`/crm/items/${deal}/timeline`);
ok('timeline entry', r.data.items.some((e) => e.kind === 'email' && e.data.direction === 'in'));

// ---- reply from the deal ----
const fd = new FormData(); fd.append('file', new Blob(['Price list 2026']), `prices-${stamp}.txt`);
const up = (await sam.post('/files', fd)).data;
ok('ivy cannot send from sam\'s mailbox', (await ivy.post(`/crm/items/${deal}/emails`, { mailbox_id: mb.id, to: [client], subject: 'x', body: 'x' })).status === 404);
ok('bad recipient rejected', (await sam.post(`/crm/items/${deal}/emails`, { mailbox_id: mb.id, to: ['not an address'], subject: 'x', body: 'x' })).status === 400);
r = await sam.post(`/crm/items/${deal}/emails`, { mailbox_id: mb.id, to: [client], subject: `Re: Pricing question ${stamp}`, body: 'Hi Clara,\nthe premium plan is €99.\nSam', reply_to: inbound.id, file_ids: [up.id] });
ok('reply queued', r.status === 200 && r.data.status === 'queued' && r.data.thread_id === inbound.thread_id, r.data);
const reply = r.data;
const sent = await waitFor(async () => { const e = (await sam.get(`/crm/emails/${reply.id}`)).data; return e.status !== 'queued' ? e : null; });
ok('reply sent', sent?.status === 'sent', sent);
const pit = await waitFor(async () => (await mailpit(`subject:"Re: Pricing question ${stamp}"`))[0]);
ok('delivered via SMTP with attachment', pit && pit.To[0].Address === client && pit.Attachments === 1, pit);
const head = pit && await (await fetch(`http://localhost:8026/api/v1/message/${pit.ID}/headers`)).json();
ok('threading headers', head?.['In-Reply-To']?.[0] === `<q1.${stamp}@example.com>` && head?.References?.[0].includes(`q1.${stamp}@example.com`), head);
const sentList = execSync(`curl -s "imap://127.0.0.1/Sent?ALL" -u sales@nerp.test:nerptest -X "SEARCH SUBJECT \\"${stamp}\\""`).toString();
ok('copy kept in the Sent folder', /SEARCH \d+/.test(sentList), sentList);
r = await sam.get(`/crm/activities?item_id=${deal}&state=open`);
ok('incoming activity closed by the reply', !r.data.items.some((a) => a.kind === 'email'), r.data.items);

// ---- the client answers: same conversation ----
deliver('sales@nerp.test', [
  `From: ${client}`, 'To: sales@nerp.test', `Subject: Re: Re: Pricing question ${stamp}`, `Message-ID: <q2.${stamp}@example.com>`,
  `In-Reply-To: <${sent.message_id}>`, `References: <q1.${stamp}@example.com> <${sent.message_id}>`, `Date: ${date()}`, 'Content-Type: text/plain', '', 'Great, we take it.',
]);
await sam.post(`/crm/mailboxes/${mb.id}/sync`);
const second = await waitFor(async () => (await sam.get(`/crm/items/${deal}/emails`)).data.items.find((e) => e.subject === `Re: Re: Pricing question ${stamp}`));
ok('answer threaded into the conversation', second?.thread_id === inbound.thread_id, second);
r = await sam.get(`/crm/items/${deal}/emails`);
ok('deal shows the whole conversation', r.data.items.length === 3, r.data.items.map((e) => e.subject));

// ---- unknown senders and unrelated mail ----
const stranger = `new.${stamp}@prospect.example`;
deliver('sales@nerp.test', [`From: Pat Prospect <${stranger}>`, 'To: sales@nerp.test', `Subject: Interested ${stamp}`, `Message-ID: <n1.${stamp}@prospect.example>`, `Date: ${date()}`, 'Content-Type: text/plain', '', 'Please call me.']);
await sam.post(`/crm/mailboxes/${mb.id}/sync`);
const lead = await waitFor(async () => (await sam.get(`/crm/lead/items?q=${encodeURIComponent(stranger)}`)).data.items[0]);
ok('unknown sender becomes a lead', lead?.title === `Interested ${stamp}` && lead.assigned_to === sam.id, lead);
r = await ivy.post('/crm/mailboxes', { email: 'support@nerp.test', password: 'nerptest', ...conn });
ok('second mailbox (no lead creation)', r.status === 200, r.data);
const mb2 = r.data;
await waitFor(async () => (await ivy.get('/crm/mailboxes')).data.items[0]?.last_sync_at);
deliver('support@nerp.test', [`From: friend.${stamp}@private.example`, 'To: support@nerp.test', `Subject: Lunch ${stamp}`, `Message-ID: <p1.${stamp}@private.example>`, `Date: ${date()}`, 'Content-Type: text/plain', '', 'Lunch tomorrow?']);
await ivy.post(`/crm/mailboxes/${mb2.id}/sync`);
await waitFor(async () => (await ivy.get('/crm/mailboxes')).data.items[0]?.last_sync_at > mb2.created_at, 8000);
await sleep(3000);
ok('private mail is not stored', !(await ivy.get(`/crm/lead/items?q=${encodeURIComponent(`friend.${stamp}`)}`)).data.items.length);

// ---- reading rights ----
ok('colleague with read rights can open the email', (await ivy.get(`/crm/emails/${inbound.id}`)).status === 200);
ok('unknown email 404', (await sam.get('/crm/emails/00000000-0000-0000-0000-000000000000')).status === 404);

// ---- disconnect keeps filed mail ----
ok('disconnect', (await sam.del(`/crm/mailboxes/${mb.id}`)).status === 200);
ok('emails stay on the record', (await sam.get(`/crm/items/${deal}/emails`)).data.items.length === 3);
await ivy.del(`/crm/mailboxes/${mb2.id}`);

console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
