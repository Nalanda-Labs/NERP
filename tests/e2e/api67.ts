// BI: datasets (NERP's own data, CSV uploads, CSV/JSON from a URL, outside PostgreSQL
// databases), charts built from them, dashboards with periods, filters and presets,
// sharing, public links and CSV downloads.
import pg from 'pg';
import http from 'node:http';
import net from 'node:net';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Bi${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [vic, dan] = await Promise.all(['Vic', 'Dan'].map(user));
const upload = async (c: Client, name: string, content: string, type = 'text/csv') => {
  const fd = new FormData();
  fd.append('file', new Blob([content], { type }), name);
  return (await c.req('POST', '/files', fd)).data;
};

// A little web server for URL datasets.
let served = JSON.stringify({ data: [{ team: 'Red', score: 3 }, { team: 'Blue', score: 5 }, { team: 'Red', score: 4 }] });
const web = http.createServer((_q, s) => { s.writeHead(200, { 'content-type': 'application/json' }); s.end(served); });
await new Promise<void>((r) => web.listen(0, '127.0.0.1', r));
const webUrl = `http://127.0.0.1:${(web.address() as net.AddressInfo).port}/scores.json`;

const created: { dashboards: string[]; datasets: string[]; sources: string[] } = { dashboards: [], datasets: [], sources: [] };
const q = (spec: object, extra: object = {}) => admin.post('/bi/query', { spec, ...extra });

try {
  // ---- datasets ----
  ok('only BI builders see datasets', (await vic.get('/bi/datasets')).status === 403);
  let r = await admin.get('/bi/datasets');
  const builtins = r.data.items.filter((d: any) => d.kind === 'nerp');
  ok('NERP’s own data is there', builtins.length >= 14 && builtins.find((d: any) => d.key === 'nerp:deals').columns.some((c: any) => c.key === 'amount' && c.type === 'number'), r.data.items.map((d: any) => d.key));
  const broken: [string, unknown][] = [];
  for (const d of builtins) {
    const x = await admin.get(`/bi/datasets/${d.key}`);
    if (x.status !== 200 || !Array.isArray(x.data.preview)) broken.push([d.key, x.data]);
  }
  ok('every built-in dataset runs', broken.length === 0, broken);

  // Deals to count.
  const base = (await admin.get('/crm/config')).data.base_currency;
  for (const [amount, source] of [[1000, 'website'], [500, 'website'], [250, 'call']] as const) {
    await admin.post('/crm/deal/items', { title: `Bi deal ${stamp} ${amount}`, currency: base, opportunity: String(amount), source });
  }
  const mine = { field: 'title', op: 'contains', value: stamp };
  r = await q({ dataset: 'nerp:deals', groups: [{ field: 'source' }], measures: [{ agg: 'sum', field: 'amount' }, { agg: 'count' }], filters: [mine] });
  ok('grouped figures', r.status === 200 && JSON.stringify(r.data.rows.map((x: any) => [x[0], Number(x[1]), x[2]])) === JSON.stringify([['website', 1500, 2], ['call', 250, 1]]) && r.data.columns[1].label === 'Total Amount', r.data);
  r = await q({ dataset: 'nerp:deals', groups: [{ field: 'created_at', bucket: 'month' }], measures: [{ agg: 'count' }], filters: [mine], date_field: 'created_at' }, { period: { preset: 'this_month' } });
  ok('dates by month, within the period', r.status === 200 && r.data.rows.length === 1 && r.data.rows[0][1] === 3 && /^\d{4}-\d{2}-01$/.test(r.data.rows[0][0]), r.data);
  r = await q({ dataset: 'nerp:deals', measures: [{ agg: 'count' }], filters: [mine], date_field: 'created_at' }, { period: { preset: 'last_year' } });
  ok('nothing outside it', r.data.rows[0][0] === 0, r.data);
  r = await q({ dataset: 'nerp:deals', columns: ['title', 'amount'], filters: [mine, { field: 'amount', op: 'gte', value: 500 }], sort: { by: 'amount', desc: false } });
  ok('a table of rows, filtered and sorted', r.data.rows.length === 2 && Number(r.data.rows[0][1]) === 500, r.data);
  r = await q({ dataset: 'nerp:deals', groups: [{ field: 'title"; DROP TABLE users; --' }] });
  ok('only the dataset’s columns', r.status === 400 && /no column/.test(r.data.error.message), r.data);
  ok('only fitting figures', (await q({ dataset: 'nerp:deals', measures: [{ agg: 'sum', field: 'title' }] })).status === 400);
  r = await q({ dataset: 'nerp:salaries' });
  ok('no such dataset', r.status === 400 && /not found/.test(r.data.error.message), r.data);

  // ---- CSV ----
  let f = await upload(admin, 'regions.csv', 'Region;Sales;Day;Note\nNorth;1.200,50;2026-01-05;=HYPERLINK("x")\nSouth;300;2026-02-01;\nNorth;99,5;2026-02-03;ok\n');
  r = await admin.post('/bi/datasets', { name: `Regions ${stamp}`, kind: 'csv', file_id: f.id });
  const csvDs = r.data;
  created.datasets.push(csvDs.id);
  ok('a CSV dataset with typed columns', r.status === 200 && csvDs.row_count === 3 && JSON.stringify(csvDs.columns.map((c: any) => c.type)) === '["text","number","date","text"]', r.data);
  r = await q({ dataset: csvDs.key, groups: [{ field: 'region' }], measures: [{ agg: 'sum', field: 'sales' }], sort: { by: 'g0' } });
  ok('figures from the file', JSON.stringify(r.data.rows.map((x: any) => [x[0], Number(x[1])])) === JSON.stringify([['North', 1300], ['South', 300]]), r.data);
  let res = await admin.req('GET', `/bi/datasets/${csvDs.key}/export.csv`, undefined, true);
  let text = await res.text();
  ok('downloaded as CSV, formulas made plain text', res.status === 200 && text.includes(`'=HYPERLINK`) && text.includes('1200.5'), text);
  f = await upload(admin, 'regions2.csv', 'Region,Sales\nEast,10\n');
  r = await admin.post(`/bi/datasets/${csvDs.key}/refresh`, { file_id: f.id });
  ok('a new file replaces the rows', r.data.row_count === 1 && r.data.columns.length === 2, r.data);
  ok('NERP’s own data needs no refresh', (await admin.post('/bi/datasets/nerp:deals/refresh', {})).status === 400);

  // ---- URL ----
  r = await admin.post('/bi/datasets', { name: `Scores ${stamp}`, kind: 'url', url: webUrl, refresh_hours: 6 });
  const urlDs = r.data;
  created.datasets.push(urlDs.id);
  ok('a JSON list from a URL', r.status === 200 && urlDs.row_count === 3 && urlDs.columns.find((c: any) => c.key === 'score').type === 'number', r.data);
  served = 'team,score\nGreen,1\n';
  r = await admin.post(`/bi/datasets/${urlDs.key}/refresh`, {});
  ok('fetched again (CSV this time)', r.data.row_count === 1 && r.data.last_error === null, r.data);
  ok('refresh hours are checked', (await admin.patch(`/bi/datasets/${urlDs.key}`, { refresh_hours: 0 })).status === 400);
  await db.query(`UPDATE bi_datasets SET refreshed_at = now() - interval '7 hours' WHERE id = $1`, [urlDs.id]);

  // ---- an outside PostgreSQL (NERP's own database, as any other) ----
  const u = new URL(dbUrl!);
  const pgIn = { name: `Warehouse ${stamp}`, kind: 'postgres', host: u.hostname, port: Number(u.port || 5432), database: u.pathname.slice(1), username: decodeURIComponent(u.username), password: decodeURIComponent(u.password) };
  ok('a wrong password is caught at once', (await admin.post('/bi/sources', { ...pgIn, password: 'nope-wrong' })).status === 400);
  r = await admin.post('/bi/sources', pgIn);
  const src = r.data;
  created.sources.push(src.id);
  ok('a PostgreSQL database', r.status === 200 && src.has_password === true && !('password' in src), r.data);
  r = await admin.get(`/bi/sources/${src.id}/tables`);
  ok('its tables', r.data.items.includes('public.crm_items'), r.data.items?.length);
  r = await admin.post('/bi/datasets', { name: `Outside deals ${stamp}`, kind: 'sql', source_id: src.id, query: `SELECT title AS "Deal title", opportunity AS "Value", created_at FROM crm_items WHERE title LIKE 'Bi deal ${stamp}%';` });
  const sqlDs = r.data;
  created.datasets.push(sqlDs.id);
  ok('a query as a dataset', r.status === 200 && JSON.stringify(sqlDs.columns.map((c: any) => [c.key, c.type])) === '[["deal_title","text"],["value","number"],["created_at","time"]]', r.data);
  r = await q({ dataset: sqlDs.key, measures: [{ agg: 'sum', field: 'value' }, { agg: 'max', field: 'value' }] });
  ok('figures from the outside database', Number(r.data.rows[0][0]) === 1750 && Number(r.data.rows[0][1]) === 1000, r.data);
  r = await admin.post('/bi/datasets', { name: `Sneaky ${stamp}`, kind: 'sql', source_id: src.id, query: 'WITH x AS (DELETE FROM bi_presets RETURNING id) SELECT * FROM x' });
  ok('queries cannot change data', r.status === 400, r.data);
  r = await admin.post('/bi/datasets', { name: `Sneaky ${stamp}`, kind: 'sql', source_id: src.id, query: "SELECT 1 AS a; UPDATE users SET first_name = 'x'" });
  ok('one statement only', r.status === 400, r.data);
  r = await admin.post('/bi/datasets', { name: `Users table ${stamp}`, kind: 'sql', source_id: src.id, table: 'public.departments' });
  created.datasets.push(r.data.id);
  ok('a whole table as a dataset', r.status === 200 && r.data.columns.some((c: any) => c.key === 'name'), r.data);

  // ---- only PostgreSQL ----
  r = await admin.post('/bi/sources', { name: `Shop db ${stamp}`, kind: 'mysql', host: '127.0.0.1', port: 3306, database: 'shop', username: 'bi', password: 'x' });
  ok('other kinds of database are refused', r.status === 400, r.data);

  // ---- dashboards ----
  ok('only BI builders make dashboards', (await vic.post('/bi/dashboards', { title: 'x' })).status === 403);
  r = await admin.post('/bi/dashboards', { title: `Sales ${stamp}`, description: 'The month at a glance' });
  const dash = r.data;
  created.dashboards.push(dash.id);
  ok('a dashboard, this month by default', r.status === 200 && dash.filters.period.preset === 'this_month' && dash.can_edit, r.data);
  const add = (w: object) => admin.post(`/bi/dashboards/${dash.id}/widgets`, w);
  await add({ title: 'Won so far', chart: 'number', w: 3, h: 1, spec: { dataset: 'nerp:deals', measures: [{ agg: 'sum', field: 'amount' }], filters: [mine], date_field: 'created_at', compare: true } });
  await add({ title: 'By source', chart: 'pie', w: 5, spec: { dataset: 'nerp:deals', groups: [{ field: 'source' }], measures: [{ agg: 'count' }], filters: [mine], date_field: 'created_at' } });
  r = await add({ title: 'Regions', chart: 'table', w: 4, spec: { dataset: csvDs.key, columns: ['region', 'sales'] } });
  ok('charts added', r.data.widgets.length === 3 && r.data.fields.some((x: any) => x.key === 'source') && r.data.fields.some((x: any) => x.key === 'region'), r.data);
  ok('checked against the dataset', (await add({ title: 'x', chart: 'bar', spec: { dataset: 'nerp:deals', groups: [{ field: 'nope' }] } })).status === 400);
  ok('known kinds of chart only', (await add({ title: 'x', chart: 'radar', spec: { dataset: 'nerp:deals' } })).status === 400);
  ok('sizes are checked', (await add({ title: 'x', chart: 'bar', w: 13, spec: { dataset: 'nerp:deals' } })).status === 400);
  r = await admin.post(`/bi/dashboards/${dash.id}/data`, { period: { preset: 'this_month' } });
  const ws = (await admin.get(`/bi/dashboards/${dash.id}`)).data.widgets;
  const item = (t: string) => r.data.items.find((i: any) => i.id === ws.find((w: any) => w.title === t).id);
  ok('every chart’s figures', r.status === 200 && Number(item('Won so far').result.rows[0][0]) === 1750 && item('By source').result.rows.length === 2 && item('Regions').result.rows.length === 1, r.data);
  ok('compared with the month before', Array.isArray(item('Won so far').result.previous.rows) && /-01$/.test(item('Won so far').result.previous.from), item('Won so far'));
  r = await admin.post(`/bi/dashboards/${dash.id}/data`, { period: { preset: 'this_month' }, filters: [{ field: 'source', op: 'eq', value: 'call' }] });
  ok('dashboard filters apply where the column exists', Number(item('Won so far').result.rows[0][0]) === 250 && item('Regions').result.rows.length === 1, r.data);
  r = await admin.post(`/bi/dashboards/${dash.id}/data`, { period: { preset: 'fortnight' } });
  ok('periods are checked', r.status === 400);

  // Layout, copy, search.
  r = await admin.put(`/bi/dashboards/${dash.id}/layout`, { items: [...ws].reverse().map((w: any) => ({ id: w.id, w: 6, h: 2 })) });
  ok('reordered and resized', r.data.widgets[0].title === 'Regions' && r.data.widgets.every((w: any) => w.w === 6), r.data.widgets);
  r = await admin.post(`/bi/dashboards/${dash.id}/copy`);
  const copyId = r.data.id;
  created.dashboards.push(copyId);
  ok('copied with its charts', r.data.title === `Sales ${stamp} (copy)` && r.data.widgets.length === 3, r.data);
  r = await admin.get(`/bi/dashboards?q=${encodeURIComponent(`sales ${stamp}`)}`);
  ok('found by name', r.data.items.length === 2 && r.data.can_manage, r.data);

  // ---- sharing ----
  ok('not shared: not there', (await vic.get(`/bi/dashboards/${dash.id}`)).status === 404 && (await vic.post(`/bi/dashboards/${dash.id}/data`, {})).status === 404);
  ok('not listed', !(await vic.get('/bi/dashboards')).data.items.some((d: any) => d.id === dash.id));
  r = await admin.put(`/bi/dashboards/${dash.id}/access`, { grants: [{ kind: 'user', id: vic.id }] });
  ok('shared with a person', r.data.access.length === 1 && r.data.access[0].name === `Vic Bi${stamp}`, r.data);
  r = await vic.get(`/bi/dashboards/${dash.id}`);
  ok('they open it, read-only', r.status === 200 && r.data.can_edit === false && !('access' in r.data), r.data);
  r = await vic.post(`/bi/dashboards/${dash.id}/data`, { period: { preset: 'this_month' } });
  ok('and see its figures', r.status === 200 && r.data.items.every((i: any) => i.result), r.data);
  ok('but cannot change it', (await vic.post(`/bi/dashboards/${dash.id}/widgets`, { title: 'x', chart: 'bar', spec: { dataset: 'nerp:deals' } })).status === 403
    && (await vic.patch(`/bi/dashboards/${dash.id}`, { title: 'x' })).status === 403 && (await vic.post('/bi/query', { spec: { dataset: 'nerp:deals' } })).status === 403);
  r = await vic.post(`/bi/dashboards/${dash.id}/presets`, { name: 'Calls', filters: { period: { preset: 'last_30_days' }, filters: [{ field: 'source', op: 'eq', value: 'call' }] } });
  ok('they save their own preset', r.status === 200 && r.data.presets.length === 1 && r.data.presets[0].mine, r.data);
  ok('but not one for everyone', (await vic.post(`/bi/dashboards/${dash.id}/presets`, { name: 'All', shared: true, filters: {} })).status === 403);
  await admin.post(`/bi/dashboards/${dash.id}/presets`, { name: 'Web', shared: true, filters: { filters: [{ field: 'source', op: 'eq', value: 'website' }] } });
  r = await vic.get(`/bi/dashboards/${dash.id}`);
  ok('shared presets reach everyone', r.data.presets.length === 2 && r.data.presets.some((p: any) => p.name === 'Web' && !p.mine), r.data.presets);
  ok('others’ presets are not deletable', (await vic.del(`/bi/presets/${r.data.presets.find((p: any) => p.name === 'Web').id}`)).status === 404);
  ok('preset filters are checked', (await vic.post(`/bi/dashboards/${dash.id}/presets`, { name: 'x', filters: { period: { preset: 'never' } } })).status === 400);
  const team = (await admin.post('/departments', { name: `BI team ${stamp}` })).data;
  await admin.post(`/departments/${team.id}/members`, { user_id: dan.id });
  if (!(await db.query('SELECT 1 FROM user_departments WHERE user_id = $1', [dan.id])).rowCount) await db.query('INSERT INTO user_departments (user_id, department_id) VALUES ($1, $2)', [dan.id, team.id]);
  ok('a department not yet', (await dan.get(`/bi/dashboards/${dash.id}`)).status === 404);
  await admin.put(`/bi/dashboards/${dash.id}/access`, { grants: [{ kind: 'user', id: vic.id }, { kind: 'department', id: team.id }] });
  ok('shared with a department', (await dan.get(`/bi/dashboards/${dash.id}`)).status === 200 && (await dan.get('/bi/dashboards')).data.items.some((d: any) => d.id === dash.id));

  // ---- public links ----
  r = await admin.post(`/bi/dashboards/${dash.id}/links`, { password: 'open sesame' });
  const token = r.data.url.split('/dash/')[1];
  ok('a public link', r.status === 200 && r.data.url.includes('/dash/'), r.data);
  const anon = new Client();
  ok('it asks for the password', (await anon.get(`/public/bi/${token}`)).status === 401);
  const pass_ = async (m: string, p: string, body?: object) => (await fetch(`http://localhost:8080/api${p}`, { method: m, headers: { 'X-Dashboard-Password': 'open sesame', 'content-type': 'application/json' }, body: body ? JSON.stringify(body) : undefined })).json();
  r = { data: await pass_('GET', `/public/bi/${token}`) } as any;
  ok('with it, the dashboard (without its queries)', r.data.title === `Sales ${stamp}` && r.data.widgets.length === 3 && !('spec' in r.data.widgets[0]), r.data);
  r = { data: await pass_('POST', `/public/bi/${token}/data`, { period: { preset: 'this_month' }, filters: [{ field: 'source', op: 'eq', value: 'nothing' }] }) } as any;
  ok('and its figures (visitors pick the period only)', r.data.items.length === 3 && r.data.items.every((i: any) => i.result) && r.data.items.some((i: any) => Number(i.result.rows[0]?.[0]) === 1750), r.data);
  const links = (await admin.get(`/bi/dashboards/${dash.id}/links`)).data.items;
  ok('links are listed', links.length === 1 && links[0].password === true, links);
  await db.query(`UPDATE bi_links SET expires_at = now() - interval '1 minute' WHERE dashboard_id = $1`, [dash.id]);
  ok('an expired link stops working', (await pass_('GET', `/public/bi/${token}`)).error !== undefined);
  await admin.del(`/bi/dashboards/${dash.id}/links/${links[0].id}`);
  ok('a deleted one too', (await admin.get(`/bi/dashboards/${dash.id}/links`)).data.items.length === 0);

  // ---- downloads, refresh job, deleting ----
  res = await admin.req('GET', '/bi/datasets/nerp:deals/export.csv', undefined, true);
  text = await res.text();
  ok('NERP data as CSV', res.status === 200 && text.includes(`Bi deal ${stamp} 1000`) && text.split('\r\n')[0].includes('Amount'), text.slice(0, 200));
  ok('downloads are for BI builders', (await vic.req('GET', '/bi/datasets/nerp:deals/export.csv', undefined, true)).status === 403);
  served = 'team,score\nGreen,1\nGold,2\n';
  await db.query(`INSERT INTO jobs (kind, payload) VALUES ('bi.refresh', '{}')`);
  const due = await (async () => { for (let i = 0; i < 30; i++) { const x = await db.query('SELECT row_count FROM bi_datasets WHERE id = $1', [urlDs.id]); if (x.rows[0].row_count === 2) return true; await new Promise((s) => setTimeout(s, 1000)); } return false; })();
  ok('URL datasets refresh on their own', due);
  r = await admin.del(`/bi/datasets/${csvDs.key}`);
  ok('a dataset in use stays', r.status === 409, r.data);
  await admin.post(`/bi/dashboards/${copyId}/widgets`, { title: 'Outside', chart: 'number', spec: { dataset: sqlDs.key, measures: [{ agg: 'count' }] } });
  ok('so does a database in use', (await admin.del(`/bi/sources/${src.id}`)).status === 409);
  r = await admin.del(`/bi/dashboards/${dash.id}`);
  ok('a dashboard deleted', r.status === 200 && (await admin.get(`/bi/dashboards/${dash.id}`)).status === 404);
} finally {
  web.close();
  for (const id of created.dashboards) await admin.del(`/bi/dashboards/${id}`);
  for (const id of created.datasets) await admin.del(`/bi/datasets/${id}`);
  for (const id of created.sources) await admin.del(`/bi/sources/${id}`);
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
