// Importing contacts from vCard files (2.1 and 3.0, as phones and address books export them)
// and from Gmail, Outlook and Yahoo Mail CSV exports (UTF-16 and Windows-1252 included, with
// country names and their birthday formats), with duplicates matched by phone or email.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const digits = String(Date.now()).slice(-7);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });

const upload = async (name: string, body: BlobPart, type = 'text/plain') => {
  const fd = new FormData();
  fd.append('file', new Blob([body], { type }), name);
  return (await admin.post('/files', fd)).data.id as string;
};
const run = async (body: any) => {
  const r = await admin.post('/crm/contact/import', body);
  if (r.status !== 200) return { error: r.data, status: r.status };
  for (let i = 0; i < 60; i++) {
    await sleep(400);
    const st = (await admin.get(`/crm/imports/${r.data.job_id}`)).data;
    if (st.state === 'done') return st;
  }
  return null;
};
const find = async (q: string) => (await admin.get(`/crm/contact/items?q=${encodeURIComponent(q)}&limit=50`)).data.items as any[];
const detail = async (id: string) => (await admin.get(`/crm/contact/items/${id}`)).data;

try {
  // ---- vCard ----
  const vcf = [
    'BEGIN:VCARD', 'VERSION:3.0', `N:Vee${stamp};Jane;;Dr.;`, `FN:Dr. Jane Vee${stamp}`, `ORG:Cardco ${stamp};Sales`, 'TITLE:Buyer',
    `TEL;TYPE=CELL:+1 555 ${digits}`, `item1.EMAIL;TYPE=INTERNET:jane.${stamp}@cards.example`, 'ADR;TYPE=WORK:;;1 Main St;Springfield;IL;62701;US',
    'BDAY:1980-04-02', 'NOTE:Met at the fair\\, twice', 'IMPP:telegram:jane_v', 'END:VCARD',
    'BEGIN:VCARD', 'VERSION:2.1', `N;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:M=C3=BCller${stamp};J=C3=B6=`, 'rg', `TEL;CELL;PREF:+49 30 ${digits}`, 'END:VCARD',
    'BEGIN:VCARD', 'VERSION:4.0', `FN:Solo Vee${stamp}`, `EMAIL:solo.${stamp}@cards.example`, 'END:VCARD',
    'BEGIN:VCARD', 'VERSION:3.0', 'NOTE:no name, phone or email', 'END:VCARD',
  ].join('\r\n');
  let file = await upload(`contacts-${stamp}.vcf`, vcf, 'text/vcard');
  ok('vCards are for contacts only', (await admin.post('/crm/company/import', { file_id: file, mapping: {}, format: 'vcard' })).status === 400);
  ok('unknown formats are refused', (await admin.post('/crm/contact/import', { file_id: file, mapping: {}, format: 'xml' })).status === 400);
  let st = await run({ file_id: file, mapping: {}, format: 'vcard', duplicates: 'skip' });
  ok('a vCard file imports', st?.created === 3 && st.processed === 3 && !st.errors.length, st);
  const jane = (await find(`vee${stamp}`)).find((c) => c.first_name === 'Jane');
  const j = jane && (await detail(jane.id));
  ok('names, honorific, job title', j?.last_name === `Vee${stamp}` && j.honorific === 'Dr.' && j.job_title === 'Buyer', j);
  ok('the company is found or added by name', j?.company_title === `Cardco ${stamp}`, j?.company_title);
  ok('phones, emails and messengers', j?.multifields.some((m: any) => m.kind === 'phone' && m.value_type === 'mobile') && j.multifields.some((m: any) => m.kind === 'email' && m.value === `jane.${stamp}@cards.example`) && j.multifields.some((m: any) => m.kind === 'im' && m.value_type === 'telegram'), j?.multifields);
  ok('address, birthday, notes', j?.address?.city === 'Springfield' && j.address.country === 'US' && j.birthdate === '1980-04-02' && j.comments === 'Met at the fair, twice', j);
  ok('2.1 cards in quoted-printable, any charset', (await find(`üller${stamp}`)).some((c) => c.first_name === 'Jörg'));
  ok('a card with only FN', (await find(`vee${stamp}`)).some((c) => c.first_name === 'Solo' && c.last_name === `Vee${stamp}`));
  st = await run({ file_id: file, mapping: {}, format: 'vcard', duplicates: 'skip' });
  ok('importing again skips them by phone or email', st?.created === 0 && st.skipped === 3, st);

  // ---- Outlook: UTF-16 with a byte-order mark, country names, US birthdays ----
  const outlook = `"First Name","Last Name","Title","Company","Job Title","Business City","Business Country/Region","Mobile Phone","E-mail Address","Birthday","Notes"\r\n` +
    `"Olga","Look${stamp}","Ms.","Outco ${stamp}","Buyer","Paris","France","+33 1 ${digits}","olga.${stamp}@out.example","1/31/1985","From Outlook"\r\n` +
    `"Otto","Look${stamp}","","","","","","","otto.${stamp}@out.example","0/0/00",""\r\n`;
  const utf16 = new Uint8Array([0xff, 0xfe, ...Array.from(outlook).flatMap((c) => { const n = c.charCodeAt(0); return [n & 0xff, n >> 8]; })]);
  file = await upload(`outlook-${stamp}.csv`, utf16, 'text/csv');
  const outMap = { 0: 'first_name', 1: 'last_name', 2: 'honorific', 3: 'company', 4: 'job_title', 5: 'address.city', 6: 'address.country', 7: 'phone', 8: 'email', 9: 'birthdate', 10: 'comments' };
  st = await run({ file_id: file, mapping: outMap, duplicates: 'skip' });
  ok('an Outlook export (UTF-16) imports', st?.created === 2 && !st.errors.length, st);
  const olga = (await find(`look${stamp}`)).find((c) => c.first_name === 'Olga');
  const o = olga && (await detail(olga.id));
  ok('…its birthday read, its country name left out rather than failing the row', o?.birthdate === '1985-01-31' && o.address?.city === 'Paris' && !o.address?.country && o.honorific === 'Ms.', o);
  ok('…and "0/0/00" means no birthday', (await find(`look${stamp}`)).some((c) => c.first_name === 'Otto' && !c.birthdate));

  // ---- Google: several values in a cell; Windows-1252 ----
  const google = `First Name,Last Name,Organization Name,E-mail 1 - Value,Phone 1 - Value,Address 1 - Country,Birthday\r\nGr\xe9ta,Goo${stamp},Gco ${stamp},greta.${stamp}@g.example ::: greta2.${stamp}@g.example,+44 20 ${digits} ::: +44 7700 ${digits},GB,1990-07-15\r\n`;
  file = await upload(`google-${stamp}.csv`, new Uint8Array(Array.from(google).map((c) => c.charCodeAt(0))), 'text/csv');
  st = await run({ file_id: file, mapping: { 0: 'first_name', 1: 'last_name', 2: 'company', 3: 'email', 4: 'phone', 5: 'address.country', 6: 'birthdate' }, duplicates: 'skip' });
  ok('a Google export imports', st?.created === 1 && !st.errors.length, st);
  const g = (await find(`goo${stamp}`))[0];
  const gd = g && (await detail(g.id));
  ok('…with each of a cell\'s values, and Windows-1252 text', gd?.first_name === 'Gréta' && gd.multifields.filter((m: any) => m.kind === 'email').length === 2 && gd.multifields.filter((m: any) => m.kind === 'phone').length === 2 && gd.address?.country === 'GB', gd);

  // ---- Yahoo: updating matches ----
  const yahoo = `First,Last,Email,Mobile,Company,Title\r\nJane,Vee${stamp},jane.${stamp}@cards.example,,Cardco ${stamp},Head buyer\r\n`;
  file = await upload(`yahoo-${stamp}.csv`, yahoo, 'text/csv');
  st = await run({ file_id: file, mapping: { 0: 'first_name', 1: 'last_name', 2: 'email', 3: 'phone', 4: 'company', 5: 'job_title' }, duplicates: 'update' });
  ok('a Yahoo export updates the contact it matches by email', st?.updated === 1 && st.created === 0, st);
  ok('…with its newer details', (await detail(jane.id)).job_title === 'Head buyer');
} finally {
  for (const q of [`vee${stamp}`, `üller${stamp}`, `look${stamp}`, `goo${stamp}`]) for (const c of await find(q)) await admin.del(`/crm/contact/items/${c.id}`);
  for (const q of [`cardco ${stamp}`, `outco ${stamp}`, `gco ${stamp}`]) for (const c of (await admin.get(`/crm/company/items?q=${encodeURIComponent(q)}`)).data.items ?? []) await admin.del(`/crm/company/items/${c.id}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
