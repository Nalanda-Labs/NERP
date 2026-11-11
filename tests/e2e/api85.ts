// Open slots from a CRM record: a booking link made out for the record's client; what they
// book lands on the record as a meeting (on the calendar once, as the event), and cancelling
// it closes the meeting. Tokens are tied to the link and the record.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const B = 'http://localhost:8080/api';
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Bk${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const pub = async (method: string, path: string, body?: unknown) => {
  const r = await fetch(B + path, { method, headers: body ? { 'content-type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined });
  const t = await r.text();
  let data: any; try { data = JSON.parse(t); } catch { data = t; }
  return { status: r.status, data };
};
const made: string[] = [];

try {
  const mia = await mk('Mia'), sam = await mk('Sam');
  const allDay = [1, 2, 3, 4, 5, 6, 7].map((d) => [d, 0, 1440]);
  const link = (await mia.post('/calendar/slot-links', { title: `Demo ${stamp}`, hours: allDay, timezone: 'UTC', duration_minutes: 30, slot_step_minutes: 30, min_notice_minutes: 0, days_ahead: 30 })).data;
  const deal = (await mia.post('/crm/deal/items', { title: `Booked deal ${stamp}`, assigned_to: mia.id })).data;
  const other = (await mia.post('/crm/deal/items', { title: `Other deal ${stamp}`, assigned_to: mia.id })).data;
  made.push(deal.id, other.id);

  let r = await mia.post(`/calendar/slot-links/${link.id}/for-record`, { item_id: deal.id });
  ok('a booking link made out for a record', r.status === 200 && r.data.url.includes(`/meet/${link.slug}?r=`), r.data);
  const token = new URL(r.data.url).searchParams.get('r')!;
  ok('only the link\'s people make them', (await sam.post(`/calendar/slot-links/${link.id}/for-record`, { item_id: deal.id })).status === 404);
  ok('only for records they may read', (await mia.post(`/calendar/slot-links/${link.id}/for-record`, { item_id: '00000000-0000-4000-8000-000000000000' })).status === 404);

  const day = new Date(Date.now() + 3 * 86400e3).toISOString().slice(0, 10);
  const slots = (await pub('GET', `/public/calendar/meet/${link.slug}/slots?from=${day}&to=${day}`)).data.days[0].slots as string[];
  const start = slots[20];
  r = await pub('POST', `/public/calendar/meet/${link.slug}`, { starts_at: start, name: `Client ${stamp}`, email: `client.${stamp}@example.com`, r: token });
  ok('the client books through it', r.status === 200, r.data);
  const manage = r.data.manage_url as string;
  const list = (await mia.get(`/crm/activities?item_id=${deal.id}&state=all`)).data.items as any[];
  const meeting = list.find((a: any) => a.kind === 'meeting' && a.subject.includes(`Client ${stamp}`));
  ok('the meeting lands on the record', !!meeting && new Date(meeting.starts_at).getTime() === new Date(start).getTime() && meeting.responsible_id === mia.id, list);
  const tl = (await mia.get(`/crm/items/${deal.id}/timeline`)).data.items;
  ok('…and in its timeline', tl.some((e: any) => e.kind === 'booking' && e.data.name === `Client ${stamp}`));
  const from = new Date(new Date(start).getTime() - 3600e3).toISOString(), to = new Date(new Date(start).getTime() + 3600e3).toISOString();
  const entries = (await mia.get(`/calendar?from=${from}&to=${to}&extras=true`)).data.items as any[];
  const mine = entries.filter((e: any) => (e.title ?? '').includes(`Client ${stamp}`));
  ok('the calendar shows it once, linked to the record', mine.length === 1 && mine[0].kind !== 'activity', mine);

  // Tokens: another record's, or a forged one, link nothing.
  const otherToken = new URL((await mia.post(`/calendar/slot-links/${link.id}/for-record`, { item_id: other.id })).data.url).searchParams.get('r')!;
  const forged = `${other.id.replace(/-/g, '')}.${token.split('.')[1]}`;
  r = await pub('POST', `/public/calendar/meet/${link.slug}`, { starts_at: slots[22], name: `Forger ${stamp}`, email: `forger.${stamp}@example.com`, r: forged });
  ok('a forged token still books, but on no record', r.status === 200 && !(await mia.get(`/crm/items/${other.id}/timeline`)).data.items.some((e: any) => e.kind === 'booking'), r.data);
  r = await pub('POST', `/public/calendar/meet/${link.slug}`, { starts_at: slots[24], name: `Second ${stamp}`, email: `second.${stamp}@example.com`, r: otherToken });
  ok('each record\'s own token books onto it', r.status === 200 && (await mia.get(`/crm/items/${other.id}/timeline`)).data.items.some((e: any) => e.kind === 'booking' && e.data.name === `Second ${stamp}`));

  // The client cancels.
  const rsvp = manage.match(/\/rsvp\/([^/?#]+)/)?.[1] ?? manage.split('/').pop();
  r = await pub('POST', `/public/calendar/rsvp/${rsvp}`, { status: 'declined' });
  ok('the client cancels', r.status === 200 && r.data.cancelled, r.data);
  const after = ((await mia.get(`/crm/activities?item_id=${deal.id}&state=all`)).data.items as any[]).find((a: any) => a.id === meeting.id);
  ok('…which closes the record\'s meeting as cancelled', after?.is_completed && /Cancelled by/.test(after.result ?? ''), after);
} finally {
  for (const id of made) await admin.del(`/crm/deal/items/${id}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
