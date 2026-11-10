// A throwaway LDAP directory for the directory tests: the installed OpenLDAP server (slapd)
// run as the current user with its own configuration and database in the output folder,
// on 127.0.0.1:3893. It holds a service account allowed to search, and employees whose
// addresses carry the run's stamp (so the usual cleanup removes the accounts they create).
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { createConnection } from 'node:net';
import { join } from 'node:path';
import { OUTPUT } from './output.ts';

export const LDAP_PORT = 3893;
export const BASE_DN = 'dc=acme,dc=test';
export const SERVICE_DN = `cn=svc,${BASE_DN}`;
export const SERVICE_PASSWORD = 'service-pass-1';
export interface Person { uid: string; given: string; sn: string; email: string; password: string; title?: string; phone?: string; department?: string }

const find = (names: string[]) => names.find((p) => existsSync(p));
const SLAPD = find(['/usr/sbin/slapd', '/usr/bin/slapd', '/usr/lib/openldap/slapd']);
const SLAPADD = find(['/usr/sbin/slapadd', '/usr/bin/slapadd']);
// Arch and Fedora keep the schemas in /etc/openldap, Debian and Ubuntu in /etc/ldap.
const SCHEMA = find(['/etc/openldap/schema', '/etc/ldap/schema']) ?? '/etc/openldap/schema';
// Debian and Ubuntu build the mdb backend as a module; elsewhere it is part of slapd.
const MDB_MODULE_DIR = ['/usr/lib/ldap', '/usr/lib/openldap', '/usr/lib64/openldap'].find((d) => existsSync(join(d, 'back_mdb.so')));

/** Writes the configuration and entries and starts slapd; returns a function that stops it
 *  and removes its files. */
export function startLdap(name: string, people: Person[]): () => void {
  const dir = join(OUTPUT, name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(join(dir, 'db'), { recursive: true });
  const conf = join(dir, 'slapd.conf');
  writeFileSync(conf, `${MDB_MODULE_DIR ? `
modulepath ${MDB_MODULE_DIR}
moduleload back_mdb` : ''}
include ${SCHEMA}/core.schema
include ${SCHEMA}/cosine.schema
include ${SCHEMA}/inetorgperson.schema
pidfile ${join(dir, 'slapd.pid')}
argsfile ${join(dir, 'slapd.args')}

database mdb
maxsize 10485760
suffix "${BASE_DN}"
rootdn "cn=root,${BASE_DN}"
rootpw root-${name}
directory ${join(dir, 'db')}

# Passwords are only for signing in; the service account reads the rest, nobody else does.
access to attrs=userPassword
  by self write
  by anonymous auth
  by * none
access to *
  by dn.exact="${SERVICE_DN}" read
  by self read
  by * none
`);
  const entry = (lines: (string | false | undefined)[]) => lines.filter(Boolean).join('\n');
  const ldif = [
    entry([`dn: ${BASE_DN}`, 'objectClass: dcObject', 'objectClass: organization', 'dc: acme', 'o: Acme']),
    entry([`dn: ${SERVICE_DN}`, 'objectClass: organizationalRole', 'objectClass: simpleSecurityObject', 'cn: svc', `userPassword: ${SERVICE_PASSWORD}`]),
    entry([`dn: ou=people,${BASE_DN}`, 'objectClass: organizationalUnit', 'ou: people']),
    ...people.map((p) => entry([
      `dn: uid=${p.uid},ou=people,${BASE_DN}`,
      'objectClass: inetOrgPerson',
      `uid: ${p.uid}`,
      `cn: ${p.given} ${p.sn}`,
      `givenName: ${p.given}`,
      `sn: ${p.sn}`,
      `mail: ${p.email}`,
      `userPassword: ${p.password}`,
      p.title && `title: ${p.title}`,
      p.phone && `telephoneNumber: ${p.phone}`,
      p.department && `departmentNumber: ${p.department}`,
    ])),
  ].join('\n\n') + '\n';
  writeFileSync(join(dir, 'data.ldif'), ldif);
  execFileSync(SLAPADD!, ['-f', conf, '-l', join(dir, 'data.ldif')], { stdio: 'pipe' });
  // -d 0: stays in the foreground as our child, without debug output.
  const proc: ChildProcess = spawn(SLAPD!, ['-f', conf, '-h', `ldap://127.0.0.1:${LDAP_PORT}/`, '-d', '0'], { stdio: 'ignore' });
  proc.unref();
  return () => {
    proc.kill('SIGTERM');
    rmSync(dir, { recursive: true, force: true });
  };
}

/** Resolves once the directory accepts connections. */
export async function ldapReady(ms = 20_000) {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    const up = await new Promise<boolean>((r) => {
      const s = createConnection(LDAP_PORT, '127.0.0.1', () => { s.end(); r(true); });
      s.on('error', () => r(false));
    });
    if (up) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error('the test directory did not start');
}

/** Why the directory suites cannot run here, if they cannot: they need OpenLDAP (slapd and
 *  slapadd) and a server whose PUBLIC_URL is the address the tests use. */
export async function directorySkipReason(admin: { get(p: string): Promise<{ data: any }> }, base: string): Promise<string | null> {
  if (!SLAPD || !SLAPADD) return 'OpenLDAP (slapd and slapadd) is not installed';
  const uri: string = (await admin.get('/directory')).data?.sso_redirect_uri ?? '';
  return uri.startsWith(base + '/') ? null : `start the server with PUBLIC_URL=${base} (it is ${uri.replace('/api/auth/sso/callback', '')})`;
}
