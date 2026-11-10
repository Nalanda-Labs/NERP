// Help desk: who may do what, the lists (categories with a responsible person…), SLAs and the
// SLA clock (deadline, notes don't stop it, replies do, hold, re-scheduling on a new category),
// history, assigning, the early warning and overdue alerts, closing, rating, reopening,
// frequently used answers, requests taken for customers, the public page and its private
// links, closing abandoned tickets, the email inbox (new tickets, replies by subject number
// and by headers, threading of our answers, automatic mail ignored), statistics, spam, deleting.
import { writeFileSync } from 'node:fs';
import { execSync } from 'node:child_process';
import pg from 'pg';
import { Client, sleep } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const roles = (await admin.get('/roles')).data;
const employee = roles.find((x: any) => x.code === 'employee');
const user = async (first: string, perms: string[]) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Desk' })).data;
  if (perms.length) {
    const role = (await admin.post('/roles', { code: `sup_${first.toLowerCase()}_${stamp}`, name: `Support ${first} ${stamp}`, permissions: perms })).data;
    await admin.put(`/users/${u.id}/roles`, { role_ids: [employee.id, role.id] });
  }
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const max = await user('Max', ['support.manage']);
const sue = await user('Sue', ['support.work']);
const tom = await user('Tom', ['support.work']);
const emma = await user('Emma', []);
const bob = await user('Bob', []);
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const waitFor = async <T,>(fn: () => Promise<T>, ms = 30000): Promise<T> => { const end = Date.now() + ms; let v = await fn(); while (!v && Date.now() < end) { await sleep(500); v = await fn(); } return v; };
const notified = async (c: Client, title: string | RegExp, ms = 15000) =>
  waitFor(async () => (await c.get('/notifications')).data.items.find((n: any) => (typeof title === 'string' ? n.title === title : title.test(n.title))), ms);
const mailpit = async (q: string) => (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent(q)}`)).json()).messages ?? [];
const mailText = async (id: string) => (await (await fetch(`http://localhost:8026/api/v1/message/${id}`)).json());
const deliver = (lines: string[]) => { const f = `nerp-mail-${Math.random().toString(36).slice(2)}.eml`; writeFileSync(f, lines.join('\r\n')); execSync(`curl -s imap://127.0.0.1/INBOX -u support@nerp.test:nerptest -T ${f}`); };
const date = () => new Date().toUTCString().replace('GMT', '+0000');
const pub = async (method: string, path: string, body?: unknown) => {
  const r = await fetch(`http://localhost:8080/api/public/support${path}`, { method, headers: body ? { 'content-type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined });
  return { status: r.status, data: await r.json().catch(() => null) };
};
const minutes = (a: string, b: string) => (Date.parse(a) - Date.parse(b)) / 60000;
const original = (await max.get('/support/settings')).data;
let mailbox: any = null;

try {
  // ---- who may do what ----
  ok('employees do not see the settings', (await emma.get('/support/settings')).status === 403);
  ok('support does not change them', (await sue.put('/support/settings', { auto_close_days: 3 })).status === 403);
  ok('everyone reads the lists', (await emma.get('/support/dictionary')).status === 200);
  ok('employees do not read the SLAs', (await emma.get('/support/slas')).status === 403);
  const team = (await sue.get('/support/team')).data.items.map((u: any) => u.id);
  ok('the team: people who work on the help desk', team.includes(sue.id) && team.includes(max.id) && !team.includes(emma.id), team);

  // ---- lists ----
  ok('only managers add to the lists', (await sue.post('/support/dictionary', { kind: 'category', name: `x ${stamp}` })).status === 403);
  ok('a category goes to someone on the help desk', (await max.post('/support/dictionary', { kind: 'category', name: `Bad ${stamp}`, responsible_id: emma.id })).status === 400);
  let r = await max.post('/support/dictionary', { kind: 'category', name: `Network ${stamp}`, responsible_id: sue.id });
  const network = r.data;
  ok('a category with a responsible person', r.status === 200 && network.responsible_id === sue.id && network.kind === 'category', r.data);
  ok('only categories have one', (await max.post('/support/dictionary', { kind: 'status', name: `S ${stamp}`, responsible_id: sue.id })).status === 400);
  ok('unknown kind', (await max.post('/support/dictionary', { kind: 'colour', name: `C ${stamp}` })).status === 400);
  const dict = (await emma.get('/support/dictionary')).data.items;
  const of = (kind: string, name: string) => dict.find((e: any) => e.kind === kind && e.name === name)?.id;
  const high = of('criticality', 'High'), inProgress = of('status', 'Problem solving in progress'), suits = of('mark', 'Answer suits the needs');
  ok('the standard lists are there', high && inProgress && suits && dict.some((e: any) => e.kind === 'status' && e.is_default), dict.length);

  // ---- SLAs ----
  const sla = (body: any) => max.post('/support/slas', { name: `Network SLA ${stamp}`, response_minutes: 120, notice_minutes: 30, ...body });
  ok('the warning comes before the deadline', (await sla({ notice_minutes: 120 })).status === 400);
  ok('working hours are checked', (await sla({ hours: { 1: [[600, 540]] } })).status === 400);
  ok('categories are checked', (await sla({ category_ids: [high] })).status === 400);
  r = await sla({ priority: 10, category_ids: [network.id], responsible_id: tom.id });
  const netSla = r.data;
  ok('an SLA for one category', r.status === 200 && netSla.category_ids[0] === network.id && netSla.hours['1'][0][1] === 1440, r.data);
  const highSla = (await max.post('/support/slas', { name: `Urgent SLA ${stamp}`, response_minutes: 60, priority: 20, criticality_ids: [high], category_ids: [network.id] })).data;
  const standard = (await max.get('/support/slas')).data.items.find((s: any) => s.is_default);
  ok('the default SLA stays', (await max.del(`/support/slas/${standard.id}`)).status === 400);

  // ---- an employee asks ----
  r = await emma.post('/support/tickets', { title: `VPN is down ${stamp}`, body: 'I cannot connect to the VPN since this morning.', category_id: network.id });
  const t1 = r.data;
  ok('a request', r.status === 200 && t1.number > 0 && t1.channel === 'staff' && t1.requester_id === emma.id && t1.messages.length === 1, r.data);
  ok('it goes to the category\'s person', t1.responsible_id === sue.id && t1.responsible_name === 'Sue Desk', t1);
  ok('the requester does not see support\'s details', !('sla_id' in t1) && !('minutes_spent' in t1) && t1.as_support === false, Object.keys(t1));
  ok('Sue is told', await notified(sue, `New ticket #${t1.number}: VPN is down ${stamp}`));
  let s1 = (await sue.get(`/support/tickets/${t1.id}`)).data;
  ok('under the category\'s SLA, due in 2 hours', s1.sla_id === netSla.id && s1.sla_name === `Network SLA ${stamp}` && Math.abs(minutes(s1.deadline_at, s1.created_at) - 120) < 1.5 && s1.waiting_for === 'support', s1);
  ok('others on the help desk see it', (await tom.get(`/support/tickets/${t1.id}`)).status === 200);
  ok('other employees do not', (await bob.get(`/support/tickets/${t1.id}`)).status === 404 && !(await bob.get('/support/tickets?scope=mine')).data.items.length);
  ok('in her requests', (await emma.get('/support/tickets?scope=mine')).data.items.some((t: any) => t.id === t1.id));
  ok('employees only see their own', (await emma.get('/support/tickets?scope=all')).data.items.every((t: any) => t.requester_id === emma.id || t.responsible_id === emma.id));
  ok('she cannot write notes', (await emma.post(`/support/tickets/${t1.id}/messages`, { body: 'x', note: true })).status === 400);
  ok('or set the status', (await emma.post(`/support/tickets/${t1.id}/messages`, { body: 'x', status_id: inProgress })).status === 403);
  ok('or change the ticket', (await emma.patch(`/support/tickets/${t1.id}`, { status_id: inProgress })).status === 403);

  // ---- notes, replies and the clock ----
  r = await sue.post(`/support/tickets/${t1.id}/messages`, { body: `Internal: check the firewall ${stamp}`, note: true, minutes_spent: 5 });
  ok('a note keeps the clock running', r.data.waiting_for === 'support' && r.data.deadline_at === s1.deadline_at && r.data.minutes_spent === 5, r.data);
  ok('she does not see notes', !(await emma.get(`/support/tickets/${t1.id}`)).data.messages.some((m: any) => m.kind !== 'message'));
  r = await sue.post(`/support/tickets/${t1.id}/messages`, { body: 'Please restart the VPN client and try again.', minutes_spent: 10, status_id: inProgress });
  ok('a reply stops the clock', r.data.waiting_for === 'customer' && r.data.deadline_at === null && r.data.first_response_at && r.data.status_id === inProgress && r.data.minutes_spent === 15, r.data);
  ok('the status change is in the history', r.data.messages.some((m: any) => m.kind === 'log' && m.body === 'Status: Request accepted → Problem solving in progress'), r.data.messages);
  ok('Emma is told', await notified(emma, `Answer to your request #${t1.number}: VPN is down ${stamp}`));
  r = await emma.post(`/support/tickets/${t1.id}/messages`, { body: 'Still not working.' });
  ok('her answer starts it again', r.data.waiting_for === 'support' && r.data.deadline_at && r.data.messages.filter((m: any) => m.kind === 'message').length === 3, r.data);
  ok('Sue hears about it', await notified(sue, `New message on #${t1.number}: VPN is down ${stamp}`));

  // ---- changes ----
  ok('tickets go to the help desk', (await sue.patch(`/support/tickets/${t1.id}`, { responsible_id: emma.id })).status === 400);
  r = await sue.patch(`/support/tickets/${t1.id}`, { criticality_id: high, responsible_id: tom.id });
  ok('more critical: the stricter SLA, due an hour after her message', r.data.sla_id === highSla.id && r.data.responsible_id === tom.id
    && Math.abs(minutes(r.data.deadline_at, r.data.waiting_since) - 60) < 1.5, r.data);
  ok('history of the changes', ['Criticality: Middle → High', 'Responsible: Sue Desk → Tom Desk', `SLA: Urgent SLA ${stamp}`].every((h) => r.data.messages.some((m: any) => m.kind === 'log' && m.body === h)), r.data.messages.filter((m: any) => m.kind === 'log'));
  ok('Tom is told', await notified(tom, `Ticket #${t1.number} is yours: VPN is down ${stamp}`));
  r = await tom.patch(`/support/tickets/${t1.id}`, { on_hold: true });
  ok('on hold: no deadline', r.data.on_hold && r.data.deadline_at === null, r.data);
  r = await tom.patch(`/support/tickets/${t1.id}`, { on_hold: false });
  ok('released: due an hour from now', !r.data.on_hold && Math.abs(minutes(r.data.deadline_at, new Date().toISOString()) - 60) < 2, r.data);

  // ---- the early warning and overdue ----
  const t2 = (await emma.post('/support/tickets', { title: `Printer jam ${stamp}`, body: 'Paper stuck.', category_id: network.id })).data;
  await db.query(`UPDATE support_tickets SET notice_at = now() - interval '1 minute' WHERE id = $1`, [t2.id]);
  await db.query(`UPDATE support_tickets SET deadline_at = now() - interval '1 minute', notice_sent = TRUE WHERE id = $1`, [t1.id]);
  ok('a warning before the deadline', await notified(sue, `Reply due soon on #${t2.number}: Printer jam ${stamp}`, 100000));
  ok('overdue: Tom is told', await notified(tom, `Overdue: #${t1.number} VPN is down ${stamp}`, 100000));
  s1 = (await tom.get(`/support/tickets/${t1.id}`)).data;
  ok('flagged overdue', s1.is_overdue && s1.overdue_count === 1, s1);
  ok('in the overdue list', (await tom.get('/support/tickets?scope=overdue')).data.items.some((t: any) => t.id === t1.id));
  r = await tom.post(`/support/tickets/${t1.id}/messages`, { body: 'Fixed: the certificate was renewed.' });
  ok('a late reply is marked; the ticket is no longer overdue', !r.data.is_overdue && r.data.overdue_count === 1 && r.data.messages.filter((m: any) => m.kind === 'message').at(-1).is_overdue, r.data);

  // ---- closing, rating, reopening ----
  ok('rated only when closed', (await emma.post(`/support/tickets/${t1.id}/rate`, { mark_id: suits })).status === 400);
  r = await tom.post(`/support/tickets/${t1.id}/close`);
  ok('closed', r.data.closed_at && !r.data.auto_closed && r.data.deadline_at === null, r.data);
  ok('Emma is asked to rate', await notified(emma, `Your request #${t1.number} was closed: VPN is down ${stamp}`));
  ok('only the requester rates', (await tom.post(`/support/tickets/${t1.id}/rate`, { mark_id: suits })).status === 403);
  ok('a mark from the marks list', (await emma.post(`/support/tickets/${t1.id}/rate`, { mark_id: high })).status === 400);
  r = await emma.post(`/support/tickets/${t1.id}/rate`, { mark_id: suits });
  ok('rated', r.data.mark_id === suits && r.data.can_rate, r.data);
  ok('Tom hears it', await notified(tom, `#${t1.number} was rated “Answer suits the needs”`));
  r = await emma.post(`/support/tickets/${t1.id}/messages`, { body: 'It broke again.' });
  ok('writing again reopens it', r.data.closed_at === null && r.data.mark_id === null && r.data.waiting_for === 'support' && r.data.deadline_at, r.data);
  r = await tom.post(`/support/tickets/${t1.id}/close`);
  r = await tom.post(`/support/tickets/${t1.id}/reopen`);
  ok('support reopens too', r.data.closed_at === null && r.data.messages.some((m: any) => m.kind === 'log' && m.body === 'Reopened'), r.data);

  // ---- frequently used answers ----
  ok('only managers write them', (await sue.post('/support/answers', { title: `Restart ${stamp}`, body: 'x' })).status === 403);
  const ans = (await max.post('/support/answers', { title: `Restart ${stamp}`, body: 'Please restart your computer.' })).data;
  ok('support reads them', (await sue.get('/support/answers')).data.items.some((a: any) => a.id === ans.id && a.body === 'Please restart your computer.'));
  ok('employees do not', (await emma.get('/support/answers')).status === 403);

  // ---- a request taken for a customer (no inbox: the system address writes) ----
  await max.put('/support/settings', { mailbox_id: null });
  const cust = `cust.${stamp}@example.com`;
  ok('only support takes requests for others', (await emma.post('/support/tickets', { title: 'x', body: 'y', customer_email: cust })).status === 403);
  r = await sue.post('/support/tickets', { title: `Invoice copy ${stamp}`, body: 'Called: needs a copy of invoice 42.', customer_email: cust, customer_name: 'Carla Customer' });
  const t3 = r.data;
  ok('for a customer', r.status === 200 && t3.requester_id === null && t3.requester_email === cust && t3.requester_name === 'Carla Customer', r.data);
  let got = await waitFor(async () => (await mailpit(`to:${cust} subject:"[#${t3.number}]"`))[0]);
  ok('she gets the number and her link', got && got.Subject === `[#${t3.number}] Invoice copy ${stamp}`, got);
  const token = /help\/t\/([A-Za-z0-9_-]+)/.exec((await mailText(got.ID)).Text)?.[1];
  ok('the link is in the email', !!token);
  let p = await pub('GET', `/t/${token}`);
  ok('her page', p.status === 200 && p.data.number === t3.number && p.data.messages.length === 1 && p.data.can_write, p.data);
  await sue.post(`/support/tickets/${t3.id}/messages`, { body: `Not for you ${stamp}`, note: true });
  r = await sue.post(`/support/tickets/${t3.id}/messages`, { body: 'Here is the copy you asked for.' });
  p = await pub('GET', `/t/${token}`);
  ok('she sees answers, not notes', p.data.messages.length === 2 && p.data.messages[1].by_support && p.data.messages[1].author_name === 'Sue Desk' && !JSON.stringify(p.data).includes('Not for you'), p.data);
  got = await waitFor(async () => (await mailpit(`to:${cust} subject:"Re: [#${t3.number}]"`))[0]);
  ok('the answer by email', got, got);
  ok('a wrong link finds nothing', (await pub('GET', '/t/nope')).status === 404);

  // ---- the public page ----
  p = await pub('GET', '');
  ok('the page lists categories', p.status === 200 && p.data.categories.some((c: any) => c.id === network.id) && p.data.company, p.data);
  ok('robots are turned away', (await pub('POST', '/tickets', { name: 'Bot', email: `bot.${stamp}@example.com`, title: 'x', body: 'y', website: 'spam.example' })).status === 400);
  ok('an email address is needed', (await pub('POST', '/tickets', { name: 'Pat', email: 'nope', title: 'x', body: 'y' })).status === 400);
  const pat = `pat.${stamp}@example.com`;
  p = await pub('POST', '/tickets', { name: 'Pat Public', email: pat, title: `Cannot log in ${stamp}`, body: 'My password does not work.', category_id: network.id });
  ok('a request from the page', p.status === 200 && p.data.number > 0 && p.data.token, p.data);
  const ptoken = p.data.token;
  const t4 = (await sue.get(`/support/tickets?scope=open&q=${p.data.number}`)).data.items[0];
  ok('support finds it by number', t4?.channel === 'portal' && t4.requester_name === 'Pat Public' && t4.responsible_id === sue.id, t4);
  ok('Pat gets the link', await waitFor(async () => (await mailpit(`to:${pat} subject:"[#${t4.number}] Cannot log in"`))[0]));
  await sue.post(`/support/tickets/${t4.id}/messages`, { body: 'Reset it with the link on the sign-in page.' });
  p = await pub('POST', `/t/${ptoken}/messages`, { body: 'Thanks, that worked!' });
  ok('Pat answers on the page', p.status === 200 && p.data.messages.length === 3 && p.data.waiting_for === 'support', p.data);
  p = await pub('POST', `/t/${ptoken}/close`);
  ok('and closes it', p.data.closed_at, p.data);
  ok('Sue hears it', await notified(sue, `#${t4.number} was closed by the customer: Cannot log in ${stamp}`));
  p = await pub('POST', `/t/${ptoken}/rate`, { mark_id: suits });
  ok('and rates it', p.data.mark_id === suits, p.data);
  await max.put('/support/settings', { portal_enabled: false });
  ok('a switched-off page is gone', (await pub('GET', '')).status === 404 && (await pub('POST', '/tickets', { name: 'P', email: pat, title: 'x', body: 'y' })).status === 404);
  await max.put('/support/settings', { portal_enabled: true });

  // ---- tickets left waiting for the customer close by themselves ----
  await max.put('/support/settings', { auto_close_days: 2 });
  await db.query(`UPDATE support_tickets SET last_message_at = now() - interval '3 days' WHERE id = $1`, [t3.id]);
  const closedT3 = await waitFor(async () => { const t = (await sue.get(`/support/tickets/${t3.id}`)).data; return t.closed_at ? t : null; }, 100000);
  ok('closed automatically', closedT3?.auto_closed && closedT3.messages.some((m: any) => m.body === 'Closed automatically: no answer from the customer in 2 days'), closedT3);
  ok('the customer is told', await waitFor(async () => (await mailpit(`to:${cust} subject:"[#${t3.number}] Invoice copy ${stamp}: closed"`))[0]));
  await max.put('/support/settings', { auto_close_days: original.auto_close_days });

  // ---- the email inbox ----
  for (const m of (await max.get('/crm/mailboxes')).data.items) if (m.is_owner) await max.del(`/crm/mailboxes/${m.id}`);
  for (const m of (await admin.get('/crm/mailboxes')).data.items) await admin.del(`/crm/mailboxes/${m.id}`);
  mailbox = (await max.post('/crm/mailboxes', { email: 'support@nerp.test', display_name: 'Acme Support', password: 'nerptest', imap_host: '127.0.0.1', imap_port: 143, imap_security: 'none', smtp_host: '127.0.0.1', smtp_port: 1025, smtp_security: 'starttls', smtp_auth: false, allow_invalid_certs: true })).data;
  await waitFor(async () => (await max.get('/crm/mailboxes')).data.items.find((m: any) => m.id === mailbox.id)?.last_sync_at);
  ok('the inbox must be one\'s own', (await admin.put('/support/settings', { mailbox_id: mailbox.id })).status === 400);
  r = await max.put('/support/settings', { mailbox_id: mailbox.id });
  ok('the help-desk inbox', r.data.mailbox_email === 'support@nerp.test', r.data);
  const sender = `mia.${stamp}@example.com`;
  deliver([`From: Mia Mail <${sender}>`, 'To: support@nerp.test', `Subject: Monitor flickers ${stamp}`, `Message-ID: <m1.${stamp}@example.com>`, `Date: ${date()}`,
    'MIME-Version: 1.0', 'Content-Type: multipart/mixed; boundary="b1"', '', '--b1', 'Content-Type: text/plain; charset=utf-8', '', 'My monitor flickers all the time.', '',
    '--b1', 'Content-Type: text/plain; name="photo.txt"', 'Content-Disposition: attachment; filename="photo.txt"', '', 'pretend photo', '', '--b1--', '']);
  await max.post(`/crm/mailboxes/${mailbox.id}/sync`);
  const t5 = await waitFor(async () => (await sue.get(`/support/tickets?scope=open&q=Monitor flickers ${stamp}`)).data.items[0]);
  ok('an email opens a ticket', t5?.channel === 'email' && t5.requester_email === sender && t5.requester_name === 'Mia Mail' && t5.title === `Monitor flickers ${stamp}`, t5);
  let s5 = (await sue.get(`/support/tickets/${t5.id}`)).data;
  ok('with its attachment', s5.messages[0].body === 'My monitor flickers all the time.' && s5.messages[0].files[0]?.name === 'photo.txt', s5.messages[0]);
  ok('support opens the attachment', (await sue.get(`/files/${s5.messages[0].files[0].id}`)).status === 200);
  ok('others do not', (await bob.get(`/files/${s5.messages[0].files[0].id}`)).status !== 200);
  ok('not filed in the CRM', !(await db.query(`SELECT 1 FROM crm_emails WHERE subject LIKE $1`, [`%Monitor flickers ${stamp}%`])).rowCount);
  const ack = await waitFor(async () => (await mailpit(`to:${sender} subject:"[#${t5.number}] Monitor flickers"`))[0]);
  ok('the acknowledgement comes from the inbox', ack?.From?.Address === 'support@nerp.test' && ack.From.Name === 'Acme Support', ack);
  deliver([`From: Mia Mail <${sender}>`, 'To: support@nerp.test', `Subject: Re: [#${t5.number}] Monitor flickers ${stamp}`, `Message-ID: <m2.${stamp}@example.com>`, `Date: ${date()}`,
    'Content-Type: text/plain; charset=utf-8', '', 'It is the left one.', '', `On Tue, Acme Support <support@nerp.test> wrote:`, '> We got your request', '']);
  await max.post(`/crm/mailboxes/${mailbox.id}/sync`);
  s5 = await waitFor(async () => { const t = (await sue.get(`/support/tickets/${t5.id}`)).data; return t.messages.filter((m: any) => m.kind === 'message').length === 2 ? t : null; });
  ok('a reply with the number joins the ticket, without the quote', s5?.messages.filter((m: any) => m.kind === 'message')[1].body === 'It is the left one.', s5?.messages);
  await sue.post(`/support/tickets/${t5.id}/messages`, { body: 'Try another cable, please.' });
  const ours = await waitFor(async () => (await mailpit(`to:${sender} subject:"Re: [#${t5.number}]"`))[0]);
  const headers = ours && (await (await fetch(`http://localhost:8026/api/v1/message/${ours.ID}/headers`)).json());
  ok('our answer is threaded on her message', ours?.From?.Address === 'support@nerp.test' && headers?.['In-Reply-To']?.[0] === `<m2.${stamp}@example.com>`, headers);
  s5 = (await sue.get(`/support/tickets/${t5.id}`)).data;
  ok('and marked sent', await waitFor(async () => (await sue.get(`/support/tickets/${t5.id}`)).data.messages.filter((m: any) => m.kind === 'message').at(-1).email_status === 'sent'));
  deliver([`From: Mia Mail <${sender}>`, 'To: support@nerp.test', `Subject: Re: ${stamp} thanks`, `Message-ID: <m3.${stamp}@example.com>`, `In-Reply-To: ${headers?.['Message-Id']?.[0] ?? ''}`, `Date: ${date()}`,
    'Content-Type: text/plain; charset=utf-8', '', 'The cable did it, thanks.', '']);
  deliver([`From: Mia Mail <${sender}>`, 'To: support@nerp.test', `Subject: Out of office ${stamp}`, 'Auto-Submitted: auto-replied', `Message-ID: <m4.${stamp}@example.com>`, `Date: ${date()}`,
    'Content-Type: text/plain; charset=utf-8', '', 'I am away.', '']);
  deliver([`From: Someone Else <other.${stamp}@example.com>`, 'To: support@nerp.test', `Subject: Re: [#${t5.number}] hijack ${stamp}`, `Message-ID: <m5.${stamp}@example.com>`, `Date: ${date()}`,
    'Content-Type: text/plain; charset=utf-8', '', 'Not my ticket.', '']);
  await max.post(`/crm/mailboxes/${mailbox.id}/sync`);
  s5 = await waitFor(async () => { const t = (await sue.get(`/support/tickets/${t5.id}`)).data; return t.messages.filter((m: any) => m.kind === 'message').length === 4 ? t : null; });
  ok('an answer to our email joins by its headers', s5?.messages.filter((m: any) => m.kind === 'message')[3].body === 'The cable did it, thanks.' && s5.waiting_for === 'support', s5?.messages);
  const other = await waitFor(async () => (await sue.get(`/support/tickets?scope=open&q=other.${stamp}`)).data.items[0]);
  ok('someone else naming the ticket gets a new one', other && other.id !== t5.id && other.title === `[#${t5.number}] hijack ${stamp}`, other);
  ok('automatic replies are ignored', !(await sue.get(`/support/tickets?scope=all&q=Out of office ${stamp}`)).data.items.length);
  await max.put('/support/settings', { mailbox_id: null });

  // ---- statistics, lists, spam, deleting ----
  ok('employees do not see the statistics', (await emma.get('/support/stats')).status === 403);
  r = await sue.get('/support/stats');
  const cat = r.data.by_category?.find((c: any) => c.id === network.id);
  ok('statistics', r.status === 200 && r.data.created >= 5 && cat?.created >= 3 && r.data.replies > 0 && r.data.late_replies >= 1 && r.data.avg_first_response_minutes !== null
    && r.data.marks.find((m: any) => m.id === suits)?.count >= 1 && r.data.daily.length === 30, r.data);
  ok('by person', r.data.by_responsible.some((p: any) => p.id === tom.id && p.late_replies >= 1), r.data.by_responsible);
  ok('filters', (await sue.get(`/support/tickets?scope=all&category_id=${network.id}&q=${stamp}`)).data.items.every((t: any) => t.category_id === network.id));
  r = await sue.patch(`/support/tickets/${t4.id}`, { is_spam: true });
  ok('spam', r.data.is_spam && (await pub('GET', `/t/${ptoken}`)).status === 404 && (await sue.get('/support/tickets?scope=spam')).data.items.some((t: any) => t.id === t4.id));
  ok('support does not delete', (await sue.del(`/support/tickets/${t4.id}`)).status === 403);
  ok('managers do', (await max.del(`/support/tickets/${t4.id}`)).status === 200 && (await sue.get(`/support/tickets/${t4.id}`)).status === 404);
  await max.del(`/support/dictionary/${network.id}`);
  ok('a removed category leaves the SLAs', !(await max.get('/support/slas')).data.items.find((s: any) => s.id === netSla.id).category_ids.length);
} finally {
  await max.put('/support/settings', { mailbox_id: original.mailbox_id, auto_close_days: original.auto_close_days, portal_enabled: original.portal_enabled }).catch(() => {});
  if (mailbox) await max.del(`/crm/mailboxes/${mailbox.id}`).catch(() => {});
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
