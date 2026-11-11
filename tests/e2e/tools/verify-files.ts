// Downloads every stored file through the API (as the administrator) and checks its size and
// SHA-256 against the database: proves encrypted files decrypt to what was uploaded.
//   cd /tmp/nerp-e2e && node <repo>/tests/e2e/tools/verify-files.ts
import { API, Client } from '../lib.ts';
import { dbUrl } from '../../db/cleanup.ts';
import { createHash } from 'node:crypto';
import pg from 'pg';

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const files = (await db.query('SELECT id, size_bytes, sha256, enc_key FROM files ORDER BY created_at')).rows;
await db.end();
let good = 0, missing = 0;
const bad: string[] = [];
for (const f of files) {
  const r = await fetch(`${API}/files/${f.id}/content`, { headers: { authorization: `Bearer ${admin.token}` } });
  if (r.status === 404) { missing++; await r.body?.cancel(); continue; }
  const body = await r.arrayBuffer().then((b) => Buffer.from(b), () => null);
  if (r.status === 200 && body && body.length === Number(f.size_bytes) && createHash('sha256').update(body).digest('hex') === f.sha256) good++;
  else bad.push(`${f.id} (status ${r.status}, ${body?.length ?? 'unreadable'} of ${f.size_bytes} bytes, key ${f.enc_key ?? 'none'})`);
}
console.log(`${files.length} files: ${good} read back intact, ${missing} missing on disk, ${bad.length} wrong`);
for (const b of bad.slice(0, Number(process.env.SHOW ?? 30))) console.log('  wrong:', b);
process.exit(bad.length ? 1 : 0);
