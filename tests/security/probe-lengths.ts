// Probe: sends every create route (POST without ids, as the admin, who passes permission
// checks) a body whose usual text fields are 100,000 characters, and lists the routes that
// accept it and keep the long text. Each long value contains the run's stamp so the usual
// cleanup finds what was made. Usage: node probe-lengths.ts
import { routes } from './routes.ts';
import { admin, stamp, cleanupRun } from './lib.ts';

const LONG = `${stamp} ` + 'x'.repeat(100_000);
const FIELDS = ['title', 'name', 'body', 'description', 'note', 'comments', 'first_name', 'last_name', 'subject', 'text', 'message', 'label', 'question', 'location', 'reason', 'content', 'summary'];
// Routes that act on the whole system or the caller's session, not things to create.
const SKIP = /^\/api\/(auth|setup|realtime|public|system|backups|admin|storage|import|.*\/import|crm\/mailboxes|calendar\/accounts|bi\/sources|download-links)/;
const kept: string[] = [];
for (const r of routes()) {
  if (!r.methods.includes('POST') || r.path.includes('{') || SKIP.test(r.path)) continue;
  const body: Record<string, unknown> = Object.fromEntries(FIELDS.map((f) => [f, LONG]));
  body.email = `long.${stamp}@acme.test`;
  const res = (await admin.post(r.path.replace('/api', ''), body)) as any;
  if (res.status >= 200 && res.status < 300) {
    const text = JSON.stringify(res.data ?? '');
    const longest = Math.max(0, ...[...text.matchAll(/x{1000,}/g)].map((m) => m[0].length));
    if (longest > 20_000) kept.push(`${r.path} kept ${longest} characters`);
  }
}
console.log(kept.length ? kept.join('\n') : 'no create route kept 100,000-character text');
await cleanupRun(stamp);
