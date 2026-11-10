// A minimal OpenID Connect provider for the sign-on tests: discovery, an authorize endpoint
// that signs in whoever the test says (`provider.as`) without a login form, a token endpoint
// that checks the client secret and PKCE, and userinfo. `provider.tamper` corrupts the next
// ID token to test that NERP rejects it.
import { createServer, type Server } from 'node:http';
import { createHash, randomBytes } from 'node:crypto';

export interface Identity { sub: string; email?: string; given_name?: string; family_name?: string; name?: string; email_verified?: boolean }

export interface MockOidc {
  issuer: string;
  clientId: string;
  clientSecret: string;
  as: Identity;
  /** Applied to the next ID token's claims. */
  tamper: ((claims: Record<string, unknown>) => void) | null;
  /** Leave the email out of the ID token (it is then only in userinfo). */
  emailOnlyInUserinfo: boolean;
  /** Answer the next authorize request with this error. */
  deny: string | null;
  close(): Promise<void>;
}

const b64 = (v: string | Buffer) => Buffer.from(v).toString('base64url');

export async function startMockOidc(port: number, clientId = 'nerp-test', clientSecret = 'test-secret-' + randomBytes(6).toString('hex')): Promise<MockOidc> {
  const issuer = `http://localhost:${port}`;
  const codes = new Map<string, { challenge: string; nonce: string; redirect: string; who: Identity }>();
  const tokens = new Map<string, Identity>();
  let server: Server;
  const mock: MockOidc = {
    issuer, clientId, clientSecret,
    as: { sub: 'nobody' },
    tamper: null,
    emailOnlyInUserinfo: false,
    deny: null,
    close: () => new Promise((r) => server.close(() => r())),
  };
  server = createServer(async (req, res) => {
    const url = new URL(req.url!, issuer);
    const json = (status: number, body: unknown) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
    if (url.pathname === '/.well-known/openid-configuration') {
      return json(200, {
        issuer,
        authorization_endpoint: `${issuer}/authorize`,
        token_endpoint: `${issuer}/token`,
        userinfo_endpoint: `${issuer}/userinfo`,
        token_endpoint_auth_methods_supported: ['client_secret_basic'],
        response_types_supported: ['code'],
      });
    }
    if (url.pathname === '/authorize') {
      const q = url.searchParams;
      const back = new URL(q.get('redirect_uri')!);
      back.searchParams.set('state', q.get('state') ?? '');
      if (mock.deny) {
        back.searchParams.set('error', 'access_denied');
        back.searchParams.set('error_description', mock.deny);
        mock.deny = null;
      } else if (q.get('client_id') !== clientId || q.get('response_type') !== 'code' || q.get('code_challenge_method') !== 'S256' || !q.get('scope')?.split(' ').includes('openid')) {
        back.searchParams.set('error', 'invalid_request');
      } else {
        const code = randomBytes(12).toString('hex');
        codes.set(code, { challenge: q.get('code_challenge')!, nonce: q.get('nonce') ?? '', redirect: q.get('redirect_uri')!, who: { ...mock.as } });
        back.searchParams.set('code', code);
      }
      res.writeHead(302, { location: back.toString() });
      return res.end();
    }
    if (url.pathname === '/token' && req.method === 'POST') {
      let raw = '';
      for await (const chunk of req) raw += chunk;
      const form = new URLSearchParams(raw);
      const auth = Buffer.from((req.headers.authorization ?? '').replace(/^Basic /, ''), 'base64').toString();
      const [id, secret] = auth.split(':').map(decodeURIComponent);
      if (id !== clientId || secret !== clientSecret) return json(401, { error: 'invalid_client' });
      const c = codes.get(form.get('code') ?? '');
      codes.delete(form.get('code') ?? '');
      if (!c || form.get('grant_type') !== 'authorization_code' || form.get('redirect_uri') !== c.redirect) return json(400, { error: 'invalid_grant' });
      const verifier = form.get('code_verifier') ?? '';
      if (createHash('sha256').update(verifier).digest('base64url') !== c.challenge) return json(400, { error: 'invalid_grant', error_description: 'PKCE verification failed' });
      const now = Math.floor(Date.now() / 1000);
      const claims: Record<string, unknown> = { iss: issuer, aud: clientId, nonce: c.nonce, iat: now, exp: now + 300, ...c.who };
      if (mock.emailOnlyInUserinfo) delete claims.email;
      if (mock.tamper) { mock.tamper(claims); mock.tamper = null; }
      const access = randomBytes(12).toString('hex');
      tokens.set(access, c.who);
      return json(200, { access_token: access, token_type: 'Bearer', expires_in: 300, id_token: `${b64(JSON.stringify({ alg: 'RS256', typ: 'JWT' }))}.${b64(JSON.stringify(claims))}.${b64('not-checked')}` });
    }
    if (url.pathname === '/userinfo') {
      const who = tokens.get((req.headers.authorization ?? '').replace(/^Bearer /, ''));
      return who ? json(200, who) : json(401, { error: 'invalid_token' });
    }
    json(404, { error: 'not_found' });
  });
  await new Promise<void>((r) => server.listen(port, '127.0.0.1', () => r()));
  return mock;
}
