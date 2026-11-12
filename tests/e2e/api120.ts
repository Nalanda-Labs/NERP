// Field visits: a day planned around a fixed visit with the others in route order from the
// start point; check-in near or far from the address; check-out logs the visit on the record.
//
// The visits lie on one line of latitude (51.5°N), 0.05° of longitude (≈3.46 km) apart; at
// 30 km/h a step takes 7 minutes. From the start (-0.15) the route is B (-0.10), C (0.00),
// A (0.05); D (-0.05) is fixed at 10:00 for an hour, so C waits until after it.
import { Client } from './lib.ts';
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
const before = (await db.query("SELECT value FROM settings WHERE key = 'field_visits'")).rows[0]?.value ?? null;
const date = new Date(Date.now() + 10 * 86400e3).toISOString().slice(0, 10);
const made: string[] = [];
const minutes = (a: string, b: string) => Math.round((new Date(a).getTime() - new Date(b).getTime()) / 60000);

try {
  await admin.post('/users', { email: `ana.${s}@acme.test`, password: 'password123', first_name: 'Ana', last_name: `F${s}` });
  const ana = new Client();
  const anaUser = (await ana.post('/auth/login', { email: `ana.${s}@acme.test`, password: 'password123' })).data.user;
  ok('field settings are for CRM administrators', (await ana.put('/field/settings', { speed_kmh: 30, day_starts: '09:00' })).status === 403);
  let r = await admin.put('/field/settings', { speed_kmh: 30, start_lat: 51.5, start_lng: -0.15, start_label: 'Office', day_starts: '09:00' });
  ok('a start point and speed', r.status === 200 && r.data.settings.speed_kmh === 30);

  const company = async (name: string, address: any = null) => { const c = (await admin.post('/crm/company/items', { company_name: `${name} ${s}`, ...(address ? { address } : {}) })).data; made.push(c.id); return c; };
  const [a, b, c, d] = [await company('Alpha'), await company('Bravo'), await company('Charlie'), await company('Delta')];
  const visit = async (who: any, lng: number, time?: string, dur = 30) => (await ana.post('/field/visits', { item_id: who.id, day: date, lat: 51.5, lng, duration_minutes: dur, ...(time ? { time } : {}) })).data;
  const va = await visit(a, 0.05), vb = await visit(b, -0.10), vc = await visit(c, 0.0), vd = await visit(d, -0.05, '10:00', 60);
  ok('visits made, one at a fixed time', va.id && vd.fixed_time === true && va.fixed_time === false && va.has_coordinates === true, [va, vd]);
  ok('a visit lasts at least five minutes', (await ana.post('/field/visits', { item_id: a.id, day: date, duration_minutes: 1 })).status === 400);

  r = await ana.post('/field/day/plan', { date });
  const v = (id: string) => r.data.visits.find((x: any) => x.id === id);
  const order = r.data.visits.map((x: any) => x.id);
  ok('the day in route order around the fixed visit', r.status === 200 && order.join() === [vb.id, vd.id, vc.id, va.id].join(), r.data.visits?.map((x: any) => [x.title, x.starts_at]));
  const D = v(vd.id).starts_at;
  ok('the fixed visit keeps its time', D === vd.starts_at, [D, vd.starts_at]);
  ok('the first visit after the drive from the start', minutes(v(vb.id).starts_at, D) === -53, minutes(v(vb.id).starts_at, D));
  ok('the next one waits until after the fixed visit, plus the drive', minutes(v(vc.id).starts_at, D) === 67, minutes(v(vc.id).starts_at, D));
  ok('…and the last follows it', minutes(v(va.id).starts_at, D) === 104, minutes(v(va.id).starts_at, D));
  ok('legs and totals', v(vb.id).leg_km === 3.5 && v(vb.id).leg_minutes === 7 && r.data.total_km > 13.5 && r.data.total_km < 14.1, [v(vb.id), r.data.total_km]);
  ok('the order is kept', v(vb.id).order === 1 && v(va.id).order === 4);

  // A visit with no coordinates (no geocoder here) goes after those with them.
  const e = await company('Echo', { line1: '1 Nowhere St', city: 'Testville', postal_code: 'T1', country: 'GB' });
  const ve = (await ana.post('/field/visits', { item_id: e.id, day: date })).data;
  ok('the record\'s address is used', ve.has_coordinates === false && (await db.query('SELECT address->>\'city\' AS c FROM field_visits WHERE activity_id = $1', [ve.id])).rows[0].c === 'Testville');
  r = await ana.post('/field/day/plan', { date });
  ok('…and without coordinates it comes last', r.data.visits.at(-1).id === ve.id, r.data.visits.map((x: any) => x.title));

  // ---- check-in and out ----
  ok('only the person visiting checks in', (await admin.post(`/field/visits/${vb.id}/check-in`, { lat: 51.5, lng: -0.10 })).status === 403);
  r = await ana.post(`/field/visits/${vb.id}/check-in`, { lat: 51.5, lng: -0.10, accuracy_m: 12 });
  ok('checked in at the address', r.status === 200 && r.data.distance_m === 0 && r.data.far === false, r.data);
  ok('…once', (await ana.post(`/field/visits/${vb.id}/check-in`, { lat: 51.5, lng: -0.10 })).status === 400);
  r = await ana.post(`/field/visits/${vc.id}/check-in`, { lat: 51.5, lng: 0.015 });
  ok('a check-in a kilometre away is flagged', r.data.far === true && r.data.distance_m > 1000 && r.data.distance_m < 1100, r.data);
  ok('check out only after checking in', (await ana.post(`/field/visits/${va.id}/check-out`, {})).status === 400);
  r = await ana.post(`/field/visits/${vb.id}/check-out`, { notes: `Measured the kitchen ${s}` });
  ok('checked out', r.status === 200 && r.data.done === true);
  const act = (await db.query('SELECT is_completed, result FROM crm_activities WHERE id = $1', [vb.id])).rows[0];
  ok('…the visit is done, with its notes', act.is_completed === true && act.result === `Measured the kitchen ${s}`);
  ok('…and logged on the record', (await db.query("SELECT 1 FROM crm_timeline WHERE item_id = $1 AND body LIKE '%' || $2 || '%'", [b.id, `Measured the kitchen ${s}`])).rowCount === 1);
  r = await ana.get(`/field/day?date=${date}`);
  ok('the day shows what happened', v.call(null, vb.id) && r.data.visits.find((x: any) => x.id === vb.id).checked_out_at && r.data.visits.find((x: any) => x.id === vc.id).in_distance_m > 1000);

  // ---- whose day ----
  ok('a CRM administrator sees anyone\'s day', (await admin.get(`/field/day?date=${date}&user=${anaUser.id}`)).data.visits.length === 5);
  const me = (await admin.get('/auth/me')).data;
  ok('others\' days are for their managers', (await ana.get(`/field/day?date=${date}&user=${(me.user ?? me).id}`)).status === 403);
} finally {
  await db.query("DELETE FROM settings WHERE key = 'field_visits'").catch(() => {});
  if (before) await db.query("INSERT INTO settings (key, value) VALUES ('field_visits', $1)", [before]).catch(() => {});
  await db.query('DELETE FROM crm_items WHERE id = ANY($1)', [made]).catch((e) => console.log('cleanup', e.message));
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
