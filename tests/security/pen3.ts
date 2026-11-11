// Injection: SQL (including time-based, with pg_sleep), template, path and odd-character
// payloads in the query parameters of every read route and the text fields of every write
// route. Nothing may fail (500), leak an internal error, or take suspiciously long.
import { routes, fill } from './routes.ts';
import { admin, person, raw, ok, finish, BASE, LEAK, stamp } from './lib.ts';

const PAYLOADS = [
  `' OR '1'='1`,
  `'; SELECT pg_sleep(6); --`,
  `1) OR pg_sleep(6) IS NULL --`,
  `" OR ""="`,
  `\\'; DROP TABLE users; --`,
  `%' AND 1=1 AND '%'='`,
  `{{7*7}}\${7*7}<%= 7*7 %>#{7*7}`,
  `../../../../etc/passwd`,
  `nul\u0000byte`,
  `\uD800 lone surrogate`,
  `x`.repeat(100_000),
  `-1`, `99999999999999999999999`, `NaN`, `1e309`, `2026-13-45`, `true`, `[]`, `{}`,
];
// Sent in groups: one badly typed parameter (a number, a date, an id) rejects the whole
// request before any SQL runs, so text parameters go separately from typed ones.
const PARAM_GROUPS = [
  ['q', 'search', 'sort', 'order', 'dir', 'filter', 'fields', 'group_by', 'tag', 'field', 'value'],
  ['status', 'scope', 'view', 'type', 'kind', 'entity'],
  ['limit', 'offset', 'page', 'year', 'month'],
  ['from', 'to', 'date', 'day'],
  ['category_id', 'stage_id', 'user_id', 'project_id'],
];
// Fields that are text in nearly every body (typed fields would reject the body outright).
const FIELDS = ['title', 'name', 'body', 'description', 'q', 'text', 'comment', 'first_name', 'last_name', 'note', 'subject', 'message'];
const SLOW_MS = 5000;

const bob = await person('Bob');
const problems: string[] = [];
const check = (label: string, res: { status: number; text: string }, ms: number) => {
  if (res.status >= 500) problems.push(`${label} → ${res.status} ${res.text.slice(0, 160)}`);
  // The backups page shows system administrators its folder and the scripts' output on purpose.
  else if (LEAK.test(/^GET \/api\/admin\/backups /.test(label) ? res.text.replace(/\/home\//g, '') : res.text)) problems.push(`${label} → leaks: ${res.text.match(LEAK)?.[0]} in ${res.text.slice(0, 160)}`);
  else if (ms > SLOW_MS) problems.push(`${label} → took ${ms} ms (time-based injection?)`);
};
const short = (p: string) => JSON.stringify(p.length > 40 ? p.slice(0, 40) + '…' : p);

// ---- query parameters of every read route (as the admin: every code path is open) ----
let reads = 0, readsAccepted = 0;
for (const route of routes()) {
  if (!route.methods.includes('GET') || /^\/api\/(auth|realtime|setup)\b|\/download$|\/content$|ws$/.test(route.path)) continue;
  for (const p of PAYLOADS) for (const PARAMS of PARAM_GROUPS) {
    // A lone surrogate cannot be percent-encoded: send the invalid UTF-8 bytes it would be.
    const enc = (v: string) => { try { return encodeURIComponent(v); } catch { return '%ED%A0%80' + encodeURIComponent(v.replace(/[\uD800-\uDFFF]/g, '')); } };
    const qs = PARAMS.map((k) => `${k}=${enc(p.length > 2000 ? p.slice(0, 2000) : p)}`).join('&');
    const t = Date.now();
    const res = await raw('GET', `${fill(route.path)}?${qs}`, { base: BASE, headers: { authorization: `Bearer ${admin.token}` } });
    check(`GET ${route.path} ?${PARAMS[0]}…=${short(p)}`, res, Date.now() - t);
    reads++;
    if (res.status < 300) readsAccepted++;
  }
}
ok(`read routes resist injection in query parameters (${reads} requests, ${readsAccepted} reached the handler's query)`, problems.length === 0, problems.slice(0, 15));

// ---- text fields of every write route (as an employee) ----
const before = problems.length;
let writes = 0, writesAccepted = 0;
for (const route of routes()) {
  if (/^\/api\/(auth|realtime|setup)\b|^\/api\/files$|\/upload$|\/import$/.test(route.path)) continue;
  for (const m of route.methods.filter((x) => x === 'POST' || x === 'PATCH' || x === 'PUT')) {
    for (const p of PAYLOADS) {
      const body = Object.fromEntries(FIELDS.map((f) => [f, f === 'title' || f === 'name' ? `${p.slice(0, 200)} ${stamp}` : p]));
      const t = Date.now();
      const res = await raw(m, fill(route.path), { base: BASE, headers: { authorization: `Bearer ${bob.token}`, 'content-type': 'application/json' }, body: JSON.stringify(body) });
      check(`${m} ${route.path} {…: ${short(p)}}`, res, Date.now() - t);
      writes++;
      if (res.status < 300) writesAccepted++;
    }
  }
}
ok(`write routes resist injection in text fields (${writes} requests, ${writesAccepted} accepted)`, problems.length === before, problems.slice(before, before + 15));

// ---- the database is intact ----
ok('the users table is still there', (await admin.get('/users?limit=1')).status === 200);
ok('no stored SQL ran: nothing named after a payload became an admin', !(await admin.get('/users?limit=200')).data.items.some((u: any) => /OR '1'='1/.test(u.first_name ?? '')));

await finish();
