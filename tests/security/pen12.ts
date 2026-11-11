// Signed in as an ordinary employee, every endpoint gets odd input — wrong JSON types,
// broken JSON, operator objects, odd path ids (quotes, traversal, huge, Unicode, numbers) —
// and is checked, one by one, for: no server error, no internal detail in the answer, an
// answer within 5 s, and on every successful read the headers that keep private data
// private (no-store, nosniff, a JSON content type).
import { person, ok, check, finish, counts, LEAK } from './lib.ts';
import { endpoints, isPublic, SELF_HARM, call, fillPath, params, NIL, ok2xx } from './sweep.ts';

const bob = await person('Bob');
const BODIES = ['{}', '[]', '"x"', 'null', '1e309', '{"id":{"$ne":1},"__proto__":{"admin":true}}', '{bad json', '{"title":' + '['.repeat(500) + ']'.repeat(500) + '}'];
const IDS = [NIL, "1' OR '1'='1", '..%2F..%2Fetc%2Fpasswd', 'x'.repeat(4000), '%F0%9F%92%A9', '-1', '0', '99999999999999999999', 'null', '%00'];
let n = 0;
for (const e of endpoints()) {
  if (isPublic(e.path) || SELF_HARM.test(e.path)) continue;
  n++;
  const tag = `${e.method} ${e.path}`;
  const bodies = e.method === 'GET' || e.method === 'DELETE' ? [undefined] : BODIES;
  let errors: string[] = [], leaks: string[] = [], slow = 0;
  for (const body of bodies) {
    const r = await call(e.method, fillPath(e.path, [NIL]), { token: bob.token, body });
    if (r.status >= 500) errors.push(`${body?.slice(0, 20)} → ${r.status}`);
    if (LEAK.test(r.text)) leaks.push(r.text.slice(0, 120));
    slow = Math.max(slow, r.ms);
    if (e.method === 'GET' && ok2xx(r.status)) {
      const cc = r.headers.get('cache-control') ?? '';
      check(`${tag}: a private answer is not cached`, /no-store|private/.test(cc) || /\/public\//.test(e.path), cc);
      check(`${tag}: nosniff`, r.headers.get('x-content-type-options') === 'nosniff');
    }
  }
  if (params(e.path)) {
    for (const id of IDS) {
      const r = await call(e.method, fillPath(e.path, [id]), { token: bob.token, body: e.method === 'GET' || e.method === 'DELETE' ? undefined : '{}' });
      if (r.status >= 500) errors.push(`id ${id.slice(0, 20)} → ${r.status}`);
      if (LEAK.test(r.text)) leaks.push(r.text.slice(0, 120));
      slow = Math.max(slow, r.ms);
    }
  }
  check(`${tag}: no server error on odd input`, errors.length === 0, errors);
  check(`${tag}: no internal detail in any answer`, leaks.length === 0, leaks);
  check(`${tag}: answers within 5 s`, slow < 5000, slow);
}
ok(`(${n} endpoints swept as an employee)`, n > 800);
console.log(`checks: ${JSON.stringify(counts())}`);
const me = await call('GET', '/auth/me', { token: bob.token });
ok('the employee is still signed in and unchanged after the sweep', me.status === 200 && me.json?.email === bob.email && !(me.json?.permissions ?? []).includes('*'), me.json);
await finish();
