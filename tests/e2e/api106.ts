// Booking inside CRM: booking from a deal (linked to it and its contact), a record's bookings
// (a contact's, a deal's, a company's), booking events as automation triggers (the deal moves
// when the client came), several reminders (only the nearest sent), a follow-up after the
// visit and a message to no-shows (each once), and the settings' checks.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const before = (await admin.get('/booking/settings')).data;
const MP = 'http://localhost:8026/api/v1';
const mails = async (to: string, subject: string) => (((await (await fetch(`${MP}/search?query=${encodeURIComponent(`to:${to} subject:"${subject}"`)}`)).json()).messages) ?? []) as any[];
const runJob = () => db.query(`INSERT INTO jobs (kind, payload) VALUES ('booking.remind', '{}')`);
const minutes = (m: number) => new Date(Date.now() + m * 60e3).toISOString();

try {
  await admin.put('/booking/settings', { confirm_email: false, deal_category_id: null, remind_hours: 24, extra_reminders: [], follow_up_hours: 0, follow_up_text: '', no_show_text: '' });
  const room = (await admin.post('/booking/resources', { name: `Studio ${s}`, kind: 'room', capacity: 5, timezone: 'UTC', min_notice_minutes: 0 })).data;
  await admin.put(`/booking/resources/${room.id}/hours`, { hours: [1, 2, 3, 4, 5, 6, 7].map((weekday) => ({ weekday, start_min: 0, end_min: 1440 })) });
  const kimEmail = `kim.${s}@example.com`;
  const acme = (await admin.post('/crm/company/items', { company_name: `Acme ${s}` })).data;
  const kim = (await admin.post('/crm/contact/items', { first_name: 'Kim', last_name: `Book${s}`, company_id: acme.id, multifields: [{ kind: 'email', value_type: 'work', value: kimEmail }] })).data;
  const pipe = (await admin.post('/crm/deal/categories', { name: `Visits ${s}` })).data.categories.find((c: any) => c.name === `Visits ${s}`);
  const later = pipe.stages.filter((x: any) => x.semantics === 'process').at(-1);
  const deal = (await admin.post('/crm/deal/items', { title: `Course ${s}`, category_id: pipe.id, contact_ids: [kim.id], company_id: acme.id })).data;

  // ---- the trigger: the client came → the deal moves on ----
  let r = await admin.post('/crm/triggers', { category_id: pipe.id, stage_id: later.id, name: `Came ${s}`, event: 'booking', params: { status: 'came' }, allow_backwards: false });
  ok('a booking trigger (when the client came)', r.status === 200, r.data);
  ok('…its status is checked', (await admin.post('/crm/triggers', { category_id: pipe.id, stage_id: later.id, event: 'booking', params: { status: 'late' } })).status === 400);

  // ---- booking from the deal ----
  ok('a contact isn\'t a deal', (await admin.post('/booking/bookings', { resource_ids: [room.id], starts_at: minutes(90), ends_at: minutes(120), client_name: 'Kim', deal_id: kim.id })).status === 400);
  r = await admin.post('/booking/bookings', { resource_ids: [room.id], starts_at: minutes(90), ends_at: minutes(120), client_name: `Kim Book${s}`, client_email: kimEmail, contact_id: kim.id, deal_id: deal.id });
  const b1 = r.data;
  ok('booked from the deal: linked to it and its contact', r.status === 200 && b1.deal_id === deal.id && b1.contact_id === kim.id, r.data);
  const list = async (id: string) => (await admin.get(`/crm/items/${id}/bookings`)).data.items.map((x: any) => x.id);
  ok('the deal\'s, the contact\'s and the company\'s bookings', (await list(deal.id)).includes(b1.id) && (await list(kim.id)).includes(b1.id) && (await list(acme.id)).includes(b1.id));
  const tl = (await admin.get(`/crm/items/${deal.id}/timeline`)).data.items;
  ok('the deal\'s timeline notes it', tl.some((e: any) => e.kind === 'booking' && e.data.booking_id === b1.id && e.data.event === 'booked'), tl.map((e: any) => e.kind));

  // ---- reminders: a day and two hours before; 90 minutes ahead only the nearest goes ----
  ok('more reminders are checked', (await admin.put('/booking/settings', { extra_reminders: [0] })).status === 400 && (await admin.put('/booking/settings', { follow_up_hours: 1000 })).status === 400);
  r = await admin.put('/booking/settings', { remind_hours: 24, extra_reminders: [2] });
  ok('a second reminder, two hours before', r.status === 200 && r.data.extra_reminders.includes(2), r.data);
  await runJob();
  const reminded = await waitFor(async () => (await mails(kimEmail, 'Reminder')).length > 0 ? true : null, 20000);
  const notices = (await db.query('SELECT kind FROM booking_notices WHERE booking_id = $1 ORDER BY kind', [b1.id])).rows.map((x) => x.kind);
  ok('only the nearest reminder is sent (the day-before one counts as sent)', reminded && notices.join(',') === 'remind:2,remind:24', notices);
  await runJob();
  await new Promise((res) => setTimeout(res, 3000));
  ok('…once', (await mails(kimEmail, 'Reminder')).length === 1);

  // ---- the client came: the deal moves; a follow-up after the visit ----
  r = await admin.post(`/booking/bookings/${b1.id}/status`, { status: 'came' });
  const moved = await waitFor(async () => { const d = (await admin.get(`/crm/deal/items/${deal.id}`)).data; return d.stage_id === later.id ? d : null; }, 15000);
  ok('the trigger moved the deal', !!moved, (await admin.get(`/crm/deal/items/${deal.id}`)).data.stage_id);
  await admin.put('/booking/settings', { follow_up_hours: 1, follow_up_text: `Thanks for coming ${s}!\nSee you again.` });
  await db.query("UPDATE bookings SET starts_at = now() - interval '3 hours', ends_at = now() - interval '2 hours' WHERE id = $1", [b1.id]);
  await runJob();
  const thanks = await waitFor(async () => (await mails(kimEmail, 'Thank you for your visit'))[0], 20000);
  const text = thanks ? (await (await fetch(`${MP}/message/${thanks.ID}`)).json()).Text : '';
  ok('a follow-up after the visit, with the message', !!thanks && text.includes(`Thanks for coming ${s}!`), text.slice(0, 300));
  await runJob();
  await new Promise((res) => setTimeout(res, 2000));
  ok('…once', (await mails(kimEmail, 'Thank you for your visit')).length === 1);

  // ---- no-shows ----
  const lee = `lee.${s}@example.com`;
  const b2 = (await admin.post('/booking/bookings', { resource_ids: [room.id], starts_at: minutes(300), ends_at: minutes(330), client_name: `Lee Book${s}`, client_email: lee })).data;
  await admin.put('/booking/settings', { no_show_text: 'Book another time with the link we sent.' });
  await admin.post(`/booking/bookings/${b2.id}/status`, { status: 'no_show' });
  await runJob();
  ok('a message to a client who didn\'t come', !!(await waitFor(async () => (await mails(lee, 'We missed you'))[0], 20000)));
  ok('the no-show doesn\'t move the deal-less booking anywhere (no deal)', b2.deal_id === null);
} finally {
  await admin.put('/booking/settings', { remind_hours: before.remind_hours ?? 24, extra_reminders: before.extra_reminders ?? [], follow_up_hours: before.follow_up_hours ?? 0, follow_up_text: before.follow_up_text ?? '', no_show_text: before.no_show_text ?? '', confirm_email: before.confirm_email ?? true, deal_category_id: before.deal_category_id ?? null }).catch(() => {});
  await db.query("DELETE FROM bookings WHERE client_name LIKE $1", [`%${s}`]).catch(() => {});
  await db.query("DELETE FROM booking_resources WHERE name = $1", [`Studio ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_categories WHERE name = $1", [`Visits ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
