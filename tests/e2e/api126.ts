// Text messages through Twilio, against the stand-in on :9466 (mock-twilio.ts): set-up and
// who may change it, texting a client from a record (local numbers get the company's calling
// code), Twilio's refusals, signed delivery reports (and refused unsigned ones), replies
// landing on the record, STOP/START opt-outs, the "Text the client" robot, booking
// confirmations, signing links, SMS campaigns and invitations by text.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { startMockTwilio, webhook, waitFor, INVALID } from './mock-twilio.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const BASE = 'http://localhost:8080';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const tw = await startMockTwilio();
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const until = async <T>(f: () => Promise<T | null | undefined | false>, ms = 10_000): Promise<T | null> => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const v = await f();
    if (v) return v;
    await sleep(150);
  }
  return null;
};
const before = (await admin.get('/sms/setup')).data;
const bookingBefore = (await admin.get('/booking/settings')).data;
const made: { robots: string[]; categories: string[]; campaigns: string[]; segments: string[]; resources: string[] } = { robots: [], categories: [], campaigns: [], segments: [], resources: [] };
// A UK number written the local way, and the same one internationally.
const local = (n: number) => `020 7946 ${String(1000 + n).slice(-4)}`;
const intl = (n: number) => `+44207946${String(1000 + n).slice(-4)}`;
const seed = Number.parseInt(s.slice(-4), 36) % 9000;

try {
  const emp = (await admin.post('/users', { email: `tx.${s}@acme.test`, password: 'password123', first_name: 'Tex', last_name: `T${s}` })).data;
  const tex = new Client();
  await tex.post('/auth/login', { email: `tx.${s}@acme.test`, password: 'password123' });

  // ---- set-up ----
  ok('anyone may ask whether texting works', (await tex.get('/sms/available')).status === 200);
  ok('only administrators see the set-up', (await tex.get('/sms/setup')).status === 403 && (await tex.put('/sms/setup', { from: '', country_code: '' })).status === 403);
  ok('the sender must be a number or a Messaging Service', (await admin.put('/sms/setup', { from: 'Acme', country_code: '' })).status === 400);
  ok('calling codes are checked', (await admin.put('/sms/setup', { from: '', country_code: '44' })).status === 400);
  let r = await admin.put('/sms/setup', { from: '', country_code: '+44' });
  ok('set-up: the server\'s sender, UK calling code', r.status === 200 && r.data.enabled === true && r.data.country_code === '+44' && r.data.from_env === '+15005550006' && /^ACnerp…/.test(r.data.account) && r.data.inbound_url.endsWith('/api/public/twilio/sms/inbound'), r.data);

  // ---- from a record ----
  const ann = (await admin.post('/crm/contact/items', { first_name: 'Ann', last_name: `Tx${s}`, assigned_to: emp.id, multifields: [{ kind: 'phone', value: local(seed) }] })).data;
  const nobody = (await admin.post('/crm/contact/items', { first_name: 'No', last_name: `Phone${s}` })).data;
  ok('a record without a phone can\'t be texted', (await admin.post(`/crm/items/${nobody.id}/sms`, { body: 'Hi' })).status === 400);
  ok('an empty message is refused', (await admin.post(`/crm/items/${ann.id}/sms`, { body: '  ' })).status === 400);
  ok('so is one over 1600 characters', (await admin.post(`/crm/items/${ann.id}/sms`, { body: 'x'.repeat(1601) })).status === 400);
  r = await admin.post(`/crm/items/${ann.id}/sms`, { body: `Hello Ann ${s}` });
  ok('texting the client', r.status === 200 && r.data.id && r.data.activity_id, r.data);
  const firstId = r.data.id;
  await waitFor(tw.sent, 1);
  const m1 = tw.sent.find((x) => x.body === `Hello Ann ${s}`);
  ok('…goes to Twilio in international format, from the sender, with a delivery-report address', m1?.to === intl(seed) && m1?.from === '+15005550006' && m1?.statusCallback === `${BASE}/api/public/twilio/sms/status`, m1);
  const row = await until(async () => (await db.query("SELECT status, provider_sid, segments FROM sms_messages WHERE id = $1 AND status <> 'queued'", [firstId])).rows[0]);
  ok('…and is recorded with Twilio\'s id', row?.status === 'sending' && row?.provider_sid === m1?.sid && row?.segments === 1, row);
  r = await admin.get(`/crm/items/${ann.id}/sms`);
  ok('the record shows the conversation', r.status === 200 && r.data.enabled === true && r.data.phones[0] === local(seed) && r.data.messages.some((x: any) => x.id === firstId && x.direction === 'out'), r.data);
  const act = (await db.query("SELECT kind, direction, description FROM crm_activities WHERE sms_id = $1", [firstId])).rows[0];
  ok('…as a completed text-message activity', act?.kind === 'sms' && act?.direction === 'outgoing' && act?.description === `Hello Ann ${s}`, act);

  // Twilio refuses some numbers: the message fails with its reason.
  r = await admin.post(`/crm/items/${ann.id}/sms`, { to: INVALID, body: 'To nowhere' });
  const bad = await until(async () => (await db.query("SELECT status, error FROM sms_messages WHERE id = $1 AND status = 'failed'", [r.data.id])).rows[0]);
  ok('a number Twilio refuses: failed, with Twilio\'s reason', bad?.error?.includes('not a valid phone number'), bad);
  ok('numbers that aren\'t numbers are refused at once', (await admin.post(`/crm/items/${ann.id}/sms`, { to: 'call me', body: 'x' })).status === 400);

  // ---- delivery reports ----
  const status = (params: Record<string, string>, opts = {}) => webhook(BASE, '/api/public/twilio/sms/status', params, opts);
  ok('an unsigned report is refused', (await status({ MessageSid: m1!.sid, MessageStatus: 'delivered' }, { badSignature: true })).status === 403);
  ok('so is one for another account', (await status({ MessageSid: m1!.sid, MessageStatus: 'delivered', AccountSid: 'ACother' })).status === 403);
  ok('a signed report is taken', (await status({ MessageSid: m1!.sid, MessageStatus: 'delivered' })).status === 204);
  await status({ MessageSid: m1!.sid, MessageStatus: 'sent' });
  const st1 = (await db.query('SELECT status FROM sms_messages WHERE id = $1', [firstId])).rows[0].status;
  ok('…and a late "sent" doesn\'t undo "delivered"', st1 === 'delivered', st1);
  r = await admin.post(`/crm/items/${ann.id}/sms`, { body: `Second ${s}` });
  await waitFor(tw.sent, tw.sent.length + 1);
  const second = await until(async () => (await db.query("SELECT provider_sid FROM sms_messages WHERE id = $1 AND provider_sid IS NOT NULL", [r.data.id])).rows[0]);
  await status({ MessageSid: second!.provider_sid, MessageStatus: 'undelivered', ErrorCode: '30003', ErrorMessage: 'Unreachable handset' });
  const und = (await db.query('SELECT status, error FROM sms_messages WHERE id = $1', [r.data.id])).rows[0];
  ok('undelivered, with Twilio\'s error', und.status === 'undelivered' && und.error === 'Twilio error 30003: Unreachable handset', und);
  const told = await until(async () => (await db.query("SELECT 1 FROM notifications WHERE user_id = $1 AND kind = 'sms.failed'", [emp.id])).rowCount);
  ok('…and the responsible person is told', told);

  // ---- replies ----
  const inbound = (params: Record<string, string>, opts = {}) => webhook(BASE, '/api/public/twilio/sms/inbound', params, opts);
  ok('an unsigned incoming message is refused', (await inbound({ MessageSid: `SMin1${s}`, From: intl(seed), To: '+15005550006', Body: 'x' }, { badSignature: true })).status === 403);
  const reply = await inbound({ MessageSid: `SMin1${s}`, From: intl(seed), To: '+15005550006', Body: `Yes please ${s}` });
  ok('a reply is taken (empty TwiML answer)', reply.status === 200 && (await reply.text()).includes('<Response></Response>'));
  await inbound({ MessageSid: `SMin1${s}`, From: intl(seed), To: '+15005550006', Body: `Yes please ${s}` });
  const replies = (await db.query("SELECT item_id, status FROM sms_messages WHERE provider_sid = $1", [`SMin1${s}`])).rows;
  ok('…once, even if Twilio retries, on the record with that number', replies.length === 1 && replies[0].item_id === ann.id && replies[0].status === 'received', replies);
  ok('…as an incoming activity', (await db.query("SELECT 1 FROM crm_activities WHERE item_id = $1 AND kind = 'sms' AND direction = 'incoming' AND description = $2", [ann.id, `Yes please ${s}`])).rowCount === 1);
  ok('…and the responsible person is told', await until(async () => (await db.query("SELECT 1 FROM notifications WHERE user_id = $1 AND kind = 'sms.received'", [emp.id])).rowCount));
  const unknown = await inbound({ MessageSid: `SMin2${s}`, From: '+447700900999', To: '+15005550006', Body: 'Who is this?' });
  ok('a message from an unknown number is kept without a record', unknown.status === 200 && (await db.query("SELECT item_id FROM sms_messages WHERE provider_sid = $1", [`SMin2${s}`])).rows[0]?.item_id === null);
  await inbound({ MessageSid: `SMin3${s}`, From: intl(seed), To: '+15005550006', Body: 'stop' });
  ok('STOP opts the number out', (await db.query('SELECT 1 FROM sms_optouts WHERE number = $1', [intl(seed)])).rowCount === 1);
  await inbound({ MessageSid: `SMin4${s}`, From: intl(seed), To: '+15005550006', Body: 'START' });
  ok('START opts it back in', (await db.query('SELECT 1 FROM sms_optouts WHERE number = $1', [intl(seed)])).rowCount === 0);

  // ---- the robot ----
  const cat = (await admin.post('/crm/deal/categories', { name: `Texting ${s}` })).data.categories.find((c: any) => c.name === `Texting ${s}`);
  made.categories.push(cat.id);
  const [first, next] = [cat.stages[0], cat.stages[1]];
  ok('the robot needs a message', (await admin.post('/crm/robots', { category_id: cat.id, stage_id: next.id, name: 'x', action: 'sms', params: {} })).status === 400);
  r = await admin.post('/crm/robots', { category_id: cat.id, stage_id: next.id, name: `Text ${s}`, action: 'sms', params: { body: 'Hi {{contact}}, about {{title}}' } });
  ok('a "Text the client" robot', r.status === 200, r.data);
  made.robots.push(r.data.id);
  const deal = (await admin.post('/crm/deal/items', { title: `Deal ${s}`, category_id: cat.id, stage_id: first.id, contact_ids: [ann.id], assigned_to: emp.id })).data;
  const n0 = tw.sent.length;
  await admin.patch(`/crm/deal/items/${deal.id}`, { stage_id: next.id });
  await waitFor(tw.sent, n0 + 1);
  const rb = tw.sent.slice(n0).find((x) => x.body.includes(`Deal ${s}`));
  ok('…texts the deal\'s contact, placeholders filled', rb?.to === intl(seed) && rb?.body === `Hi Ann Tx${s}, about Deal ${s}`, tw.sent.slice(n0));
  ok('…and logs it on the deal', await until(async () => (await db.query("SELECT 1 FROM crm_activities WHERE item_id = $1 AND kind = 'sms'", [deal.id])).rowCount));

  // ---- booking messages ----
  r = await admin.put('/booking/settings', { sms_notices: true, confirm_email: true });
  ok('bookings can text clients', r.status === 200 && r.data.sms_notices === true && r.data.sms_available === true, r.data);
  const room = (await admin.post('/booking/resources', { name: `Room ${s}`, kind: 'room', capacity: 2, timezone: 'UTC', min_notice_minutes: 0 })).data;
  made.resources.push(room.id);
  const at = (m: number) => new Date(Math.ceil(Date.now() / 3600e3) * 3600e3 + m * 60e3 + 86400e3).toISOString();
  const n1 = tw.sent.length;
  r = await admin.post('/booking/bookings', { resource_ids: [room.id], starts_at: at(0), ends_at: at(30), client_name: `Bo ${s}`, client_phone: local(seed + 1) });
  ok('a booking with a phone', r.status === 200, r.data);
  await waitFor(tw.sent, n1 + 1);
  const bk = tw.sent.slice(n1).find((x) => x.to === intl(seed + 1));
  ok('…texts the confirmation with the link to manage it', /you're booked/.test(bk?.body ?? '') && /\/book\/manage\//.test(bk?.body ?? ''), tw.sent.slice(n1));

  // ---- signing links ----
  const tpl = (await admin.get('/crm/doc-templates?type=deal')).data.items.find((t: any) => t.builtin_key === 'typst.invoice');
  const doc = await until(async () => {
    const d = (await admin.post(`/crm/items/${deal.id}/documents`, { template_id: tpl.id })).data;
    return d.status === 'ready' ? d : null;
  }, 20_000);
  ok('signer phones are checked', (await admin.post(`/crm/documents/${doc.id}/signature`, { signers: [{ name: 'A', email: `a.${s}@example.com`, phone: 'nope' }] })).status === 400);
  const n2 = tw.sent.length;
  r = await admin.post(`/crm/documents/${doc.id}/signature`, { signers: [{ name: `Sig ${s}`, email: `sig.${s}@example.com`, phone: local(seed + 2) }, { name: `Mail ${s}`, email: `mail.${s}@example.com` }] });
  ok('sending for signature with a phone', r.status === 200 && r.data.signers.some((x: any) => x.phone === intl(seed + 2)), r.data);
  await waitFor(tw.sent, n2 + 1);
  const sg = tw.sent.slice(n2);
  ok('…texts that signer their link (only them)', sg.length === 1 && sg[0].to === intl(seed + 2) && /\/sign\/[A-Za-z0-9_-]+$/.test(sg[0].body), sg);

  // ---- campaigns ----
  await admin.post('/crm/contact/items', { first_name: 'Bob', last_name: `Tx${s}`, multifields: [{ kind: 'phone', value: intl(seed + 3) }] });
  await admin.post('/crm/contact/items', { first_name: 'Cy', last_name: `Tx${s}`, multifields: [{ kind: 'phone', value: intl(seed + 4) }] });
  await db.query('INSERT INTO sms_optouts (number) VALUES ($1) ON CONFLICT DO NOTHING', [intl(seed + 4)]);
  const seg = (await admin.post('/marketing/segments', { name: `Tx ${s}`, entity_type: 3, filter: { q: `Tx${s}` } })).data;
  made.segments.push(seg.id);
  r = await admin.post('/marketing/campaigns', { name: `Sale ${s}`, channel: 'sms', body: '{{first_name}}, the sale starts tomorrow', segment_ids: [seg.id] });
  ok('an SMS campaign', r.status === 200 && r.data.channel === 'sms', r.data);
  const camp = r.data;
  made.campaigns.push(camp.id);
  ok('channels are checked', (await admin.post('/marketing/campaigns', { name: `X ${s}`, channel: 'fax' })).status === 400);
  const n3 = tw.sent.length;
  r = await admin.post(`/marketing/campaigns/${camp.id}/test`, { phone: intl(seed + 5) });
  await waitFor(tw.sent, n3 + 1);
  ok('a test goes to the given phone', r.status === 200 && tw.sent.slice(n3).some((x) => x.to === intl(seed + 5) && x.body.startsWith('[Test] ')), tw.sent.slice(n3));
  const n4 = tw.sent.length;
  r = await admin.post(`/marketing/campaigns/${camp.id}/send`, {});
  ok('sending it', r.status === 200, r.data);
  await waitFor(tw.sent, n4 + 2);
  await sleep(500);
  const got = tw.sent.slice(n4);
  ok('…everyone with a phone gets it once, opted-out numbers don\'t', got.length === 2 && got.some((x) => x.to === intl(seed)) && got.some((x) => x.to === intl(seed + 3)) && !got.some((x) => x.to === intl(seed + 4)), got.map((x) => x.to));
  ok('…filled in, with how to opt out', got.some((x) => x.body.startsWith(`Bob, the sale starts tomorrow\n`) && /reply STOP to opt out$/.test(x.body)), got.map((x) => x.body));
  ok('…the recipients are linked to the campaign', (await db.query('SELECT COUNT(*)::int AS n FROM sms_messages WHERE campaign_id = $1', [camp.id])).rows[0].n === 2);

  // ---- invitations ----
  const n5 = tw.sent.length;
  r = await admin.post('/users', { email: `inv.${s}@acme.test`, first_name: 'Ivy', last_name: `Inv${s}`, phone: local(seed + 6), send_invite: true, invite_by_text: true });
  ok('inviting by text', r.status === 200, r.data);
  await waitFor(tw.sent, n5 + 1);
  ok('…texts the invitation link', tw.sent.slice(n5).some((x) => x.to === intl(seed + 6) && /\/accept-invite\?token=/.test(x.body)), tw.sent.slice(n5));
  ok('inviting by text needs a number', (await admin.post('/users', { email: `inv2.${s}@acme.test`, first_name: 'No', last_name: `Inv${s}`, send_invite: true, invite_by_text: true })).status === 400);

  // ---- the log ----
  r = await admin.get(`/sms/messages?q=${encodeURIComponent(`Ann ${s}`)}`);
  ok('the message log, searchable', r.status === 200 && r.data.messages.some((x: any) => x.id === firstId), r.data);
  ok('…for administrators only', (await tex.get('/sms/messages')).status === 403);
} finally {
  await admin.put('/sms/setup', { from: before.from ?? '', country_code: before.country_code ?? '' }).catch(() => {});
  await admin.put('/booking/settings', { sms_notices: bookingBefore.sms_notices ?? false, confirm_email: bookingBefore.confirm_email ?? true }).catch(() => {});
  for (const id of made.campaigns) await db.query('DELETE FROM mkt_campaigns WHERE id = $1', [id]).catch(() => {});
  for (const id of made.segments) await db.query('DELETE FROM mkt_segments WHERE id = $1', [id]).catch(() => {});
  for (const id of made.robots) await db.query('DELETE FROM crm_robots WHERE id = $1', [id]).catch(() => {});
  for (const id of made.resources) await db.query('DELETE FROM booking_resources WHERE id = $1', [id]).catch(() => {});
  await db.query("DELETE FROM sms_messages WHERE body LIKE $1 OR provider_sid LIKE $2 OR to_number LIKE '+44207946%' OR from_number = '+447700900999' OR to_number = $3", [`%${s}%`, `SMin%${s}`, INVALID]).catch(() => {});
  await db.query("DELETE FROM sms_optouts WHERE number LIKE '+44207946%'").catch(() => {});
  await cleanupRun(s);
  for (const id of made.categories) await db.query('DELETE FROM crm_categories WHERE id = $1', [id]).catch(() => {});
  await db.end();
  await tw.close();
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
