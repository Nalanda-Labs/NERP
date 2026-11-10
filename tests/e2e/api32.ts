// Website builder: permission, sites (addresses), pages (blocks checked, links and images
// limited), drafts versus published pages, the public site (menu, pages, rendered and cleaned
// text, images served only while published), web forms on pages, visits, deleting.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const sam = new Client();
await admin.post('/users', { email: `sam.${stamp}@acme.test`, password: 'password123', first_name: 'Sam', last_name: 'Site' });
await sam.post('/auth/login', { email: `sam.${stamp}@acme.test`, password: 'password123' });
const pub = new Client();
const PNG = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='), (c) => c.charCodeAt(0));
const slug = `acme-${stamp}`;

try {
  ok('building sites needs the permission', (await sam.get('/sites')).status === 403 && (await sam.post('/sites', { name: 'x' })).status === 403);
  ok('addresses checked', (await admin.post('/sites', { name: `Bad ${stamp}`, slug: 'Bad Address!' })).status === 400);
  let r = await admin.post('/sites', { name: `Acme site ${stamp}`, slug, settings: { color: '#118833', footer: 'Acme footer' } });
  const site = r.data;
  ok('site with a home page to start from', r.status === 200 && site.page_list.length === 1 && site.page_list[0].path === '' && site.settings.color === '#118833', r.data);
  ok('addresses are unique', (await admin.post('/sites', { name: `Again ${stamp}`, slug })).status === 400);
  ok('nothing public before publishing', (await pub.get(`/public/sites/${slug}`)).status === 404);

  // ---- blocks ----
  const home = site.page_list[0];
  ok('unknown blocks refused', (await admin.patch(`/sites/pages/${home.id}`, { blocks: [{ kind: 'script' }] })).status === 400);
  ok('unsafe links refused', (await admin.patch(`/sites/pages/${home.id}`, { blocks: [{ kind: 'cta', button_label: 'x', button_link: 'javascript:alert(1)' }] })).status === 400);
  const up = async (c: Client, name: string, bytes: BlobPart, type: string) => { const fd = new FormData(); fd.append('file', new Blob([bytes], { type }), name); return (await c.req('POST', '/files', fd)).data; };
  const pic = await up(admin, `hero-${stamp}.png`, PNG, 'image/png');
  const txt = await up(admin, `doc-${stamp}.txt`, 'hi', 'text/plain');
  const samPic = await up(sam, `sams-${stamp}.png`, PNG, 'image/png');
  ok('images must be pictures', (await admin.patch(`/sites/pages/${home.id}`, { blocks: [{ kind: 'image', image: txt.id }] })).status === 400);
  ok('and your own', (await admin.patch(`/sites/pages/${home.id}`, { blocks: [{ kind: 'image', image: samPic.id }] })).status === 400);
  const form = (await admin.post('/crm/webforms', { name: `Site form ${stamp}`, entity_type: 1, fields: [{ key: 'name', label: 'Name', kind: 'text', required: true, maps_to: 'first_name' }] })).data;
  r = await admin.patch(`/sites/pages/${home.id}`, {
    title: 'Home',
    description: 'Acme makes things',
    blocks: [
      { kind: 'hero', heading: `Welcome ${stamp}`, text: 'We make things', button_label: 'Contact', button_link: '/s/' + slug + '/contact', image: pic.id, unknown: 'dropped' },
      { kind: 'text', markdown: '## About\n\n**Since 1990** <script>alert(1)</script>' },
      { kind: 'features', heading: 'Why', items: [{ title: 'Fast', text: 'Quick' }] },
      { kind: 'form', heading: 'Write to us', form_key: form.key },
    ],
  });
  ok('saved as a draft, unknown fields dropped', r.data.blocks.length === 4 && !('unknown' in r.data.blocks[0]) && r.data.changed && !r.data.published_at, r.data);
  ok('the preview renders text', r.data.rendered[1].html.includes('<strong>Since 1990</strong>') && !r.data.rendered[1].html.includes('<script'));
  const contact = (await admin.post(`/sites/${site.id}/pages`, { title: 'Contact', blocks: [{ kind: 'contact', heading: 'Find us', phone: '+1 555 0100', email: 'hi@acme.example' }] })).data;
  ok('a second page', contact.path === 'contact');
  ok('page addresses are unique', (await admin.post(`/sites/${site.id}/pages`, { title: 'Contact' })).status === 400);
  ok('the home page keeps its address', (await admin.patch(`/sites/pages/${home.id}`, { path: 'start' })).status === 400);
  const hidden = (await admin.post(`/sites/${site.id}/pages`, { title: 'Hidden', in_menu: false, blocks: [{ kind: 'text', markdown: 'Secret' }] })).data;

  // ---- publishing ----
  await admin.post(`/sites/pages/${home.id}/publish`, {});
  r = await pub.get(`/public/sites/${slug}`);
  ok('published: the site and its menu', r.status === 200 && r.data.color === '#118833' && r.data.menu.length === 1 && r.data.menu[0].title === 'Home', r.data);
  r = await pub.get(`/public/sites/${slug}/page?path=`);
  ok('the home page', r.data.title === 'Home' && r.data.blocks[0].heading === `Welcome ${stamp}` && r.data.blocks[0].image_url === `/api/public/sites/${slug}/files/${pic.id}` && r.data.blocks[3].form_key === form.key, r.data);
  ok('unpublished pages are not public', (await pub.get(`/public/sites/${slug}/page?path=contact`)).status === 404);
  const img = await fetch(`http://localhost:8080/api/public/sites/${slug}/files/${pic.id}`);
  ok('published images are public', img.status === 200 && img.headers.get('content-type') === 'image/png');
  ok('other files are not', (await fetch(`http://localhost:8080/api/public/sites/${slug}/files/${txt.id}`)).status === 404);
  await admin.patch(`/sites/pages/${home.id}`, { blocks: [{ kind: 'text', markdown: `Changed draft ${stamp}` }] });
  ok('visitors keep seeing the published page', (await pub.get(`/public/sites/${slug}/page?path=`)).data.blocks[0].heading === `Welcome ${stamp}`);
  ok('the site shows unpublished changes', (await admin.get(`/sites/${site.id}`)).data.page_list.find((p: any) => p.id === home.id).changed);
  await admin.post(`/sites/${site.id}/publish`, {});
  r = await pub.get(`/public/sites/${slug}`);
  ok('publish all: menu without hidden pages', r.data.menu.map((m: any) => m.path).join() === ',contact', r.data.menu);
  ok('a hidden page is still reachable', (await pub.get(`/public/sites/${slug}/page?path=${hidden.path}`)).status === 200);
  ok('the image is no longer used, so no longer public', (await fetch(`http://localhost:8080/api/public/sites/${slug}/files/${pic.id}`)).status === 404);
  ok('unpublishing a page', (await admin.post(`/sites/pages/${contact.id}/unpublish`, {})).status === 200 && (await pub.get(`/public/sites/${slug}/page?path=contact`)).status === 404);
  ok('the home page stays published', (await admin.post(`/sites/pages/${home.id}/unpublish`, {})).status === 400 && (await admin.req('DELETE', `/sites/pages/${home.id}`)).status === 400);

  // ---- visits ----
  await pub.get(`/public/sites/${slug}/page?path=`);
  r = await admin.get(`/sites/${site.id}/stats`);
  ok('visits counted', r.data.items.find((v: any) => v.path === '').week >= 3, r.data);

  // ---- deleting ----
  await admin.req('DELETE', `/sites/${site.id}`);
  ok('deleted', (await pub.get(`/public/sites/${slug}`)).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
