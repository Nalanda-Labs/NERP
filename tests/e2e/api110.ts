// Tamper-evident audit trail and the full company export.
//
// The audit chain verifies; the database refuses changes and removals; a record changed
// behind its back (triggers off) is caught, and verifies again once put back; a forged
// checkpoint is caught; a head noted earlier checks against the chain.
//
// The export: a ZIP with every table as CSV (secrets left out), the stored files decrypted,
// and a manifest whose checksums match; for system managers only.
import { createHash } from 'node:crypto';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { readZip } from './zip.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const sha = (b: Buffer) => createHash('sha256').update(b).digest('hex');
/** Runs `sql` with the audit tables' guards off, as someone with database access could. */
const behindItsBack = async (sql: string, params: unknown[] = []) => {
  await db.query('BEGIN');
  try {
    await db.query('ALTER TABLE audit_log DISABLE TRIGGER audit_log_guard');
    await db.query('ALTER TABLE audit_anchors DISABLE TRIGGER audit_anchors_guard');
    await db.query(sql, params);
    await db.query('ALTER TABLE audit_log ENABLE TRIGGER audit_log_guard');
    await db.query('ALTER TABLE audit_anchors ENABLE TRIGGER audit_anchors_guard');
    await db.query('COMMIT');
  } catch (e) {
    await db.query('ROLLBACK');
    throw e;
  }
};
const forged: string[] = [];

try {
  // ---- the chain ----
  let r = await admin.post('/audit/chain/verify', {});
  ok('the audit chain verifies', r.status === 200 && r.data.ok === true && r.data.records > 0 && r.data.unsealed === 0 && /^[0-9a-f]{64}$/.test(r.data.head.hash), r.data);
  const status = (await admin.get('/audit/chain')).data;
  ok('…its head is shown', status.head?.position >= r.data.records && status.last_verify?.ok === true, status);

  const target = (await db.query("SELECT id, chain_pos, details::text AS details FROM audit_log WHERE chain_pos IS NOT NULL ORDER BY chain_pos DESC LIMIT 1")).rows[0];
  const refused = async (sql: string) => db.query(sql, [target.id]).then(() => false, (e) => /cannot be (changed|removed)/.test(e.message));
  ok('the database refuses changing a record', await refused("UPDATE audit_log SET details = '{\"x\": 1}' WHERE id = $1"));
  ok('…or removing one', await refused('DELETE FROM audit_log WHERE id = $1'));
  ok('…or emptying the table', await db.query('TRUNCATE audit_log').then(() => false, (e) => /cannot be/.test(e.message)));

  await behindItsBack(`UPDATE audit_log SET details = '{"tampered": "${s}"}' WHERE id = $1`, [target.id]);
  r = await admin.post('/audit/chain/verify', {});
  ok('a record changed behind its back is caught', r.data.ok === false && r.data.problems.some((p: any) => p.kind === 'changed' && p.position === Number(target.chain_pos)), r.data.problems);
  await behindItsBack('UPDATE audit_log SET details = $2::jsonb WHERE id = $1', [target.id, target.details]);
  r = await admin.post('/audit/chain/verify', {});
  ok('…and the chain holds again once it is put back', r.data.ok === true, r.data.problems);

  // ---- checkpoints ----
  const cp = (await admin.post('/audit/chain/checkpoint', {})).data;
  ok('a checkpoint of the head', cp.head?.position > 0, cp);
  r = await admin.post('/audit/chain/verify', { checkpoint: { position: cp.head.position, hash: cp.head.hash } });
  ok('a head noted earlier checks against the chain', r.data.ok === true && r.data.checkpoint?.ok === true, r.data);
  r = await admin.post('/audit/chain/verify', { checkpoint: { position: cp.head.position, hash: '00'.repeat(32) } });
  ok('…a wrong one doesn\'t', r.data.ok === false && r.data.checkpoint?.ok === false && r.data.problems.some((p: any) => p.kind === 'noted'), r.data);
  const f = (await db.query("INSERT INTO audit_anchors (chain_pos, hash, mac) VALUES ($1, $2, $3) RETURNING id", [cp.head.position, Buffer.from(cp.head.hash, 'hex'), Buffer.alloc(32)])).rows[0].id;
  forged.push(f);
  r = await admin.post('/audit/chain/verify', {});
  ok('a checkpoint not signed by the server is caught', r.data.ok === false && r.data.problems.some((p: any) => p.kind === 'checkpoint'), r.data.problems);
  ok('checkpoints can\'t be removed', await db.query('DELETE FROM audit_anchors WHERE id = $1', [f]).then(() => false, (e) => /cannot be/.test(e.message)));
  await behindItsBack('DELETE FROM audit_anchors WHERE id = $1', [f]);
  forged.length = 0;

  const nina = new Client();
  await admin.post('/users', { email: `nina.${s}@acme.test`, password: 'password123', first_name: 'Nina', last_name: `X${s}` });
  await nina.post('/auth/login', { email: `nina.${s}@acme.test`, password: 'password123' });
  ok('verifying is for those who see the audit log', (await nina.post('/audit/chain/verify', {})).status === 403);

  // ---- the export ----
  const deal = (await admin.post('/crm/deal/items', { title: `Export me ${s}` })).data;
  const text = `exported file ${s} ✓`;
  const fd = new FormData(); fd.append('file', new Blob([text], { type: 'text/plain' }), `notes ${s}.txt`);
  const fileId = (await admin.req('POST', '/files', fd) as any).data.id as string;
  ok('exporting is for system managers', (await nina.post('/admin/exports', { include_files: true })).status === 403);
  r = await admin.post('/admin/exports', { include_files: true });
  ok('an export starts', r.status === 200 && r.data.id, r.data);
  const id = r.data.id;
  ok('…one at a time', [409, 200].includes((await admin.post('/admin/exports', {})).status));
  let ex: any;
  for (let i = 0; i < 240; i++) {
    ex = (await admin.get('/admin/exports')).data.items.find((x: any) => x.id === id);
    if (ex?.status !== 'running') break;
    await new Promise((res) => setTimeout(res, 500));
  }
  ok('…and finishes', ex?.status === 'done' && ex.tables > 50 && ex.files >= 1 && ex.size_bytes > 0, ex);
  // A second export, if one was started above, is removed later.
  const res = await admin.req('GET', `/admin/exports/${id}/download`, undefined, true) as Response;
  const zip = Buffer.from(await res.arrayBuffer());
  ok('the download is the recorded archive', res.status === 200 && sha(zip) === ex.sha256 && /nerp-export-.*\.zip/.test(res.headers.get('content-disposition') ?? ''), res.status);
  const parts = readZip(zip);
  const manifest = JSON.parse(parts.get('manifest.json')?.toString() ?? '{}');
  ok('a manifest with tables, files and the audit head', manifest.format === 'nerp-export/1' && manifest.tables.length === ex.tables && manifest.audit_head?.position > 0, Object.keys(manifest));
  const bad = manifest.checksums.filter((c: any) => !parts.has(c.path) || sha(parts.get(c.path)!) !== c.sha256);
  ok('every part matches its checksum', manifest.checksums.length === parts.size - 1 && bad.length === 0, bad.slice(0, 3));
  const users = parts.get('data/users.csv')?.toString() ?? '';
  const header = users.split('\n')[0];
  ok('tables as CSV, secrets left out', /\bemail\b/.test(header) && !/password_hash|totp_secret/.test(header) && users.includes('admin@acme.test')
    && manifest.tables.find((t: any) => t.name === 'users').left_out.includes('password_hash'), header);
  ok('sign-in sessions aren\'t exported', !parts.has('data/auth_sessions.csv') && !parts.has('data/jobs.csv'));
  ok('records are in', (parts.get('data/crm_items.csv')?.toString() ?? '').includes(`Export me ${s}`) && (parts.get('data/crm_items.csv')?.toString() ?? '').includes(deal.id));
  const entry = manifest.files.find((x: any) => x.id === fileId);
  ok('stored files, decrypted, under their names', entry?.path === `files/${fileId}/notes ${s}.txt` && parts.get(entry.path)?.toString() === text, entry);
  ok('the files table doesn\'t give away storage paths or keys', !/storage_path|enc_key/.test((parts.get('data/files.csv')?.toString() ?? '').split('\n')[0]));
  ok('downloading is for system managers', (await nina.req('GET', `/admin/exports/${id}/download`, undefined, true) as Response).status === 403);
  ok('the export is in the audit log', (await db.query("SELECT 1 FROM audit_log WHERE action = 'system.export_download' AND entity_id = $1", [id])).rowCount === 1);
  for (let i = 0; i < 240 && (await admin.get('/admin/exports')).data.items.some((x: any) => x.status === 'running'); i++) await new Promise((res) => setTimeout(res, 500));
  for (const x of (await admin.get('/admin/exports')).data.items.filter((x: any) => x.status !== 'running' && new Date(x.created_at).getTime() > Date.now() - 600e3)) {
    await admin.req('DELETE', `/admin/exports/${x.id}`);
  }
  ok('an export can be removed', !(await admin.get('/admin/exports')).data.items.some((x: any) => x.id === id) && (await admin.req('GET', `/admin/exports/${id}/download`, undefined, true) as Response).status === 404);
} finally {
  for (const f of forged) await behindItsBack('DELETE FROM audit_anchors WHERE id = $1', [f]).catch(() => {});
  await db.query("DELETE FROM crm_items WHERE title = $1", [`Export me ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
