// Capacity-aware assignment: the assign robot's "capacity" mode gives a record to whoever is in
// today (not away, on the rota, optionally checked in), fits it (skills against a field of the
// record) and has the fewest open tasks and records; the preview explains each person; skills
// live on the profile.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const tz = (await db.query("SELECT value #>> '{}' AS v FROM settings WHERE key = 'default_timezone'")).rows[0]?.v ?? 'UTC';
const today = new Intl.DateTimeFormat('en-CA', { timeZone: tz }).format(new Date());
const made: Record<string, any> = { robots: [] };
const holiday = (await db.query('SELECT 1 FROM hr_holidays WHERE day = $1', [today])).rowCount;

try {
  if (holiday) console.log('skip: today is a company holiday');
  const mk = async (first: string) => (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `C${s}` })).data;
  const [ada, bo, cy, di, ed] = [await mk('Ada'), await mk('Bo'), await mk('Cy'), await mk('Di'), await mk('Ed')];
  const name = (u: any) => `${u.first_name} C${s}`;
  // Everyone but Ed works today by the rota (so a weekend doesn't matter); Ed is on the rota
  // another day this week only; Bo is on leave.
  made.shift = (await db.query("INSERT INTO hr_shifts (name, start_min, end_min, break_minutes, color, is_active) VALUES ($1, 540, 1020, 60, '#888', TRUE) RETURNING id", [`Day ${s}`])).rows[0].id;
  for (const u of [ada, bo, cy, di]) await db.query('INSERT INTO hr_rota (user_id, day, shift_id) VALUES ($1, $2, $3)', [u.id, today, made.shift]);
  const weekStart = (() => { const d = new Date(today + 'T12:00:00Z'); d.setUTCDate(d.getUTCDate() - ((d.getUTCDay() + 6) % 7)); return d; })();
  const edDay = new Date(weekStart); edDay.setUTCDate(edDay.getUTCDate() + (today === weekStart.toISOString().slice(0, 10) ? 1 : 0));
  await db.query('INSERT INTO hr_rota (user_id, day, shift_id) VALUES ($1, $2, $3)', [ed.id, edDay.toISOString().slice(0, 10), made.shift]);
  await db.query("INSERT INTO hr_absences (user_id, kind, starts_on, ends_on, status) VALUES ($1, 'vacation', $2, $2, 'approved')", [bo.id, today]);
  // Cy has three open tasks, Di two, Ada none.
  for (const [u, n] of [[cy, 3], [di, 2]] as const) for (let i = 0; i < n; i++) await db.query("INSERT INTO tasks (title, created_by, responsible_id, status) VALUES ($1, $2, $2, 'new')", [`Busy ${s}`, u.id]);
  // Skills, on the profile.
  ok('one\'s own skills', (await (async () => { const c = new Client(); await c.post('/auth/login', { email: `di.${s}@acme.test`, password: 'password123' }); const r = await c.put(`/users/${di.id}/skills`, { skills: ['German', ' german ', 'Solar  panels'] }); const other = await c.put(`/users/${ada.id}/skills`, { skills: ['x'] }); return r.data.skills.join('|') === 'German|Solar panels' && other.status === 403; })()));
  ok('skills show on the profile', (await admin.get(`/users/${di.id}/skills`)).data.skills.length === 2);

  const users = [ada, bo, cy, di, ed].map((u) => u.id).join(',');
  const preview = (p: Record<string, string> = {}) => admin.get('/crm/assign/preview?' + new URLSearchParams({ type: 'lead', users, ...p }));
  let r = await preview();
  const who = (x: any, u: any) => x.people.find((p: any) => p.id === u.id);
  if (!holiday) {
    ok('the person in today with the least on their plate', r.status === 200 && r.data.pick === ada.id, r.data);
    ok('…not the one on leave', who(r.data, bo).in_today === false && /away/.test(who(r.data, bo).why), who(r.data, bo));
    ok('…nor one not on the rota today', who(r.data, ed).in_today === false && /rota/.test(who(r.data, ed).why), who(r.data, ed));
    ok('…and how busy each is', who(r.data, cy).open_tasks === 3 && who(r.data, cy).eligible === true);
    r = await preview({ max_open: '3' });
    ok('a cap skips the busiest', who(r.data, cy).eligible === false && /cap/.test(who(r.data, cy).why), who(r.data, cy));

    // Skills against a field of the record.
    const german = (await admin.post('/crm/lead/items', { title: `German lead ${s}`, source_details: 'German' })).data;
    const klingon = (await admin.post('/crm/lead/items', { title: `Klingon lead ${s}`, source_details: 'Klingon' })).data;
    made.leads = [german.id, klingon.id];
    r = await preview({ item: german.id, skill_field: 'source_details' });
    ok('someone with the skill comes first, though busier', r.data.pick === di.id && who(r.data, di).fits === true && who(r.data, ada).eligible === false, r.data);
    r = await preview({ item: klingon.id, skill_field: 'source_details' });
    ok('nobody has the skill: the least busy who is in', r.data.pick === ada.id, r.data);
    r = await preview({ item: klingon.id, skill_field: 'source_details', skills_required: 'true' });
    ok('…unless the skill is required: then the least busy of all', r.data.pick === ada.id && /nobody available/.test(r.data.how), r.data);
    r = await preview({ item: klingon.id, skill_field: 'source_details', skills_required: 'true', if_nobody: 'keep' });
    ok('…or nobody, if the rule says so', r.data.pick === null, r.data);

    // Checked in.
    await db.query("INSERT INTO hr_work_days (user_id, day, started_at) VALUES ($1, $2, now())", [cy.id, today]);
    r = await preview({ checked_in: 'true' });
    ok('only those who checked in', r.data.pick === cy.id && /hasn't checked in/.test(who(r.data, ada).why), r.data);

    // ---- the robot ----
    const cat = (await admin.get('/crm/config')).data.types.find((t: any) => t.code === 'lead').categories.find((c: any) => c.is_default);
    const first = cat.stages[0];
    ok('a capacity rule needs sensible settings', (await admin.post('/crm/robots', { category_id: cat.id, stage_id: first.id, action: 'assign', params: { users: [ada.id], mode: 'capacity', max_open: 0 } })).status === 400);
    const rb = await admin.post('/crm/robots', { category_id: cat.id, stage_id: first.id, name: `Cap ${s}`, action: 'assign', params: { users: [ada.id, bo.id, cy.id, di.id, ed.id], mode: 'capacity', skill_field: 'source_details' }, conditions: [{ field: 'title', op: 'contains', value: `Robot ${s}` }] });
    ok('a capacity rule is saved', rb.status === 200 && rb.data.params?.mode === 'capacity' && rb.data.params?.skill_field === 'source_details', rb.data);
    made.robots.push(rb.data.id);
    const lead = (await admin.post('/crm/lead/items', { title: `Robot ${s}`, source_details: 'german' })).data;
    made.leads.push(lead.id);
    let run: any;
    for (let i = 0; i < 60 && !(run && ['done', 'failed'].includes(run.status)); i++) {
      run = (await db.query('SELECT status, result FROM crm_robot_runs WHERE item_id = $1 AND robot_name = $2', [lead.id, `Cap ${s}`])).rows[0];
      await sleep(300);
    }
    const now = (await admin.get(`/crm/lead/items/${lead.id}`)).data;
    ok('the robot gives it to the right person, and says why', run?.status === 'done' && now.assigned_to === di.id && run.result.includes(name(di)) && /skill/.test(run.result), [run, now.assigned_to]);
  }
  const u = new Client();
  await u.post('/auth/login', { email: `ed.${s}@acme.test`, password: 'password123' });
  ok('others\' skills are for those who manage employees', (await u.put(`/users/${ada.id}/skills`, { skills: ['Hacking'] })).status === 403);
} finally {
  for (const id of made.robots) await admin.del(`/crm/robots/${id}`).catch(() => {});
  await db.query('DELETE FROM tasks WHERE title = $1', [`Busy ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title LIKE $1", [`% ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title = $1", [`Robot ${s}`]).catch(() => {});
  if (made.shift) { await db.query('DELETE FROM hr_rota WHERE shift_id = $1', [made.shift]).catch(() => {}); await db.query('DELETE FROM hr_shifts WHERE id = $1', [made.shift]).catch(() => {}); }
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
