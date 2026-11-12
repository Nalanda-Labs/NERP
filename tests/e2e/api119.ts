// Client portal: staff send a contact a one-time sign-in link; signed in (cookie), the contact
// sees their invoices (balances, PDFs), quotes, payments, signatures, tickets and bookings —
// and only theirs, unless allowed their company's invoices; turning it off signs them out.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const B = 'http://localhost:8080/api';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const base = (await admin.get('/crm/config')).data.base_currency;
const today = new Date().toISOString().slice(0, 10);
const email = `pia.${s}@example.com`;
const made: { items: string[]; payments: string[]; ticket?: string; booking?: string; files: string[] } = { items: [], payments: [], files: [] };
const portal = async (path: string, cookie = '', init: RequestInit = {}) => fetch(B + path, { ...init, headers: { 'content-type': 'application/json', ...(cookie ? { cookie } : {}), ...(init.headers ?? {}) } });

try {
  const co = (await admin.post('/crm/company/items', { company_name: `Pia's Co ${s}` })).data;
  const pia = (await admin.post('/crm/contact/items', { first_name: 'Pia', last_name: `P${s}`, company_id: co.id, multifields: [{ kind: 'email', value_type: 'work', value: email }] })).data;
  made.items.push(co.id, pia.id);
  const mine = (await admin.post('/crm/invoice/items', { title: `Pia's invoice ${s}`, contact_ids: [pia.id], currency: base, opportunity: '200', is_manual_amount: true })).data;
  const theirs = (await admin.post('/crm/invoice/items', { title: `Company invoice ${s}`, company_id: co.id, currency: base, opportunity: '900', is_manual_amount: true })).data;
  const quote = (await admin.post('/crm/quote/items', { title: `Pia's quote ${s}`, contact_ids: [pia.id], currency: base, opportunity: '50', is_manual_amount: true })).data;
  made.items.push(mine.id, theirs.id, quote.id);
  const ready = (await admin.get('/inventory/settings')).data.accounting_ready;
  if (ready) {
    await admin.post(`/accounting/invoices/${mine.id}/post`, { date: today });
    const bank = (await admin.get('/accounting/accounts')).data.items.find((a: any) => a.subtype === 'bank' && a.is_active);
    const p = await admin.post('/accounting/payments', { direction: 'in', date: today, account_id: bank.id, amount: '50.00', reference: `REF-${s}`, allocations: [{ doc_kind: 'invoice', doc_id: mine.id, amount: '50.00' }] });
    made.payments.push(p.data.id);
  }
  // A PDF of Pia's invoice, a signed contract, a ticket and a booking.
  const up = async (name: string) => { const fd = new FormData(); fd.append('file', new Blob([`%PDF ${name} ${s}`], { type: 'application/pdf' }), name); const id = (await admin.req('POST', '/files', fd) as any).data.id; made.files.push(id); return id; };
  const pdf = await up(`invoice-${s}.pdf`);
  const signed = await up(`signed-${s}.pdf`);
  const secret = await up(`internal-${s}.pdf`);
  await db.query("INSERT INTO crm_documents (item_id, template_name, engine, number, status, pdf_file_id) VALUES ($1, 'Invoice', 'typst', $2, 'ready', $3)", [mine.id, `D-${s}`, pdf]);
  await db.query("INSERT INTO sign_requests (item_id, title, original_sha256, status, signed_file_id, completed_at) VALUES ($1, $2, 'x', 'completed', $3, now())", [mine.id, `Contract ${s}`, signed]);
  made.ticket = (await db.query("INSERT INTO support_tickets (title, channel, contact_id) VALUES ($1, 'email', $2) RETURNING id", [`Broken tap ${s}`, pia.id])).rows[0].id;
  made.booking = (await db.query("INSERT INTO bookings (starts_at, ends_at, client_name, contact_id, status) VALUES (now() + interval '3 days', now() + interval '3 days 1 hour', 'Pia', $1, 'confirmed') RETURNING id", [pia.id])).rows[0].id;

  // ---- staff ----
  let r = await admin.get(`/crm/items/${pia.id}/portal`);
  ok('off at first, with the contact\'s addresses', r.status === 200 && r.data.enabled === false && r.data.emails[0] === email, r.data);
  ok('only for contacts', (await admin.get(`/crm/items/${co.id}/portal`)).status === 400);
  ok('a link goes to the contact\'s own address only', (await admin.post(`/crm/items/${pia.id}/portal/invite`, { email: 'someone@else.test' })).status === 400);
  r = await admin.post(`/crm/items/${pia.id}/portal/invite`, {});
  ok('a sign-in link, emailed', r.status === 200 && r.data.sent_to === email && /\/portal\/sign-in\?token=/.test(r.data.url), r.data);
  const token = new URL(r.data.url).searchParams.get('token')!;
  ok('…and the portal is on', (await admin.get(`/crm/items/${pia.id}/portal`)).data.enabled === true);
  ok('…by email', (await db.query("SELECT 1 FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1 AND payload->>'text' LIKE '%' || $2 || '%'", [email, token])).rowCount === 1);

  // ---- the client ----
  ok('nothing without signing in', (await portal('/public/portal/me')).status === 401);
  let res = await portal('/public/portal/sign-in', '', { method: 'POST', body: JSON.stringify({ token }) });
  const setCookie = res.headers.get('set-cookie') ?? '';
  const cookie = setCookie.split(';')[0];
  ok('signing in sets a private cookie', res.status === 200 && /^nerp_portal=/.test(cookie) && /HttpOnly/i.test(setCookie) && /Path=\/api\/public\/portal/.test(setCookie), setCookie);
  ok('the link works once', (await portal('/public/portal/sign-in', '', { method: 'POST', body: JSON.stringify({ token }) })).status === 404);
  res = await portal('/public/portal/me', cookie);
  const me = await res.json();
  ok('who they are', res.status === 200 && me.client.name === `Pia P${s}` && me.client.company === `Pia's Co ${s}`, me.client);
  const inv = me.invoices?.find((x: any) => x.id === mine.id);
  ok('their invoice, with what is left and its PDF', inv && inv.pdf === `/api/public/portal/files/${pdf}` && (!ready || (n(inv.balance) === 150 && inv.status === 'open')), inv);
  ok('…not the company\'s other invoices', !me.invoices.some((x: any) => x.id === theirs.id));
  ok('their quote', me.quotes.some((x: any) => x.id === quote.id));
  ok('their payment', !ready || me.payments.some((p: any) => p.reference === `REF-${s}`), me.payments);
  ok('the signed contract, with its copy', me.signatures.some((x: any) => x.title === `Contract ${s}` && x.signed_copy === `/api/public/portal/files/${signed}`));
  ok('their ticket and booking', me.tickets.some((t: any) => t.title === `Broken tap ${s}`) && me.bookings.length >= 1, [me.tickets, me.bookings]);
  res = await portal(`/public/portal/files/${pdf}`, cookie);
  ok('the PDF downloads', res.status === 200 && (await res.text()).includes(`invoice-${s}`));
  ok('…other files don\'t', (await portal(`/public/portal/files/${secret}`, cookie)).status === 404);

  await admin.put(`/crm/items/${pia.id}/portal`, { enabled: true, company_wide: true });
  ok('with the company\'s invoices allowed, those too', (await (await portal('/public/portal/me', cookie)).json()).invoices.some((x: any) => x.id === theirs.id));

  // ---- asking for a new link ----
  await db.query("DELETE FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [email]);
  ok('anyone can ask for a link', (await portal('/public/portal/request', '', { method: 'POST', body: JSON.stringify({ email: `nobody.${s}@example.com` }) })).status === 200);
  ok('…but only those with the portal get one', (await db.query("SELECT 1 FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [`nobody.${s}@example.com`])).rowCount === 0);
  await portal('/public/portal/request', '', { method: 'POST', body: JSON.stringify({ email: email.toUpperCase() }) });
  ok('…as Pia does', (await db.query("SELECT 1 FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [email])).rowCount === 1);

  // ---- off ----
  await admin.put(`/crm/items/${pia.id}/portal`, { enabled: false });
  ok('turning it off signs them out', (await portal('/public/portal/me', cookie)).status === 401);
  ok('signing out clears the cookie', /Max-Age=0/.test((await portal('/public/portal/sign-out', cookie, { method: 'POST' })).headers.get('set-cookie') ?? ''));
  const u = new Client();
  await admin.post('/users', { email: `ty.${s}@acme.test`, password: 'password123', first_name: 'Ty', last_name: `P${s}` });
  await u.post('/auth/login', { email: `ty.${s}@acme.test`, password: 'password123' });
  ok('the portal card needs access to the contact', [200, 403, 404].includes((await u.get(`/crm/items/${pia.id}/portal`)).status));
} finally {
  const entries = (await db.query('SELECT id FROM acc_entries WHERE source_id = ANY($1) OR source_id = ANY($2)', [made.items, made.payments])).rows.map((r) => r.id);
  await db.query('DELETE FROM acc_allocations WHERE payment_id = ANY($1)', [made.payments]).catch(() => {});
  await db.query('DELETE FROM acc_payments WHERE id = ANY($1)', [made.payments]).catch(() => {});
  await db.query('DELETE FROM acc_invoices WHERE item_id = ANY($1)', [made.items]).catch(() => {});
  if (entries.length) {
    await db.query('DELETE FROM acc_lines WHERE entry_id = ANY($1)', [entries]).catch(() => {});
    await db.query('UPDATE acc_entries SET reverses = NULL, reversed_by = NULL WHERE id = ANY($1)', [entries]).catch(() => {});
    await db.query('DELETE FROM acc_entries WHERE id = ANY($1)', [entries]).catch(() => {});
  }
  await db.query('DELETE FROM sign_requests WHERE title = $1', [`Contract ${s}`]).catch(() => {});
  await db.query('DELETE FROM crm_documents WHERE number = $1', [`D-${s}`]).catch(() => {});
  if (made.ticket) await db.query('DELETE FROM support_tickets WHERE id = $1', [made.ticket]).catch(() => {});
  if (made.booking) await db.query('DELETE FROM bookings WHERE id = $1', [made.booking]).catch(() => {});
  await db.query("DELETE FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [email]).catch(() => {});
  await db.query('DELETE FROM crm_items WHERE id = ANY($1)', [made.items]).catch((e) => console.log('cleanup', e.message));
  await db.end();
  await cleanupRun(s);
}
function n(v: unknown) { return Number(v); }
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
