// Checks that the database supports what NERP relies on (trigram and full-text indexes on
// YugabyteDB's ybgin, range indexes), in a scratch table it drops afterwards.
import pg from 'pg';
import { readFileSync } from 'node:fs';
// DATABASE_URL from the environment, else from the backend's .env.
const url = process.env.DATABASE_URL ?? readFileSync(new URL('../../backend/.env', import.meta.url), 'utf8').match(/^DATABASE_URL=(.*)$/m)?.[1];
const c = new pg.Client({ connectionString: url });
await c.connect();
const t = async (label, sql) => { try { await c.query(sql); console.log('ok  ', label); } catch (e: any) { console.log('FAIL', label, '-', e.message.split('\n')[0]); } };
console.log((await c.query('select version()')).rows[0].version);
await t('drop', 'DROP TABLE IF EXISTS probe_t');
await t('table', 'CREATE TABLE probe_t (id uuid primary key default gen_random_uuid(), et smallint, s text, ts tsvector)');
await t('pg_trgm ext', 'CREATE EXTENSION IF NOT EXISTS pg_trgm');
await t('ybgin trgm index', 'CREATE INDEX probe_trgm ON probe_t USING ybgin (s gin_trgm_ops)');
await t('ybgin tsvector index', 'CREATE INDEX probe_tsv ON probe_t USING ybgin (ts)');
await t('range index', 'CREATE INDEX probe_rng ON probe_t (et ASC, id ASC)');
await t('insert', "INSERT INTO probe_t (et, s, ts) VALUES (1, 'acme corporation john smith', to_tsvector('simple','acme corporation john smith'))");
for (const [l, q] of [['trgm ilike', "SELECT count(*) FROM probe_t WHERE s ILIKE '%corp%'"], ['tsquery prefix', "SELECT count(*) FROM probe_t WHERE ts @@ to_tsquery('simple','corp:*')"]]) {
  try { const r = await c.query(q); const p = await c.query('EXPLAIN ' + q); console.log('ok  ', l, r.rows[0].count, '|', p.rows.map((x) => x['QUERY PLAN']).join(' / ').slice(0, 160)); } catch (e: any) { console.log('FAIL', l, e.message); }
}
await t('cleanup', 'DROP TABLE probe_t');
await c.end();
