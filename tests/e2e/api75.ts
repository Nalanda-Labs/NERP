// Knowledge base editor, server side: pictures and files placed in a page (its editors place
// them; its readers, and only they, open them) and the full Markdown rendering (heading
// anchors, task lists, footnotes, callouts, code languages), with nothing unsafe let through.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Md${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat] = await Promise.all(['Ann', 'Bob', 'Cat'].map(user));
const upload = async (c: Client, name: string, text: string, type = 'image/png') => {
  const fd = new FormData();
  fd.append('file', new Blob([text], { type }), name);
  return (await c.req('POST', '/files', fd)).data.id as string;
};

try {
  const space = (await admin.post('/kb/spaces', { name: `Editor ${stamp}`, to_all: false, readers: { users: [bob.id] }, editors: { users: [ann.id] } })).data;
  const page = (await ann.post(`/kb/spaces/${space.id}/pages`, { title: `Guide ${stamp}`, body: '' })).data;

  // ---- pictures and files ----
  const pic = await upload(ann, 'diagram.png', 'png bytes');
  let r = await ann.post(`/kb/pages/${page.id}/files`, { file_id: pic });
  ok('an editor places a picture in the page', r.status === 200 && r.data.url === `/api/files/${pic}/content` && r.data.name === 'diagram.png', r.data);
  ok('a reader of the space opens it', (await bob.req('GET', `/files/${pic}/content`, undefined, true) as Response).status === 200);
  ok('someone who can\'t read the space can\'t', [403, 404].includes((await cat.req('GET', `/files/${pic}/content`, undefined, true) as Response).status));
  ok('readers list the page\'s files', (await bob.get(`/kb/pages/${page.id}/files`)).data.items?.some((f: any) => f.id === pic));
  ok('others don\'t', (await cat.get(`/kb/pages/${page.id}/files`)).status === 404);
  ok('a reader can\'t place files', (await bob.post(`/kb/pages/${page.id}/files`, { file_id: await upload(bob, 'b.png', 'b') })).status === 403);
  const catsFile = await upload(cat, 'private.png', 'cat private');
  ok('nobody places someone else\'s private file (and so exposes it)', (await ann.post(`/kb/pages/${page.id}/files`, { file_id: catsFile })).status === 404
    && [403, 404].includes((await bob.req('GET', `/files/${catsFile}/content`, undefined, true) as Response).status));

  // ---- rendering ----
  const body = [
    '# Getting started', '', '## Install', '', '## Install', '',
    '- [x] Laptop', '- [ ] Badge', '',
    'Read this first[^why].', '', '[^why]: Because it matters.', '',
    '> [!TIP]', '> Ask your buddy.', '',
    '```sql', 'SELECT 1;', '```', '',
    '~~old~~', '',
    '| A | B |', '| :- | -: |', '| 1 | 2 |', '',
    `![diagram](/api/files/${pic}/content)`, '',
    '<h2 id="location" onclick="x()">Clobber</h2><input type="text" value="x"><code class="language-x onerror">c</code><script>alert(1)</script>',
  ].join('\n');
  r = await ann.patch(`/kb/pages/${page.id}`, { title: `Guide ${stamp}`, body, version: page.version });
  const html: string = r.data.html;
  ok('headings get anchors, unique on the page', html.includes('<h1 id="h-getting-started">') && html.includes('id="h-install"') && html.includes('id="h-install-2"'), html.slice(0, 400));
  ok('checklists', /<input[^>]*type="checkbox"/.test(html) && html.includes('checked'), html);
  ok('footnotes', html.includes('fn-why') && html.includes('footnote-definition'), html);
  ok('callouts', html.includes('markdown-alert-tip'), html);
  ok('code is marked with its language', html.includes('class="language-sql"'), html);
  ok('strikethrough, aligned tables and pictures', html.includes('<del>old</del>') && /<td[^>]*align="right"/.test(html) && html.includes(`src="/api/files/${pic}/content"`), html);
  ok('and nothing unsafe: no foreign ids, handlers, text inputs, odd classes or scripts',
    !html.includes('id="location"') && !html.includes('onclick') && !html.includes('type="text"') && ![...html.matchAll(/<input[^>]*>/g)].some((m) => !m[0].includes('checkbox')) && !html.includes('onerror') && !html.includes('<script'), html.slice(-500));
  r = await bob.post('/kb/preview', { body: '## Same as saved' });
  ok('the preview renders the same way', r.data.html.includes('<h2 id="h-same-as-saved">'), r.data);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
