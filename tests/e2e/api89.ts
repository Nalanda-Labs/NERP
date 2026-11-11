// The employee list as a CardDAV address book (read with the tsdav client, as contacts apps
// do): found from the principal, one card per employee with department and job title,
// changes picked up by sync, people who left dropped, read-only; and a vCard download for
// Outlook.
import { createDAVClient } from 'tsdav';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const BASE = 'http://localhost:8080';
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string, extra: any = {}) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Card${stamp}`, ...extra })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return Object.assign(c, { email });
};
const auth = (email: string, pw: string) => 'Basic ' + Buffer.from(`${email}:${pw}`).toString('base64');

try {
  const dept = (await admin.post('/departments', { name: `Field team ${stamp}` })).data;
  const ivy = await mk('Ivy', { position: 'Field engineer' });
  const tom = await mk('Tom');
  await admin.post(`/departments/${dept.id}/members`, { user_id: ivy.id });
  await admin.patch(`/users/${ivy.id}`, { mobile: '+44 7700 900123' });
  const pw = (await tom.post('/auth/app-passwords', { name: `Contacts ${stamp}` })).data.password;

  const wk = await fetch(`${BASE}/.well-known/carddav`, { redirect: 'manual' });
  ok('/.well-known/carddav leads to the server', wk.status === 301 && wk.headers.get('location') === '/dav/');
  const client = await createDAVClient({ serverUrl: BASE + '/dav/', credentials: { username: tom.email, password: pw }, authMethod: 'Basic', defaultAccountType: 'carddav' });
  const books = await client.fetchAddressBooks();
  ok('the principal points to one address book: Employees', books.length === 1 && books[0].displayName === 'Employees', books.map((b: any) => [b.url, b.displayName]));
  const book = books[0];
  let cards = await client.fetchVCards({ addressBook: book });
  const ivyCard = cards.find((c: any) => c.data.includes(`Ivy Card${stamp}`) || c.data.includes(`FN:Ivy Card${stamp}`));
  ok('a card per employee', cards.length > 2 && !!ivyCard, cards.length);
  ok('…with job title, department, email and phone', !!ivyCard && /TITLE:Field engineer/.test(ivyCard.data) && ivyCard.data.includes(`;Field team ${stamp}`) && ivyCard.data.includes(`ivy.${stamp}@acme.test`) && ivyCard.data.includes('TEL;TYPE=CELL:+44 7700 900123'), ivyCard?.data);
  ok('admins of the system and guests aren\'t in it', !cards.some((c: any) => /Automation/.test(c.data)));

  // A change shows up in a sync.
  const first = await client.syncCollection({ url: book.url, props: { 'd:getetag': {} }, syncLevel: 1, syncToken: undefined });
  const token0 = (first as any).syncToken ?? (first as any)[0]?.raw?.multistatus?.syncToken;
  await admin.patch(`/users/${ivy.id}`, { position: 'Field lead' });
  const raw = async (body: string, method = 'REPORT', url = book.url) => fetch(url.startsWith('http') ? url : BASE + url, { method, headers: { authorization: auth(tom.email, pw), 'content-type': 'application/xml', depth: '1' }, body });
  const tokenRes = await (await raw(`<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:sync-token/></d:prop></d:propfind>`, 'PROPFIND')).text();
  const token = tokenRes.match(/<d:sync-token>([^<]+)</)?.[1] ?? '';
  ok('the address book has a sync token', token.startsWith('urn:nerp:cards:'), tokenRes.slice(0, 300));
  // Token from before the change: Ivy's card comes back changed.
  await admin.patch(`/users/${ivy.id}`, { position: 'Field director' });
  let res = await (await raw(`<?xml version="1.0"?><d:sync-collection xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav"><d:sync-token>${token}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/><card:address-data/></d:prop></d:sync-collection>`)).text();
  ok('a sync from a token gives only what changed', res.includes('TITLE:Field director') && !res.includes(`Tom Card${stamp}`), res.slice(0, 600));
  await admin.patch(`/users/${ivy.id}`, { is_active: false });
  res = await (await raw(`<?xml version="1.0"?><d:sync-collection xmlns:d="DAV:"><d:sync-token>${token}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>`)).text();
  ok('people who leave are dropped (404 in the sync)', new RegExp(`${ivy.id}\\.vcf</d:href><d:status>HTTP/1.1 404`).test(res), res.slice(0, 600));
  cards = await client.fetchVCards({ addressBook: book });
  ok('…and gone from the list', !cards.some((c: any) => c.data.includes(`Ivy Card${stamp}`)));
  void first; void token0;

  const card = cards.find((c: any) => c.data.includes(`Tom Card${stamp}`))!;
  const get = await fetch(card.url.startsWith('http') ? card.url : BASE + card.url, { headers: { authorization: auth(tom.email, pw) } });
  ok('a card on its own', get.status === 200 && get.headers.get('content-type')?.startsWith('text/vcard') && (await get.text()).startsWith('BEGIN:VCARD'));
  const put = await fetch(card.url.startsWith('http') ? card.url : BASE + card.url, { method: 'PUT', headers: { authorization: auth(tom.email, pw), 'content-type': 'text/vcard' }, body: 'BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Hacker\r\nEND:VCARD\r\n' });
  ok('read-only: apps can\'t change the list', put.status === 403);
  const others = await fetch(`${BASE}/dav/addressbooks/${ivy.id}/employees/`, { method: 'PROPFIND', headers: { authorization: auth(tom.email, pw), depth: '0' } });
  ok('only one\'s own address book path', others.status === 404);
  ok('a wrong password gets nothing', (await fetch(book.url.startsWith('http') ? book.url : BASE + book.url, { method: 'PROPFIND', headers: { authorization: auth(tom.email, 'nope'), depth: '0' } })).status === 401);

  // Outlook: a vCard file of everyone.
  const vcf = await admin.req('GET', '/users/export.vcf', undefined, true) as Response;
  const text = await vcf.text();
  ok('a vCard download of the employee list', vcf.status === 200 && text.includes(`FN:Tom Card${stamp}`) && !text.includes(`Ivy Card${stamp}`), text.slice(0, 200));
  const guest = new Client();
  ok('…not for guests or signed-out people', (await guest.req('GET', '/users/export.vcf', undefined, true) as Response).status === 401);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
