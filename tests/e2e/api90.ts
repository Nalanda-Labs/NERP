// Knowledge base import: a Markdown file, a Notion-style ZIP of Markdown pages in folders (tree,
// images, links between pages), and a Confluence HTML export (tree from index.html, only the
// page's own content); only editors import, only their own uploads.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { makeZip } from './zip.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const ann = new Client();
await admin.post('/users', { email: `ann.${stamp}@acme.test`, password: 'password123', first_name: 'Ann', last_name: `Kb${stamp}` });
await ann.post('/auth/login', { email: `ann.${stamp}@acme.test`, password: 'password123' });
const png = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='), (c) => c.charCodeAt(0));
const upload = async (c: Client, name: string, body: BlobPart) => { const fd = new FormData(); fd.append('file', new Blob([body]), name); return (await c.req('POST', '/files', fd)).data.id as string; };
let spaceId = '';

try {
  const space = (await admin.post('/kb/spaces', { name: `Imported ${stamp}` })).data;
  spaceId = space.id;
  const tree = async () => (await admin.get(`/kb/spaces/${spaceId}`)).data.pages as { id: string; parent_id: string | null; title: string }[];
  const page = async (id: string) => (await admin.get(`/kb/pages/${id}`)).data;

  // ---- one Markdown file ----
  let r = await admin.post(`/kb/spaces/${spaceId}/import`, { file_id: await upload(admin, 'policy.md', `# Leave policy ${stamp}\n\nYou get **25 days**.`) });
  ok('a Markdown file becomes a page titled by its heading', r.status === 200 && r.data.pages.length === 1 && r.data.pages[0].title === `Leave policy ${stamp}`, r.data);
  ok('…without repeating the heading in the text', (await page(r.data.pages[0].id)).body.trim() === 'You get **25 days**.');

  // ---- a Notion-style export ----
  const id32 = '0123456789abcdef0123456789abcdef';
  const zip = makeZip({
    [`Export/Team ${id32}.md`]: `# Team\n\nSee [the rules](Team%20${id32}/Rules%20${id32.replace(/./g, 'a')}.md) and ![logo](Team%20${id32}/logo.png).`,
    [`Export/Team ${id32}/Rules ${id32.replace(/./g, 'a')}.md`]: '# Rules\n\nBe kind. Back to [the team](../Team%20' + id32 + '.md).',
    [`Export/Team ${id32}/logo.png`]: png,
    'Export/Guides/Setup.md': 'No heading here, just text.',
    'Export/Guides/notes.txt': 'plain notes',
    'Export/.DS_Store': 'junk',
    '__MACOSX/Export/._Team.md': 'junk',
  });
  r = await admin.post(`/kb/spaces/${spaceId}/import`, { file_id: await upload(admin, 'notion.zip', zip) });
  ok('a ZIP export imports', r.status === 200 && r.data.images === 1, r.data);
  let t = await tree();
  const team = t.find((p) => p.title === 'Team'), rules = t.find((p) => p.title === 'Rules'), guides = t.find((p) => p.title === 'Guides'), setup = t.find((p) => p.title === 'Setup');
  ok('the folder tree is kept; Notion ids leave the titles', !!team && rules?.parent_id === team.id && !!guides && setup?.parent_id === guides.id && team.parent_id === null, t);
  ok('the export\'s own top folder and hidden files aren\'t pages', !t.some((p) => p.title === 'Export' || p.title.includes('DS_Store') || p.title.startsWith('._')));
  const teamBody = (await page(team!.id)).body;
  ok('links between pages point to the imported pages', teamBody.includes(`(/kb/page/${rules!.id})`) && (await page(rules!.id)).body.includes(`(/kb/page/${team!.id})`), teamBody);
  const img = teamBody.match(/\/api\/files\/([0-9a-f-]+)\/content/)?.[1];
  ok('images become the page\'s files', !!img && (await admin.get(`/kb/pages/${team!.id}/files`)).data.items.some((f: any) => f.id === img), teamBody);
  ok('…which readers can open', !!img && (await ann.req('GET', `/files/${img}/content`, undefined, true) as Response).status === 200);
  ok('a page without a heading is titled by its file', !!setup && (await page(setup.id)).body === 'No heading here, just text.');

  // ---- a Confluence HTML export, under a chosen page ----
  const conf = makeZip({
    'SPACE/index.html': '<html><title>Confluence</title><body><div id="main-content"><ul><li><a href="Home_1001.html">Home</a><ul><li><a href="Release-notes_1002.html">Release notes</a></li></ul></li></ul></div></body></html>',
    'SPACE/Home_1001.html': '<html><head><title>SPACE : Home</title></head><body><div id="breadcrumbs">Crumbs</div><div id="main-content"><h2>Welcome</h2><p>Read the <a href="Release-notes_1002.html">notes</a>.</p></div><div id="footer">Powered by</div></body></html>',
    'SPACE/Release-notes_1002.html': '<html><head><title>SPACE : Release notes</title></head><body><div id="main-content"><ul><li>One</li><li>Two</li></ul></div></body></html>',
  });
  r = await admin.post(`/kb/spaces/${spaceId}/import`, { file_id: await upload(admin, 'confluence.zip', conf), parent_id: guides!.id });
  ok('a Confluence export imports', r.status === 200 && r.data.pages.length === 2, r.data);
  t = await tree();
  const home = t.find((p) => p.title === 'Home'), notes = t.find((p) => p.title === 'Release notes');
  ok('…its tree from index.html, under the chosen page', home?.parent_id === guides!.id && notes?.parent_id === home!.id, t);
  const homeBody = (await page(home!.id)).body;
  ok('…only the page itself, as Markdown', homeBody.includes('## Welcome') && homeBody.includes(`[notes](/kb/page/${notes!.id})`) && !homeBody.includes('Crumbs') && !homeBody.includes('Powered by'), homeBody);

  // ---- who and what ----
  ok('only editors import', (await ann.post(`/kb/spaces/${spaceId}/import`, { file_id: await upload(ann, 'x.md', '# x') })).status === 403);
  ok('only one\'s own uploads', (await admin.post(`/kb/spaces/${spaceId}/import`, { file_id: await upload(ann, 'y.md', '# y') })).status === 400);
  ok('other kinds of files are refused', (await admin.post(`/kb/spaces/${spaceId}/import`, { file_id: await upload(admin, 'a.exe', 'MZ') })).status === 400);
  ok('a damaged ZIP is refused', (await admin.post(`/kb/spaces/${spaceId}/import`, { file_id: await upload(admin, 'bad.zip', 'PK\u0003\u0004nope') })).status === 400);
  const versions = (await admin.get(`/kb/pages/${team!.id}/versions`)).data;
  ok('imported pages start their history', (versions.items ?? versions).length >= 1, versions);
} finally {
  if (spaceId) await admin.del(`/kb/spaces/${spaceId}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
