// Every API route, read from the backend's source: `.route("/path", get(a).post(b))` in
// backend/src (all mounted under /api). Used by the sweeps so new routes are attacked too.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

export type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
export interface Route { path: string; methods: Method[]; file: string }

const SRC = new URL('../../backend/src/', import.meta.url).pathname;

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? files(p) : p.endsWith('.rs') ? [p] : [];
  });
}

export function routes(): Route[] {
  const out = new Map<string, Route>();
  for (const file of files(SRC)) {
    const src = readFileSync(file, 'utf8');
    let i = 0;
    while ((i = src.indexOf('.route("', i)) !== -1) {
      const start = i + '.route("'.length;
      const end = src.indexOf('"', start);
      const path = src.slice(start, end);
      // The handler chain runs to the parenthesis closing `.route(`.
      let depth = 1, j = src.indexOf('(', i) + 1;
      while (j < src.length && depth > 0) { if (src[j] === '(') depth++; else if (src[j] === ')') depth--; j++; }
      const chain = src.slice(end + 1, j);
      const methods = [...chain.matchAll(/\b(get|post|put|patch|delete)\(/g)].map((m) => m[1].toUpperCase() as Method);
      const key = '/api' + path;
      const r = out.get(key) ?? { path: key, methods: [], file: file.slice(SRC.length) };
      for (const m of methods) if (!r.methods.includes(m)) r.methods.push(m);
      out.set(key, r);
      i = j;
    }
  }
  return [...out.values()].sort((a, b) => a.path.localeCompare(b.path));
}

/** Fills path parameters (`{id}`, `{token}`…) with a value. */
export const fill = (path: string, value = '00000000-0000-4000-8000-000000000000') => path.replace(/\{[^}]+\}/g, value);

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const r = routes();
  console.log(`${r.length} paths, ${r.reduce((n, x) => n + x.methods.length, 0)} endpoints`);
}
