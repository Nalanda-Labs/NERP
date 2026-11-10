// Shared pieces of the end-to-end suites.

export const BASE = 'http://localhost:8080';
export const API = `${BASE}/api`;

/** A JSON response: the HTTP status and the parsed body (text when it isn't JSON). */
export interface Res<T = any> {
  status: number;
  data: T;
}

export type LiveSocket = WebSocket & { events: any[]; closed: boolean };

/**
 * A caller of the API: keeps the access token it signs in with (or is given, e.g. an API
 * key) and the refresh cookie. Extra fields (`id`, `email`, …) may be attached by suites.
 */
export class Client {
  token: string | null;
  cookie: string | null = null;
  ua: string;
  id = '';
  [extra: string]: any;

  constructor(token: string | null = null, ua = 'nerp-e2e') {
    this.token = token;
    this.ua = ua;
  }

  req(method: string, path: string, body: unknown, raw: true): Promise<Response>;
  req(method: string, path: string, body?: unknown, raw?: false): Promise<Res>;
  async req(method: string, path: string, body?: unknown, raw = false): Promise<Res | Response> {
    const headers: Record<string, string> = { 'user-agent': this.ua };
    if (this.token) headers.authorization = `Bearer ${this.token}`;
    if (this.cookie) headers.cookie = this.cookie;
    let payload: BodyInit | undefined;
    if (body instanceof FormData) payload = body;
    else if (body !== undefined) {
      headers['content-type'] = 'application/json';
      payload = JSON.stringify(body);
    }
    const res = await fetch(API + path, { method, headers, body: payload });
    const setCookie = res.headers.get('set-cookie')!;
    if (setCookie) this.cookie = setCookie.split(';')[0];
    if (raw) return res;
    const text = await res.text();
    let data: any;
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
    if (data?.access_token) this.token = data.access_token;
    return { status: res.status, data };
  }

  get(p: string) { return this.req('GET', p); }
  post(p: string, b: unknown = {}) { return this.req('POST', p, b); }
  patch(p: string, b: unknown) { return this.req('PATCH', p, b); }
  put(p: string, b: unknown) { return this.req('PUT', p, b); }
  del(p: string) { return this.req('DELETE', p); }

  /** A real-time connection; received events collect in `events`. */
  async ws(): Promise<LiveSocket> {
    const { data } = await this.post('/realtime/ticket');
    const sock = new WebSocket(`ws://localhost:8080/api/realtime/ws?ticket=${data.ticket}`) as LiveSocket;
    sock.events = [];
    sock.closed = false;
    sock.onmessage = (e) => sock.events.push(JSON.parse(e.data));
    sock.onclose = () => (sock.closed = true);
    await new Promise((r) => (sock.onopen = r));
    await sleep(200);
    return sock;
  }
}

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Polls `fn` until it returns something truthy (or time runs out; then its last value). */
export async function waitFor<T>(fn: () => T | Promise<T>, ms = 20_000): Promise<T> {
  const end = Date.now() + ms;
  let v = await fn();
  while (!v && Date.now() < end) {
    await sleep(300);
    v = await fn();
  }
  return v;
}
