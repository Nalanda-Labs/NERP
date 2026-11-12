// A SAML 2.0 identity provider for the tests, on http://127.0.0.1:9480: its metadata, and a
// sign-in address (HTTP-Redirect) that answers NERP's request with a signed response for the
// person set with `next()` — or, for the security checks, a broken one (unsigned, signed by
// another key, changed after signing, SHA-1, wrapped, for another audience, expired, or
// answering another request). Browsers get a form that posts itself to NERP; tests that ask
// for JSON get the response to post themselves.
import { createServer, type Server } from 'node:http';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { inflateRawSync } from 'node:zlib';
import { randomUUID } from 'node:crypto';
import { SignedXml } from 'xml-crypto';

export const IDP = 'http://127.0.0.1:9480';
export const IDP_ENTITY = `${IDP}/metadata`;

/** A key and self-signed certificate (PEM), made with openssl. */
function keyPair(cn: string): { key: string; cert: string } {
  const dir = mkdtempSync(join(tmpdir(), 'nerp-saml-'));
  try {
    execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2', '-subj', `/CN=${cn}`, '-keyout', join(dir, 'k.pem'), '-out', join(dir, 'c.pem')], { stdio: 'ignore' });
    return { key: readFileSync(join(dir, 'k.pem'), 'utf8'), cert: readFileSync(join(dir, 'c.pem'), 'utf8') };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
const certBody = (pem: string) => pem.replace(/-----[^-]+-----/g, '').replace(/\s+/g, '');
const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

export type Mode = 'ok' | 'unsigned' | 'wrong-key' | 'tampered' | 'sha1' | 'wrapped' | 'wrong-audience' | 'expired' | 'other-request';
export type Person = { email: string; nameId?: string; first?: string; last?: string; mode?: Mode; attacker?: string };

export function metadataXml(cert: string, withCert = true): string {
  return `<?xml version="1.0"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="${IDP_ENTITY}">
  <md:IDPSSODescriptor WantAuthnRequestsSigned="false" protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
${withCert ? `    <md:KeyDescriptor use="signing"><ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:X509Data><ds:X509Certificate>${certBody(cert)}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor>\n` : ''}    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</md:NameIDFormat>
    <md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="${IDP}/sso"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>`;
}

function assertionXml(id: string, p: Person, req: { id: string; acs: string; audience: string }, now: Date): string {
  const iso = (d: Date) => d.toISOString().replace(/\.\d+Z$/, 'Z');
  const later = new Date(now.getTime() + 5 * 60e3), earlier = new Date(now.getTime() - 60e3);
  const expired = p.mode === 'expired';
  const until = expired ? new Date(now.getTime() - 3600e3) : later;
  const issued = expired ? new Date(now.getTime() - 7200e3) : now;
  const audience = p.mode === 'wrong-audience' ? 'https://somewhere-else.example/sp' : req.audience;
  const inResponseTo = p.mode === 'other-request' ? `id-${randomUUID()}` : req.id;
  const attr = (name: string, v?: string) => v ? `<saml:Attribute Name="${name}"><saml:AttributeValue>${esc(v)}</saml:AttributeValue></saml:Attribute>` : '';
  return `<saml:Assertion xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion" ID="${id}" Version="2.0" IssueInstant="${iso(issued)}">`
    + `<saml:Issuer>${IDP_ENTITY}</saml:Issuer>`
    + `<saml:Subject><saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress">${esc(p.nameId ?? p.email)}</saml:NameID>`
    + `<saml:SubjectConfirmation Method="urn:oasis:names:tc:SAML:2.0:cm:bearer"><saml:SubjectConfirmationData NotOnOrAfter="${iso(until)}" Recipient="${req.acs}" InResponseTo="${inResponseTo}"/></saml:SubjectConfirmation></saml:Subject>`
    + `<saml:Conditions NotBefore="${iso(expired ? issued : earlier)}" NotOnOrAfter="${iso(until)}"><saml:AudienceRestriction><saml:Audience>${audience}</saml:Audience></saml:AudienceRestriction></saml:Conditions>`
    + `<saml:AuthnStatement AuthnInstant="${iso(issued)}"><saml:AuthnContext><saml:AuthnContextClassRef>urn:oasis:names:tc:SAML:2.0:ac:classes:PasswordProtectedTransport</saml:AuthnContextClassRef></saml:AuthnContext></saml:AuthnStatement>`
    + `<saml:AttributeStatement>${attr('email', p.email)}${attr('givenName', p.first)}${attr('sn', p.last)}</saml:AttributeStatement>`
    + `</saml:Assertion>`;
}

function sign(xml: string, key: { key: string; cert: string }, sha1 = false): string {
  const sig = new SignedXml({
    privateKey: key.key,
    publicCert: key.cert,
    signatureAlgorithm: sha1 ? 'http://www.w3.org/2000/09/xmldsig#rsa-sha1' : 'http://www.w3.org/2001/04/xmldsig-more#rsa-sha256',
    canonicalizationAlgorithm: 'http://www.w3.org/2001/10/xml-exc-c14n#',
  });
  sig.addReference({
    xpath: "//*[local-name(.)='Assertion']",
    digestAlgorithm: sha1 ? 'http://www.w3.org/2000/09/xmldsig#sha1' : 'http://www.w3.org/2001/04/xmlenc#sha256',
    transforms: ['http://www.w3.org/2000/09/xmldsig#enveloped-signature', 'http://www.w3.org/2001/10/xml-exc-c14n#'],
  });
  sig.computeSignature(xml, { location: { reference: "//*[local-name(.)='Assertion']/*[local-name(.)='Issuer']", action: 'after' } });
  return sig.getSignedXml();
}

export function responseXml(p: Person, req: { id: string; acs: string; audience: string }, keys: { good: { key: string; cert: string }; other: { key: string; cert: string } }): string {
  const now = new Date();
  const aid = `_${randomUUID()}`;
  let assertion = assertionXml(aid, p, req, now);
  switch (p.mode) {
    case 'unsigned': break;
    case 'wrong-key': assertion = sign(assertion, keys.other); break;
    case 'sha1': assertion = sign(assertion, keys.good, true); break;
    case 'tampered': assertion = sign(assertion, keys.good).replace(esc(p.nameId ?? p.email), esc(p.attacker ?? 'mallory@evil.example')).replace(`>${esc(p.email)}<`, `>${esc(p.attacker ?? 'mallory@evil.example')}<`); break;
    case 'wrapped': {
      // The genuine signed assertion hidden away, a forged unsigned one in its place.
      const signed = sign(assertion, keys.good);
      const forged = assertionXml(`_${randomUUID()}`, { ...p, email: p.attacker ?? 'mallory@evil.example', nameId: p.attacker ?? 'mallory@evil.example' }, req, now);
      assertion = `${forged}<samlp:Extensions>${signed}</samlp:Extensions>`;
      break;
    }
    default: assertion = sign(assertion, keys.good);
  }
  return `<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol" xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion" ID="_${randomUUID()}" Version="2.0" IssueInstant="${now.toISOString().replace(/\.\d+Z$/, 'Z')}" Destination="${req.acs}" InResponseTo="${req.id}">`
    + `<saml:Issuer>${IDP_ENTITY}</saml:Issuer><samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>`
    + assertion + `</samlp:Response>`;
}

export type MockIdp = { close: () => Promise<void>; next: (p: Person) => void; metadata: (withCert?: boolean) => string; requests: { id: string; acs: string; issuer: string; signed: boolean }[] };

export async function startMockIdp(): Promise<MockIdp> {
  const keys = { good: keyPair('Mock IdP'), other: keyPair('Someone else') };
  let person: Person = { email: 'nobody@example.com' };
  const requests: MockIdp['requests'] = [];
  const server: Server = createServer((req, res) => {
    const url = new URL(req.url ?? '/', IDP);
    if (url.pathname === '/metadata') {
      res.writeHead(200, { 'content-type': 'application/samlmetadata+xml' }).end(metadataXml(keys.good.cert));
      return;
    }
    if (url.pathname === '/metadata-nocert') {
      res.writeHead(200, { 'content-type': 'application/samlmetadata+xml' }).end(metadataXml(keys.good.cert, false));
      return;
    }
    if (url.pathname === '/sso') {
      const raw = inflateRawSync(Buffer.from(url.searchParams.get('SAMLRequest') ?? '', 'base64')).toString();
      const id = /\sID="([^"]+)"/.exec(raw)?.[1] ?? '';
      const acs = /AssertionConsumerServiceURL="([^"]+)"/.exec(raw)?.[1] ?? '';
      const issuer = /<(?:\w+:)?Issuer[^>]*>([^<]+)</.exec(raw)?.[1] ?? '';
      requests.push({ id, acs, issuer, signed: url.searchParams.has('Signature') && url.searchParams.has('SigAlg') });
      const xml = responseXml(person, { id, acs, audience: issuer }, keys);
      const body = Buffer.from(xml).toString('base64');
      const relay = url.searchParams.get('RelayState') ?? '';
      if ((req.headers.accept ?? '').includes('application/json')) {
        res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ acs, SAMLResponse: body, RelayState: relay, xml }));
      } else {
        res.writeHead(200, { 'content-type': 'text/html' }).end(`<!doctype html><form method="post" action="${esc(acs)}"><input type="hidden" name="SAMLResponse" value="${body}"><input type="hidden" name="RelayState" value="${esc(relay)}"><button>Continue</button></form><script>document.forms[0].submit()</script>`);
      }
      return;
    }
    res.writeHead(404).end();
  });
  await new Promise<void>((r) => server.listen(9480, '127.0.0.1', () => r()));
  return {
    close: () => new Promise<void>((r) => server.close(() => r())),
    next: (p) => { person = p; },
    metadata: (withCert = true) => metadataXml(keys.good.cert, withCert),
    requests,
  };
}
