// Injection, endpoint by endpoint, signed in as an employee. Each endpoint's body is first
// completed until its handler answers; then every text field, and the usual query
// parameters (q, search, sort, order, filter, from, to, status), get each payload: SQL
// (error-based and time-based with pg_sleep), template expressions, shell, path traversal,
// header line breaks, null bytes, NoSQL operators and markup. Per endpoint: no server error
// and no internal detail, no answer delayed by the payload, and no markup coming back as
// live HTML.
import { person, ok, check, finish, counts, LEAK, stamp } from './lib.ts';
import { endpoints, isPublic, SELF_HARM, call, fillPath, NIL, reach } from './sweep.ts';

const bob = await person('Bob');
const PAYLOADS = [
  "x' OR '1'='1", "x'; SELECT pg_sleep(3)--", "x' AND (SELECT 1 FROM pg_sleep(3)) IS NOT NULL--", '1); SELECT pg_sleep(3);--', "x\") OR (\"1\"=\"1",
  '{{7*7}}${7*7}<%= 7*7 %>#{7*7}', '$(sleep 3)`sleep 3`;sleep 3|sleep 3', '../../../../etc/passwd', '..\\..\\..\\windows\\win.ini',
  `pen\r\nX-Injected: 1\r\nSet-Cookie: pwn=1`, 'a\u0000b', '\ud800', '<img src=x onerror=alert(1)><script>alert(1)</script>',
  '%27%20OR%201=1--', '"; DROP TABLE users; --',
];
const QUERY = ['q', 'search', 'sort', 'order', 'filter', 'from', 'to', 'status', 'kind', 'type'];
let swept = 0;
for (const e of endpoints()) {
  if (isPublic(e.path) || SELF_HARM.test(e.path)) continue;
  swept++;
  const path = fillPath(e.path, [NIL]);
  const base = await reach(e.method, path, bob.token, { title: `inj ${stamp}`, name: `inj ${stamp}`, body: `inj ${stamp}` });
  const errors: string[] = [], slow: string[] = [], live: string[] = [];
  for (const p of PAYLOADS) {
    const body: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(base.body)) body[k] = typeof v === 'string' ? `${p}` : v;
    const q = new URLSearchParams(base.query);
    for (const k of QUERY) q.set(k, p);
    const r = await call(e.method, `${path}?${q}`, { token: bob.token, body: e.method === 'GET' || e.method === 'DELETE' ? undefined : JSON.stringify(body) });
    if (r.status >= 500 || LEAK.test(r.text)) errors.push(`${p.slice(0, 24)} → ${r.status} ${r.text.slice(0, 80)}`);
    if (/sleep/.test(p) && r.ms > 2500) slow.push(`${p.slice(0, 24)} took ${r.ms} ms`);
    // Markup may come back as text inside JSON, never as live HTML in an "html" field.
    const htmls = JSON.stringify(r.json ?? '').match(/"html":"(?:[^"\\]|\\.)*"/g) ?? [];
    if (htmls.some((h) => /<script|onerror=/i.test(h))) live.push(`${p.slice(0, 24)}`);
  }
  check(`${e.method} ${e.path}: injected input causes no server error or internal detail`, errors.length === 0, errors.slice(0, 3));
  check(`${e.method} ${e.path}: time-based SQL payloads cause no delay`, slow.length === 0, slow);
  check(`${e.method} ${e.path}: markup never comes back as live HTML`, live.length === 0, live);
}
ok(`(${swept} endpoints × ${PAYLOADS.length} payloads in every field)`, swept > 800);
const users = await call('GET', '/users?limit=1', { token: bob.token });
ok('the users table is still there', users.status === 200);
console.log(`checks: ${JSON.stringify(counts())}`);
await finish();
