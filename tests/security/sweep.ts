// Shared parts of the per-endpoint sweeps: every endpoint read from the backend's source,
// requests with full control over the token, headers and body (timed), and the endpoints a
// signed-in attacker must not call on themselves during a sweep (signing out, changing their
// own password…), which would end the very session doing the sweep.
import { routes, type Method, type Route } from './routes.ts';
import { API } from './lib.ts';

export type Endpoint = { method: Method; path: string; route: Route; gated: boolean };
export const NIL = '00000000-0000-4000-8000-000000000000';

export function endpoints(): Endpoint[] {
  return routes().flatMap((r) => r.methods.map((m) => ({ method: m, path: r.path, route: r, gated: r.gated.includes(m) })));
}

/** The routes for people without an account. */
export const PUBLIC = [
  /^\/api\/public\//, /^\/api\/health$/, /^\/api\/openapi\.json$/, /^\/api\/setup$/,
  /^\/api\/auth\/(login|login\/2fa|login\/2fa-setup|login\/2fa-setup\/start|logout|refresh|forgot-password|reset-password|accept-invite|invite\/\{token\})$/,
  /^\/api\/auth\/sso(\/start|\/callback)?$/, /^\/api\/auth\/providers(\/\{id\}\/start|\/callback)?$/, /^\/api\/crm\/mailboxes\/oauth\/callback$/, /^\/api\/realtime\/ws$/,
  /^\/api\/crm\/automation\/webhook\/\{token\}$/, /^\/api\/files\/\{id\}\/(content|video|poster|video-status)$/, /^\/api\/wopi\//,
];
export const isPublic = (path: string) => PUBLIC.some((p) => p.test(path));

/** Calls that end or change the caller's own session or account: never swept with a live session. */
export const SELF_HARM = /^\/api\/(auth\/(logout|change-password|sessions|2fa|app-passwords|identities|refresh|login|accept-invite|reset-password|forgot-password|providers|sso)|realtime|setup|me\/ui$)/;

/** Fills each path parameter with the next value (the last one repeats). */
export const fillPath = (path: string, values: string[]) => {
  let i = 0;
  return path.replace(/\{[^}]+\}/g, () => values[Math.min(i++, values.length - 1)] ?? NIL);
};
export const params = (path: string) => (path.match(/\{[^}]+\}/g) ?? []).length;

export type Res = { status: number; headers: Headers; text: string; json: any; ms: number };
/** A request with the given bearer token (or none), headers and raw body. */
export async function call(method: string, path: string, opts: { token?: string | null; body?: string; headers?: Record<string, string> } = {}): Promise<Res> {
  const headers: Record<string, string> = { ...(opts.headers ?? {}) };
  if (opts.token) headers.authorization = `Bearer ${opts.token}`;
  if (opts.body !== undefined && !Object.keys(headers).some((h) => h.toLowerCase() === 'content-type')) headers['content-type'] = 'application/json';
  const t = Date.now();
  const res = await fetch(API + path.replace(/^\/api/, ''), { method, headers, body: opts.body, redirect: 'manual' });
  const text = await res.text();
  let json: any = null;
  try { json = JSON.parse(text); } catch { /* not JSON */ }
  return { status: res.status, headers: res.headers, text, json, ms: Date.now() - t };
}
export const ok2xx = (s: number) => s >= 200 && s < 300;
/** A write body that names the attacker, with the fields most create and change routes read. */
export const writeBody = (who: string, tag: string) => JSON.stringify({ title: tag, name: tag, body: tag, text: tag, description: tag, members: [who], member_ids: [who], user_id: who, users: [who], level: 'edit', role: 'owner', status: 'approved', approved: true });

/** A value of the type a deserialization error asks for. */
function guess(expected: string): unknown {
  const e = expected.toLowerCase();
  if (/uuid/.test(e)) return NIL;
  if (/sequence|vec|array|tuple/.test(e)) return [];
  if (/bool/.test(e)) return false;
  if (/u8|u16|u32|u64|i8|i16|i32|i64|usize|isize|integer/.test(e)) return 1;
  if (/f32|f64|float|number/.test(e)) return 1;
  if (/date|time/.test(e)) return /time/.test(e) ? '2027-01-01T00:00:00Z' : '2027-01-01';
  if (/map|struct|object/.test(e)) return {};
  if (/variant/.test(e)) return null;
  return 'x';
}

/** Calls an endpoint, completing the body and query from the server's own "missing field" and
 *  "invalid type" answers (up to 15 times), so the handler itself is reached. Returns the last
 *  answer and whether the request got past the shape checks. */
export async function reach(method: string, path: string, token: string | null | undefined, start: Record<string, unknown> = {}): Promise<Res & { reached: boolean; body: Record<string, unknown>; query: URLSearchParams }> {
  const body: Record<string, unknown> = { ...start };
  const query = new URLSearchParams();
  let r!: Res;
  for (let i = 0; i < 15; i++) {
    const q = query.toString();
    r = await call(method, q ? `${path}${path.includes('?') ? '&' : '?'}${q}` : path, { token: token ?? undefined, body: method === 'GET' || method === 'DELETE' ? undefined : JSON.stringify(body) });
    const msg = String(r.json?.error?.message ?? r.text ?? '');
    const missing = msg.match(/(?:target type|query string): (?:([\w.\[\]]+): )?missing field `([\w.]+)`/);
    const wrong = msg.match(/target type: (?:([\w.\[\]]+): )?invalid type: [^,]+, expected ([^"]+?)(?: at line|$)/);
    const unknownVariant = msg.match(/unknown variant `[^`]*`, expected (?:one of )?`([^`]+)`/);
    if (/deserialize the JSON body|deserialize query string/.test(msg) || r.status === 422) {
      const target = /query string/.test(msg) ? 'query' : 'body';
      if (missing) {
        const f = missing[2];
        const v = /(_id|^id)$/.test(f) ? NIL : /_ids$|s$/.test(f) ? [] : /^(is_|has_)/.test(f) ? false : /(count|days|hours|minutes|limit|amount|price|quantity|extra|position|sort)/.test(f) ? 1 : 'x';
        if (target === 'query') query.set(f, f.endsWith('id') ? NIL : '1');
        else {
          // A field inside another (`spec: missing field …`): made in place.
          let o: Record<string, unknown> = body;
          for (const k of (missing[1] ?? '').split('.').filter(Boolean).map((k) => k.replace(/\[\d+\]$/, ''))) {
            if (typeof o[k] !== 'object' || o[k] === null || Array.isArray(o[k])) o[k] = {};
            o = o[k] as Record<string, unknown>;
          }
          o[f] = v;
        }
        continue;
      }
      if (wrong && wrong[1] && target === 'body') {
        const key = wrong[1].split('.')[0].replace(/\[\d+\]$/, '');
        body[key] = guess(wrong[2]);
        continue;
      }
      if (unknownVariant && target === 'body') {
        const m = msg.match(/(?:([\w.]+): )?unknown variant/);
        if (m?.[1]) { body[m[1].split('.')[0]] = unknownVariant[1]; continue; }
      }
      break;
    }
    return { ...r, reached: true, body, query };
  }
  return { ...r, reached: !/deserialize/.test(String(r.json?.error?.message ?? r.text ?? '')) && r.status !== 422, body, query };
}
