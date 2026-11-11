// Designed emails: a campaign's message in blocks, checked (known blocks, real addresses, only
// one's own uploaded images), previewed, and sent as email-safe HTML with the recipient's
// placeholders (escaped), tracked links, a plain-text version and publicly served images.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const waitFor = async <T>(f: () => Promise<T | null | undefined | false>, ms = 15000): Promise<T | null> => { const end = Date.now() + ms; while (Date.now() < end) { const v = await f().catch(() => null); if (v) return v as T; await sleep(400); } return null; };
const MP = 'http://localhost:8026/api/v1';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const other = new Client();
await admin.post('/users', { email: `oli.${stamp}@acme.test`, password: 'password123', first_name: 'Oli', last_name: `Ds${stamp}` });
await other.post('/auth/login', { email: `oli.${stamp}@acme.test`, password: 'password123' });
// A 1×1 PNG.
const png = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='), (c) => c.charCodeAt(0));
const upload = async (c: Client) => { const fd = new FormData(); fd.append('file', new Blob([png], { type: 'image/png' }), 'logo.png'); return (await c.req('POST', '/files', fd)).data.id as string; };
let campId = '';

try {
  const mine = await upload(admin), theirs = await upload(other);
  const design = (blocks: any[]) => ({ style: { background: '#eeeeee', accent: '#ff5500', font: 'serif', preheader: 'Spring is here, {{first_name}}' }, blocks });
  const good = design([
    { kind: 'heading', text: 'Hello {{first_name}}' },
    { kind: 'text', text: 'Our **spring** offer: [see it](https://shop.example/spring).\n\nOr visit https://shop.example' },
    { kind: 'button', label: 'Shop now', url: 'https://shop.example/now' },
    { kind: 'image', file_id: mine, alt: 'Logo', link: 'https://shop.example' },
    { kind: 'columns', left: 'Left side', right: 'Right side' },
    { kind: 'social', links: [{ network: 'linkedin', url: 'https://linkedin.example/acme' }] },
    { kind: 'divider' }, { kind: 'spacer', height: 999 },
  ]);
  ok('unknown blocks are refused', (await admin.post('/marketing/campaigns', { name: `X ${stamp}`, design: design([{ kind: 'script' }]) })).status === 400);
  ok('buttons need real addresses', (await admin.post('/marketing/campaigns', { name: `X ${stamp}`, design: design([{ kind: 'button', label: 'x', url: 'javascript:alert(1)' }]) })).status === 400);
  ok('only one\'s own uploaded images', (await admin.post('/marketing/campaigns', { name: `X ${stamp}`, design: design([{ kind: 'image', file_id: theirs }]) })).status === 400);

  let r = await admin.post('/marketing/campaigns', { name: `Designed ${stamp}`, subject: 'Hi {{first_name}}', design: good });
  ok('a designed campaign', r.status === 200 && r.data.design?.blocks.length === 8, r.data);
  campId = r.data.id;
  ok('values are bounded (a 999px space becomes 120)', r.data.design.blocks[7].height === 120 && r.data.design.style.font === 'serif');
  ok('its plain text goes in the message', r.data.body.includes('Hello {{first_name}}') && r.data.body.includes('Shop now: https://shop.example/now') && r.data.body.includes('see it (https://shop.example/spring)'), r.data.body);

  r = await admin.post('/marketing/design/preview', { design: good });
  const h: string = r.data.html;
  ok('the preview fills a sample client in', r.status === 200 && h.includes('Hello Alex') && h.includes('Spring is here, Alex'), h.slice(0, 300));
  ok('…as email-safe HTML: tables, inline styles, the page style', h.includes('role="presentation"') && h.includes('background:#eeeeee') && h.includes('Georgia') && h.includes('<strong>spring</strong>'));
  ok('…the image from a signed public address', /\/api\/public\/m\/i\/[0-9a-f-]+\/[0-9a-f]{32}/.test(h));
  const imgUrl = h.match(/http:\/\/[^"]+\/api\/public\/m\/i\/[0-9a-f-]+\/[0-9a-f]{32}/)![0];
  const img = await fetch(imgUrl);
  ok('which mail programs load without signing in', img.status === 200 && img.headers.get('content-type') === 'image/png');
  ok('a wrong signature finds nothing', (await fetch(imgUrl.replace(/.$/, (c) => (c === '0' ? '1' : '0')))).status === 404);
  ok('only marketers preview', (await other.post('/marketing/design/preview', { design: good })).status === 403);

  // A client whose name is markup: it is escaped.
  const contact = (await admin.post('/crm/contact/items', { first_name: '<b>Eve</b>', last_name: `Ds${stamp}`, multifields: [{ kind: 'email', value: `eve.${stamp}@example.com` }] })).data;
  const seg = (await admin.post('/marketing/segments', { name: `Eve ${stamp}`, entity_type: 3, filter: { q: `ds${stamp}` } })).data;
  await admin.patch(`/marketing/campaigns/${campId}`, { segment_ids: [seg.id] });
  r = await admin.post(`/marketing/campaigns/${campId}/send`, {});
  ok('it sends', r.status === 200, r.data);
  const m = await waitFor(async () => (await (await fetch(`${MP}/search?query=${encodeURIComponent(`to:eve.${stamp}@example.com`)}`)).json()).messages?.[0]);
  const full = m ? await (await fetch(`${MP}/message/${m.ID}`)).json() : null;
  const html: string = full?.HTML ?? '';
  ok('the client\'s name, escaped', html.includes('Hello &lt;b&gt;Eve&lt;/b&gt;') && !html.includes('<b>Eve</b>'), html.slice(0, 400));
  ok('links go through click tracking', /href="http:\/\/[^"]+\/api\/public\/m\/c\/[^"]+"/.test(html) && !html.includes('href="https://shop.example/now"'));
  ok('the open pixel and the unsubscribe line', html.includes('/api/public/m/o/') && html.includes('Unsubscribe'));
  ok('a plain-text version goes with it', (full?.Text ?? '').includes('Hello <b>Eve</b>') && full.Text.includes('Shop now: http'), full?.Text);

  r = await admin.post('/marketing/campaigns', { name: `Plain ${stamp}`, body: 'Plain' });
  const plainId = r.data.id;
  r = await admin.patch(`/marketing/campaigns/${plainId}`, { design: good });
  ok('a plain campaign can be designed', r.data.design !== null && r.data.body.includes('Shop now'));
  r = await admin.patch(`/marketing/campaigns/${plainId}`, { design: null });
  ok('…and go back to plain text, keeping the text', r.data.design === null && r.data.body.includes('Shop now'), r.data);
  await admin.del(`/marketing/campaigns/${plainId}`);
  await admin.del(`/crm/contact/items/${contact.id}`);
  await admin.del(`/marketing/segments/${seg.id}`);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
