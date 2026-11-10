// Marketing: segments (counts, preview, validation), campaigns (test send, sending,
// dedupe, opt-outs, headers), open and click tracking, unsubscribing, results and
// attribution, scheduling and cancelling, the opt-out list.
import { Client, sleep, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const MP = 'http://localhost:8026/api/v1';
const mailpit = async (q: string): Promise<any[]> => (await (await fetch(`${MP}/search?query=${encodeURIComponent(q)}`)).json()).messages ?? [];

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const sam = new Client();
const samEmail = `sam.${stamp}@acme.test`;
await admin.post('/users', { email: samEmail, password: 'password123', first_name: 'sam', last_name: 'Mkt' });
await sam.post('/auth/login', { email: samEmail, password: 'password123' });

// ---- people ----
const mail = (n: string) => `${n}.${stamp}@example.com`;
const person = async (first: string, email: string | null) =>
  (await admin.post('/crm/contact/items', { first_name: first, last_name: `Mkt${stamp}`, multifields: email ? [{ kind: 'email', value_type: 'work', value: email }] : [] })).data;
const ann = await person('Ann', mail('ann'));
const bob = await person('Bob', mail('bob'));
await person('Cal', null); // no address
await person('Dee', mail('dee')); // opted out below
const lead = (await admin.post('/crm/lead/items', { title: `Lead ${stamp}`, first_name: 'Ann', last_name: `Dup${stamp}`, multifields: [{ kind: 'email', value_type: 'work', value: mail('ann').toUpperCase() }] })).data;
await admin.post('/marketing/optouts', { email: mail('dee') });

// ---- segments ----
ok('marketing needs its permission', (await sam.get('/marketing/segments')).status === 403);
let r = await admin.post('/marketing/segments/preview', { entity_type: 3, filter: { q: `Mkt${stamp}` } });
ok('preview counts', r.status === 200 && r.data.count === 4 && r.data.with_email === 3 && r.data.opted_out === 1 && r.data.reachable === 2 && r.data.sample.length === 4, r.data);
ok('only leads, contacts or companies', (await admin.post('/marketing/segments', { name: `Deals ${stamp}`, entity_type: 2, filter: {} })).status === 400);
ok('"mine" is not a segment', (await admin.post('/marketing/segments', { name: `Mine ${stamp}`, entity_type: 3, filter: { assigned_to: 'me' } })).status === 400);
ok('bad filter rejected', (await admin.post('/marketing/segments', { name: `Bad ${stamp}`, entity_type: 3, filter: { created_from: 'yesterday' } })).status === 400);
const segC = (await admin.post('/marketing/segments', { name: `Contacts ${stamp}`, entity_type: 3, filter: { q: `Mkt${stamp}` } })).data;
const segL = (await admin.post('/marketing/segments', { name: `Leads ${stamp}`, entity_type: 1, filter: { q: `Dup${stamp}` } })).data;
ok('segments saved with counts', segC.reachable === 2 && segL.reachable === 1 && segC.type_code === 'contact', [segC, segL]);
ok('listed', (await admin.get('/marketing/segments')).data.items.some((s: any) => s.id === segC.id));

// ---- campaign ----
r = await admin.post('/marketing/campaigns', { name: `Spring ${stamp}`, segment_ids: [segC.id, segL.id], subject: `Hi {{first_name}} ${stamp}`, body: 'Hello {{first_name}},\n\nSee https://example.com/offer?x=1 for the offer.\n\nBest', from_name: 'Acme Marketing', reply_to: 'sales@acme.test' });
const camp = r.data;
ok('campaign created as a draft', r.status === 200 && camp.status === 'draft' && camp.slug.startsWith('spring-'), r.data);
ok('reply-to must be an address', (await admin.patch(`/marketing/campaigns/${camp.id}`, { reply_to: 'nope' })).status === 400);
ok('cannot send without a subject', (await admin.post(`/marketing/campaigns/${(await admin.post('/marketing/campaigns', { name: `Empty ${stamp}` })).data.id}/send`, {})).status === 400);
r = await admin.post(`/marketing/campaigns/${camp.id}/test`, { email: `test.${stamp}@example.com` });
ok('test send', r.status === 200 && r.data.filled_from, r.data);
let m = await waitFor(async () => (await mailpit(`subject:"[Test] Hi"  to:test.${stamp}@example.com`))[0], 15000);
ok('test email arrives, filled in', m && m.Subject.startsWith('[Test] Hi '), m);

r = await admin.post(`/marketing/campaigns/${camp.id}/send`, {});
ok('sending starts', r.status === 200 && r.data.status === 'sending' && r.data.recipients === 2, r.data);
ok('a sent campaign cannot be edited', (await admin.patch(`/marketing/campaigns/${camp.id}`, { subject: 'x' })).status === 400);
const done = await waitFor(async () => (await admin.get(`/marketing/campaigns/${camp.id}`)).data.status === 'sent', 30000);
const c2 = (await admin.get(`/marketing/campaigns/${camp.id}`)).data;
ok('sent: one email per address, opt-outs and missing addresses left out', done && c2.counts.sent === 2 && c2.counts.recipients === 2 && c2.skipped === 3, c2);
const annMail = await waitFor(async () => (await mailpit(`to:${mail('ann')} subject:"Hi Ann ${stamp}"`))[0], 15000);
ok('Ann got it, once', !!annMail && (await mailpit(`to:${mail('ann')} subject:"${stamp}"`)).filter((x) => !x.Subject.startsWith('[Test]')).length === 1, annMail);
ok('Dee (unsubscribed) got nothing', (await mailpit(`to:${mail('dee')}`)).length === 0);
const headers = await (await fetch(`${MP}/message/${annMail.ID}/headers`)).json();
ok('one-click unsubscribe headers', headers['List-Unsubscribe']?.[0]?.includes('/api/public/m/u/') && headers['List-Unsubscribe-Post']?.[0] === 'List-Unsubscribe=One-Click', headers);
ok('from name and reply-to', annMail.From.Name === 'Acme Marketing' && annMail.ReplyTo?.[0]?.Address === 'sales@acme.test', annMail);
const full = await (await fetch(`${MP}/message/${annMail.ID}`)).json();
const html: string = full.HTML;
const pixel = html.match(/src="([^"]*\/api\/public\/m\/o\/[^"]+)"/)?.[1];
const link = html.match(/href="([^"]*\/api\/public\/m\/c\/[^"]+)"/)?.[1]?.replace(/&amp;/g, '&');
const unsub = html.match(/href="([^"]*\/unsubscribe\/[^"]+)"/)?.[1];
ok('message has a pixel, tracked links and an unsubscribe link', pixel && link && unsub && full.Text.includes('Unsubscribe: '), { pixel, link, unsub });

// ---- tracking ----
const local = (u: string) => u.replace(/^https?:\/\/[^/]+/, 'http://localhost:8080');
let res = await fetch(local(pixel!));
ok('open pixel', res.status === 200 && res.headers.get('content-type') === 'image/gif');
res = await fetch(local(link!), { redirect: 'manual' });
const target = res.headers.get('location') ?? '';
ok('click redirects to the link with UTM tags', res.status === 303 || res.status === 302 || res.status === 307 ? target.startsWith('https://example.com/offer?x=1&utm_source=nerp&utm_medium=email&utm_campaign=' + camp.slug) : false, [res.status, target]);
ok('tampered link refused', (await fetch(local(link!).replace(/s=[0-9a-f]+/, 's=0000'), { redirect: 'manual' })).status === 400);
const token = unsub!.split('/unsubscribe/')[1];
r = await new Client().get(`/public/m/u/${token}`);
ok('unsubscribe page data (address masked)', r.status === 200 && r.data.email === `a***@example.com` && r.data.unsubscribed === false, r.data);
res = await fetch(`http://localhost:8080/api/public/m/u/${token}`, { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: 'List-Unsubscribe=One-Click' });
ok('one-click unsubscribe', res.status === 200 && (await new Client().get(`/public/m/u/${token}`)).data.unsubscribed === true);
ok('now on the opt-out list', (await admin.get(`/marketing/optouts?q=${encodeURIComponent(mail('ann'))}`)).data.items.some((o: any) => o.email === mail('ann') && o.source === 'unsubscribed' && o.campaign === `Spring ${stamp}`));
ok('unknown token', (await new Client().get('/public/m/u/nope')).status === 404);

// ---- results ----
await admin.post('/crm/lead/items', { title: `From campaign ${stamp}`, utm: { source: 'nerp', medium: 'email', campaign: camp.slug } });
await sleep(300);
r = await admin.get(`/marketing/campaigns/${camp.id}/stats`);
ok('results: opens, clicks, unsubscribes', r.data.counts.opened === 1 && r.data.counts.clicked === 1 && r.data.counts.unsubscribed === 1 && r.data.counts.sent === 2, r.data.counts);
ok('results: clicks per link', r.data.links[0]?.url.startsWith('https://example.com/offer') && r.data.links[0].clicks === 1, r.data.links);
ok('results: what it brought in', r.data.brought.some((b: any) => b.type === 'lead' && b.count === 1) && r.data.activity.length >= 1, r.data.brought);
r = await admin.get(`/marketing/campaigns/${camp.id}/recipients?status=clicked`);
ok('recipients by status', r.data.items.length === 1 && r.data.items[0].email === mail('ann') && r.data.items[0].type_code, r.data.items);
ok('the record\'s timeline notes it', (await admin.get(`/crm/items/${bob.id}/timeline`)).data.items.some((e: any) => e.kind === 'campaign' && e.data.event === 'sent'));
ok('and the unsubscribe', (await admin.get(`/crm/items/${ann.id}/timeline`)).data.items.some((e: any) => e.kind === 'campaign' && e.data.event === 'unsubscribed') || (await admin.get(`/crm/items/${lead.id}/timeline`)).data.items.some((e: any) => e.kind === 'campaign' && e.data.event === 'unsubscribed'));

// ---- a second campaign honours the new opt-out ----
const c3 = (await admin.post('/marketing/campaigns', { name: `Summer ${stamp}`, segment_ids: [segC.id], subject: `Summer ${stamp}`, body: 'Hi' })).data;
r = await admin.post(`/marketing/campaigns/${c3.id}/send`, {});
ok('unsubscribed people are left out next time', r.data.recipients === 1, r.data);
await waitFor(async () => (await admin.get(`/marketing/campaigns/${c3.id}`)).data.status === 'sent', 30000);

// ---- schedule and cancel ----
const c4 = (await admin.post('/marketing/campaigns', { name: `Autumn ${stamp}`, segment_ids: [segC.id], subject: 'Autumn', body: 'Hi' })).data;
const at = new Date(Date.now() + 2 * 3600e3).toISOString();
r = await admin.post(`/marketing/campaigns/${c4.id}/send`, { send_at: at });
ok('scheduled for later', r.data.status === 'scheduled' && (await admin.get(`/marketing/campaigns/${c4.id}`)).data.send_at);
ok('segment in use cannot be deleted', (await admin.del(`/marketing/segments/${segC.id}`)).status === 400);
ok('cancel puts it back to a draft', (await admin.post(`/marketing/campaigns/${c4.id}/cancel`, {})).status === 200 && (await admin.get(`/marketing/campaigns/${c4.id}`)).data.status === 'draft');
ok('filter campaigns by status', (await admin.get('/marketing/campaigns?status=sent')).data.items.every((c: any) => c.status === 'sent'));

// ---- opt-out list ----
ok('invalid address rejected', (await admin.post('/marketing/optouts', { email: 'nope' })).status === 400);
ok('remove from the list', (await admin.del(`/marketing/optouts/${encodeURIComponent(mail('dee'))}`)).status === 200);
ok('removed', !(await admin.get(`/marketing/optouts?q=${encodeURIComponent(mail('dee'))}`)).data.items.length);

await cleanupRun(stamp);
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
