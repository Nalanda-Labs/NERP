// Shows the CRM tables, installed extensions and which migrations have run.
import pg from 'pg';
import { readFileSync } from 'node:fs';
// DATABASE_URL from the environment, else from the backend's .env.
const url = process.env.DATABASE_URL ?? readFileSync(new URL('../../backend/.env', import.meta.url), 'utf8').match(/^DATABASE_URL=(.*)$/m)?.[1];
const c = new pg.Client({ connectionString: url });
await c.connect();
console.log((await c.query("select tablename from pg_tables where tablename like 'crm_%' order by 1")).rows.map(r => r.tablename).join(' ') || '(no crm tables)');
console.log((await c.query("select extname from pg_extension")).rows.map(r => r.extname).join(' '));
console.log((await c.query("select version, success from _sqlx_migrations order by version")).rows.map(r => `${r.version}:${r.success}`).join(' '));
await c.end();
