// A stand-in for Twilio's Messages API on http://127.0.0.1:9466, for the SMS suites. The dev
// server talks to it when started by tools/restart-server.sh (TWILIO_API_URL, a test account
// and token), so the tests never send real messages. It records what NERP sent, refuses
// numbers Twilio would refuse, and can play Twilio's side of the webhooks: delivery reports
// and incoming messages, signed like Twilio signs them.
import { createServer, type Server } from 'node:http';
import { createHmac } from 'node:crypto';

export const TWILIO = 'http://127.0.0.1:9466';
/** The account and token restart-server.sh gives the server. */
export const SID = 'ACnerptest0000000000000000000000';
export const TOKEN = 'nerp-test-twilio-token';
/** Twilio's magic number for "not a valid phone number". */
export const INVALID = '+15005550001';

export type Sent = { sid: string; to: string; from: string | null; service: string | null; body: string; statusCallback: string | null; auth: string };

/** Twilio's request signature: base64(HMAC-SHA1(token, url + each key and value, sorted)). */
export function sign(url: string, params: Record<string, string>, token = TOKEN): string {
  const data = url + Object.keys(params).sort().map((k) => k + params[k]).join('');
  return createHmac('sha1', token).update(data).digest('base64');
}

export async function startMockTwilio(): Promise<{ sent: Sent[]; server: Server; close: () => Promise<void> }> {
  const sent: Sent[] = [];
  let n = 0;
  const server = createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      const m = req.url?.match(/^\/2010-04-01\/Accounts\/([^/]+)\/Messages\.json$/);
      if (req.method !== 'POST' || !m) {
        res.writeHead(404).end();
        return;
      }
      const p = Object.fromEntries(new URLSearchParams(raw));
      const auth = Buffer.from((req.headers.authorization ?? '').replace(/^Basic /, ''), 'base64').toString();
      const json = (code: number, v: unknown) => res.writeHead(code, { 'content-type': 'application/json' }).end(JSON.stringify(v));
      if (auth !== `${SID}:${TOKEN}` || m[1] !== SID) return json(401, { code: 20003, message: 'Authenticate', status: 401 });
      if (p.To === INVALID) return json(400, { code: 21211, message: `The 'To' number ${p.To} is not a valid phone number.`, status: 400 });
      const sid = `SM${String(++n).padStart(32, '0')}`;
      sent.push({ sid, to: p.To, from: p.From ?? null, service: p.MessagingServiceSid ?? null, body: p.Body, statusCallback: p.StatusCallback ?? null, auth });
      json(201, { sid, status: 'queued', to: p.To, from: p.From ?? null, body: p.Body, num_segments: String(Math.max(1, Math.ceil(p.Body.length / 153))) });
    });
  });
  await new Promise<void>((r) => server.listen(9466, '127.0.0.1', () => r()));
  return { sent, server, close: () => new Promise((r) => server.close(() => r())) };
}

/** Posts to one of NERP's Twilio webhooks as Twilio would (signed; `badSignature` breaks it). */
export async function webhook(base: string, path: string, params: Record<string, string>, opts: { badSignature?: boolean; publicUrl?: string } = {}): Promise<Response> {
  const all = { AccountSid: SID, ...params };
  const url = (opts.publicUrl ?? base) + path;
  const sig = opts.badSignature ? sign(url, all, 'wrong-token') : sign(url, all);
  return fetch(base + path, { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded', 'x-twilio-signature': sig }, body: new URLSearchParams(all).toString() });
}

/** Waits until `n` messages have reached the stand-in (the worker sends them). */
export async function waitFor(sent: Sent[], n: number, ms = 10_000): Promise<void> {
  const until = Date.now() + ms;
  while (sent.length < n && Date.now() < until) await new Promise((r) => setTimeout(r, 100));
}
