// Booking: resources with working hours and capacity, services and extras, staff bookings
// (conflicts, overbooking, moving, statuses), the waiting list, a public self-booking form
// (free slots, booking, the client's confirm/cancel link), the CRM contact and deal, and
// bookings on the employee's calendar.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const B = 'http://localhost:8080/api';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Book${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [mia, sam] = await Promise.all(['Mia', 'Sam'].map(user));
const pub = async (method: string, path: string, body?: unknown) => {
  const r = await fetch(B + path, { method, headers: body ? { 'content-type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined });
  const t = await r.text();
  let data: any; try { data = JSON.parse(t); } catch { data = t; }
  return { status: r.status, data };
};
// The next Monday, at a UTC hour.
const monday = (() => { const d = new Date(); d.setUTCHours(0, 0, 0, 0); d.setUTCDate(d.getUTCDate() + ((8 - d.getUTCDay()) % 7 || 7)); return d; })();
const at = (h: number, m = 0, days = 0) => new Date(+monday + days * 86400e3 + (h * 60 + m) * 60e3).toISOString().replace('.000Z', 'Z');
const day = monday.toISOString().slice(0, 10);

try {
  // ---- set-up ----
  ok('setting up booking is for those who manage it', (await sam.post('/booking/resources', { name: 'x' })).status === 403);
  let r = await admin.post('/booking/resources', { kind: 'person', user_id: mia.id, timezone: 'UTC', slot_minutes: 30, min_notice_minutes: 0 });
  const miaRes = r.data;
  ok('a person resource, named after the employee, with office hours', r.status === 200 && miaRes.name === `Mia Book${stamp}` && miaRes.hours.length === 5 && miaRes.hours[0].start_min === 540, r.data);
  const room = (await admin.post('/booking/resources', { name: `Room ${stamp}`, kind: 'room', capacity: 2, allow_overbooking: true, timezone: 'UTC', min_notice_minutes: 0 })).data;
  ok('working hours are checked', (await admin.put(`/booking/resources/${miaRes.id}/hours`, { hours: [{ weekday: 1, start_min: 600, end_min: 540 }] })).status === 400);
  r = await admin.put(`/booking/resources/${miaRes.id}/hours`, { hours: [{ weekday: 1, start_min: 540, end_min: 720 }, { weekday: 1, start_min: 780, end_min: 1020 }, { weekday: 2, start_min: 540, end_min: 1020 }] });
  ok('hours with a lunch break', r.data.hours.length === 3, r.data.hours);
  r = await admin.post('/booking/services', { name: `Haircut ${stamp}`, duration_minutes: 60, price: '30.00', resource_ids: [miaRes.id] });
  const cut = r.data.items.find((s: any) => s.name === `Haircut ${stamp}`);
  const towel = (await admin.post('/booking/services', { name: `Towel ${stamp}`, duration_minutes: 5, price: '2.50', is_extra: true })).data.items.find((s: any) => s.name === `Towel ${stamp}`);
  ok('a service and an extra', cut?.price === '30.00' && towel?.is_extra, r.data);

  // ---- slots ----
  r = await sam.get(`/booking/slots?resource_id=${miaRes.id}&service_id=${cut.id}&date=${day}`);
  const slots: string[] = r.data.slots;
  ok('free slots skip the lunch break', slots.length > 0 && slots.includes(at(9)) && slots.includes(at(11)) && !slots.includes(at(11, 30)) && !slots.includes(at(12)) && slots.includes(at(13)) && !slots.includes(at(16, 30)), slots);

  // ---- staff bookings ----
  r = await sam.post('/booking/bookings', { resource_ids: [miaRes.id], service_id: cut.id, extra_ids: [towel.id], starts_at: at(10), client_name: `Carla ${stamp}`, client_email: `carla.${stamp}@example.com`, client_phone: '+1 555 0100' });
  const b1 = r.data;
  ok('anyone books: the end and price come from the service and extras', r.status === 200 && b1.ends_at === at(11) && b1.price === '32.50' && b1.status === 'booked' && b1.number > 0, r.data);
  ok('the client becomes a CRM contact', !!b1.contact_id);
  const contact = (await admin.get(`/crm/contact/items/${b1.contact_id}`)).data;
  ok('with their email and phone', JSON.stringify(contact).includes(`carla.${stamp}@example.com`), contact);
  r = await sam.post('/booking/bookings', { resource_ids: [miaRes.id], service_id: cut.id, starts_at: at(10, 30), client_name: `Dan ${stamp}` });
  ok('a clash is refused', r.status === 409, r.data);
  r = await sam.post('/booking/bookings', { resource_ids: [miaRes.id], service_id: cut.id, starts_at: at(10, 30), client_name: `Dan ${stamp}`, overbook: true });
  ok('no overbooking where the resource doesn\'t allow it', r.status === 409);
  r = await sam.get(`/booking/slots?resource_id=${miaRes.id}&service_id=${cut.id}&date=${day}`);
  ok('the booked time is no longer free', !r.data.slots.includes(at(10)) && !r.data.slots.includes(at(9, 30)), r.data.slots);
  const again = (await sam.post('/booking/bookings', { resource_ids: [miaRes.id, room.id], starts_at: at(14), ends_at: at(15), client_name: `Eve ${stamp}`, client_email: `carla.${stamp}@example.com` })).data;
  ok('one booking takes a person and a room; a returning client is found again', again.resource_ids.length === 2 && again.contact_id === b1.contact_id, again);
  await sam.post('/booking/bookings', { resource_ids: [room.id], starts_at: at(14), ends_at: at(15), client_name: `Fay ${stamp}` });
  ok('the room holds two at once, not three', (await sam.post('/booking/bookings', { resource_ids: [room.id], starts_at: at(14), ends_at: at(15), client_name: `Gus ${stamp}` })).status === 409);
  r = await sam.post('/booking/bookings', { resource_ids: [room.id], starts_at: at(14), ends_at: at(15), client_name: `Gus ${stamp}`, overbook: true });
  ok('staff overbook where it is allowed', r.status === 200 && r.data.overbooked, r.data);

  // ---- moving, statuses, waiting list ----
  r = await sam.patch(`/booking/bookings/${b1.id}`, { starts_at: at(14) });
  ok('moving onto a taken time is refused', r.status === 409);
  r = await sam.patch(`/booking/bookings/${b1.id}`, { starts_at: at(9) });
  ok('moved, keeping its length', r.status === 200 && r.data.ends_at === at(10));
  const w = (await sam.post('/booking/bookings', { resource_ids: [miaRes.id], service_id: cut.id, starts_at: at(9), client_name: `Hal ${stamp}`, waitlist: true })).data;
  ok('the waiting list for a taken time', w.status === 'waitlist' && (await sam.get('/booking/waitlist')).data.items.some((x: any) => x.id === w.id));
  ok('it can\'t be placed while the time is taken', (await sam.post(`/booking/bookings/${w.id}/status`, { status: 'booked' })).status === 409);
  const before = (await mia.get('/notifications?limit=50')).data;
  await sam.post(`/booking/bookings/${b1.id}/status`, { status: 'cancelled' });
  const after = (await mia.get('/notifications?limit=50')).data;
  ok('cancelling tells the resource\'s person the time is free for the waiting list', JSON.stringify(after).includes(`Hal ${stamp}`) && !JSON.stringify(before).includes('waits for is free'), after.items?.slice(0, 3));
  ok('now it can be placed', (await sam.post(`/booking/bookings/${w.id}/status`, { status: 'booked' })).data.status === 'booked');
  ok('came / no-show', (await sam.post(`/booking/bookings/${again.id}/status`, { status: 'came' })).data.status === 'came' && (await sam.post(`/booking/bookings/${again.id}/status`, { status: 'teleported' })).status === 400);

  // ---- on the employee's calendar ----
  r = await mia.get(`/calendar?from=${at(0)}&to=${at(0, 0, 1)}`);
  ok('bookings show on the person\'s calendar', r.data.items.some((e: any) => e.kind === 'booking' && e.title.includes(`Hal ${stamp}`)), r.data.items.map((e: any) => e.title));

  // ---- a deal per booking ----
  const cat = (await admin.get('/crm/config')).data.types.find((t: any) => t.code === 'deal').categories[0];
  await admin.put('/booking/settings', { deal_category_id: cat.id, currency: 'EUR' });
  r = await sam.post('/booking/bookings', { resource_ids: [miaRes.id], service_id: cut.id, starts_at: at(15), client_name: `Ida ${stamp}`, client_email: `ida.${stamp}@example.com` });
  ok('with a pipeline set, each booking gets a deal', !!r.data.deal_id, r.data);
  const deal = (await admin.get(`/crm/deal/items/${r.data.deal_id}`)).data;
  ok('the deal: the client, the price, the person booked', deal.title?.includes(`Ida ${stamp}`) && Number(deal.opportunity) === 30 && deal.currency === 'EUR' && deal.assigned_to === mia.id, deal);
  await admin.put('/booking/settings', { deal_category_id: null, currency: 'USD' });

  // ---- the public form ----
  r = await admin.post('/booking/forms', { name: `Salon ${stamp}`, service_ids: [cut.id], phone_required: true, thanks: 'See you!' });
  const form = r.data.items.find((f: any) => f.name === `Salon ${stamp}`);
  ok('a self-booking form with its own address', !!form?.slug, r.data);
  r = await pub('GET', `/public/booking/${form.slug}`);
  ok('the public form: its services (and extras), who can be booked', r.status === 200 && r.data.services.some((s: any) => s.id === cut.id) && r.data.services.some((s: any) => s.id === towel.id) && r.data.resources.some((x: any) => x.id === miaRes.id), r.data);
  const tue = new Date(+monday + 86400e3).toISOString().slice(0, 10);
  r = await pub('GET', `/public/booking/${form.slug}/slots?service_id=${cut.id}&date=${tue}`);
  const first = r.data.slots[0];
  ok('free times, with who is free', first?.resource_ids.includes(miaRes.id) && first.start === at(9, 0, 1), r.data);
  ok('the phone is required by this form', (await pub('POST', `/public/booking/${form.slug}`, { service_id: cut.id, starts_at: first.start, name: 'Jo', email: `jo.${stamp}@example.com` })).status === 400);
  ok('bots filling the hidden field are refused', (await pub('POST', `/public/booking/${form.slug}`, { service_id: cut.id, starts_at: first.start, name: 'Jo', email: `jo.${stamp}@example.com`, phone: '1', website: 'spam' })).status === 400);
  r = await pub('POST', `/public/booking/${form.slug}`, { service_id: cut.id, starts_at: first.start, name: `Jo ${stamp}`, email: `jo.${stamp}@example.com`, phone: '+1 555 0111' });
  ok('a client books themselves', r.status === 200 && r.data.status === 'booked' && r.data.thanks === 'See you!' && r.data.manage_url.includes('/book/manage/'), r.data);
  const token = r.data.manage_url.split('/').pop();
  ok('the same time can\'t be taken twice', (await pub('POST', `/public/booking/${form.slug}`, { service_id: cut.id, starts_at: first.start, name: `Kim ${stamp}`, email: `kim.${stamp}@example.com`, phone: '1' })).status === 409);
  r = await pub('POST', `/public/booking/${form.slug}`, { service_id: cut.id, starts_at: first.start, name: `Kim ${stamp}`, email: `kim.${stamp}@example.com`, phone: '1', waitlist: true });
  ok('or the client joins the waiting list', r.status === 200 && r.data.status === 'waitlist', r.data);
  ok('a time outside working hours isn\'t bookable', (await pub('POST', `/public/booking/${form.slug}`, { service_id: cut.id, starts_at: at(20, 0, 1), name: `Lee ${stamp}`, email: `lee.${stamp}@example.com`, phone: '1' })).status === 409);
  r = await pub('GET', `/public/booking/manage/${token}`);
  ok('the client\'s link shows the booking', r.status === 200 && r.data.status === 'booked' && r.data.with.includes(`Mia Book${stamp}`) && r.data.can_change, r.data);
  ok('confirm it', (await pub('POST', `/public/booking/manage/${token}`, { action: 'confirm' })).data.status === 'confirmed');
  ok('then cancel it', (await pub('POST', `/public/booking/manage/${token}`, { action: 'cancel' })).data.status === 'cancelled');
  ok('a cancelled booking stays cancelled', (await pub('POST', `/public/booking/manage/${token}`, { action: 'confirm' })).status === 400);
  ok('a wrong link finds nothing', (await pub('GET', '/public/booking/manage/nope')).status === 404);
  await admin.patch(`/booking/forms/${form.id}`, { is_active: false });
  ok('a switched-off form is gone', (await pub('GET', `/public/booking/${form.slug}`)).status === 404);

  // ---- days off ----
  await admin.put(`/booking/resources/${miaRes.id}/days-off`, { days: [{ day: tue, note: 'Training' }] });
  ok('nothing free on a day off', (await sam.get(`/booking/slots?resource_id=${miaRes.id}&service_id=${cut.id}&date=${tue}`)).data.slots.length === 0);
  ok('a resource with bookings to come can\'t be deleted', (await admin.del(`/booking/resources/${miaRes.id}`)).status === 400);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
