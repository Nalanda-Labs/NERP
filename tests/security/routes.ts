// Every API route, read from the backend's source: `.route("/path", get(a).post(b))` in
// backend/src (all mounted under /api, except main.rs's own top-level routes such as /dav and
// the ads' short links /go/{code}, which aren't API routes). Used by the sweeps so new routes
// are attacked too.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

export type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
export interface Route {
  path: string;
  methods: Method[];
  file: string;
  /** The handler function for each method. */
  handlers: Partial<Record<Method, string>>;
  /** Methods whose handler starts by requiring a permission (an employee without it is refused). */
  gated: Method[];
}

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
    if (file.slice(SRC.length) === 'main.rs') continue;
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
      const found = [...chain.matchAll(/\b(get|post|put|patch|delete)\(([\w:]*)\)?/g)].map((m) => [m[1].toUpperCase() as Method, m[2].split('::').pop() ?? ''] as const);
      const key = '/api' + path;
      const r = out.get(key) ?? { path: key, methods: [], file: file.slice(SRC.length), handlers: {}, gated: [] };
      for (const [m, h] of found) {
        if (!r.methods.includes(m)) r.methods.push(m);
        if (h) r.handlers[m] = h;
        if (h && gatedHandler(src, h)) r.gated.push(m);
      }
      out.set(key, r);
      i = j;
    }
  }
  return [...out.values()].sort((a, b) => a.path.localeCompare(b.path));
}

/** Whether a handler's first statements require a permission of the caller (not a check on
 *  one object, like the drive's `need(…)`). */
function gatedHandler(src: string, name: string): boolean {
  const at = src.search(new RegExp(`async fn ${name}\\s*[<(]`));
  if (at < 0) return false;
  const first = src.slice(at, at + 700).split('\n').slice(1, 4).join('\n');
  const m = first.match(/^\s*(?:let\s+\w+\s*=\s*)?([\w:.]*(?:require|need|manage|admin|must|is_admin)\w*)\(\s*&?(?:state,\s*&?)?auth\b[^;]*\)\?;/m);
  return !!m && m[1] !== 'need';
}

/** Fills path parameters (`{id}`, `{token}`…) with a value. */
export const fill = (path: string, value = '00000000-0000-4000-8000-000000000000') => path.replace(/\{[^}]+\}/g, value);

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const r = routes();
  console.log(`${r.length} paths, ${r.reduce((n, x) => n + x.methods.length, 0)} endpoints`);
}
