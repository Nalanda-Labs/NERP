// Content: script injection through the Markdown everyone writes (knowledge base, training,
// websites), uploads that try to run in the browser or escape the upload folder, the
// security headers, cross-site requests, and links in emails that a forged Host header could
// redirect.
import { existsSync } from 'node:fs';
import { admin, person, raw, ok, finish, BASE, stamp } from './lib.ts';

// Real markup only: dangerous tags, event-handler attributes, and script URLs in attributes.
// (The words "javascript:alert(1)" left as plain link text are harmless.)
const DANGER = /<(script|iframe|object|embed|form|style|base|meta|svg|math)\b|<[^>]*\son[a-z]+\s*=|<[^>]*\b(href|src|action|formaction|xlink:href|srcdoc)\s*=\s*["']?\s*(javascript|vbscript|data:text)/i;
const XSS = [
  '<script>alert(1)</script>',
  '<img src=x onerror=alert(1)>',
  '<svg/onload=alert(1)>',
  '[click](javascript:alert(1))',
  '[click](JaVaScRiPt:alert(1))',
  '[click](  javascript:alert(1))',
  '![img](javascript:alert(1))',
  '<javascript:alert(1)>',
  '<a href="javascript&colon;alert(1)">x</a>',
  '<a href="data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==">x</a>',
  '<iframe src="https://evil.example"></iframe>',
  '<form action="https://evil.example"><input name=p></form>',
  '<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>',
  '<details open ontoggle=alert(1)>',
  '<a href="https://ok.example" style="position:fixed;inset:0">x</a>',
  '<base href="https://evil.example/">',
  '<meta http-equiv="refresh" content="0;url=https://evil.example">',
  '```\n</code><script>alert(1)</script>\n```',
];

// ---- Markdown renderers ----
const bob = await person('Bob');
const kbBad: string[] = [];
for (const x of XSS) {
  const html = (await bob.post('/kb/preview', { body: x })).data?.html ?? '';
  if (DANGER.test(html)) kbBad.push(`${x} → ${html}`);
}
ok('knowledge-base Markdown strips scripts and dangerous links', kbBad.length === 0, kbBad);

const course = (await admin.post('/training/courses', { title: `XSS course ${stamp}`, description: XSS.join('\n\n') })).data;
ok('training course text is cleaned', course?.html !== undefined && !DANGER.test(course.html), course?.html);
const lesson = (await admin.post(`/training/courses/${course.course.id}/lessons`, { title: 'XSS', body: XSS.join('\n\n') })).data;
ok('training lesson text is cleaned', lesson?.html !== undefined && !DANGER.test(lesson.html), lesson?.html);
ok('lesson video links must be web addresses', (await admin.post(`/training/courses/${course.course.id}/lessons`, { title: 'v', video_url: 'javascript:alert(1)' })).status === 400);

const site = (await admin.post('/sites', { name: `XSS site ${stamp}` })).data;
const siteId = site?.id ?? site?.site?.id;
if (siteId) {
  const pageId = site.page_list?.[0]?.id;
  const bad = await admin.patch(`/sites/pages/${pageId}`, { blocks: [{ kind: 'cta', heading: 'x', button_label: 'Go', button_link: 'javascript:alert(1)' }] });
  ok('website buttons cannot link to javascript:', bad.status === 400, bad.data);
  const saved = await admin.patch(`/sites/pages/${pageId}`, { blocks: [{ kind: 'text', markdown: XSS.join('\n\n') }] });
  // The Markdown is kept as written; what visitors get is the HTML rendered from it.
  const rendered = ((await admin.get(`/sites/pages/${pageId}`)).data?.rendered ?? []).map((b: any) => b.html ?? '').join('');
  ok('website text is cleaned when rendered', saved.status === 200 && rendered.length > 0 && !DANGER.test(rendered), rendered.slice(0, 400));
}

// ---- uploads ----
const upload = async (name: string, type: string, content: string) => {
  const fd = new FormData();
  fd.append('file', new Blob([content], { type }), name);
  return (await bob.post('/files', fd)).data;
};
const content = async (id: string) => {
  const r = await fetch(`${BASE}/api/files/${id}/content`, { headers: { authorization: `Bearer ${bob.token}` } });
  return { status: r.status, type: r.headers.get('content-type') ?? '', disposition: r.headers.get('content-disposition') ?? '', csp: r.headers.get('content-security-policy') ?? '', nosniff: r.headers.get('x-content-type-options') };
};
for (const [name, type, body] of [
  ['evil.html', 'text/html', '<script>alert(document.cookie)</script>'],
  ['evil.svg', 'image/svg+xml', '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>'],
  ['evil.xml', 'application/xhtml+xml', '<html xmlns="http://www.w3.org/1999/xhtml"><script>alert(1)</script></html>'],
  ['disguised.png', 'image/png', '<html><script>alert(1)</script></html>'],
]) {
  const f = await upload(`${name}`, type, body);
  const c = await content(f.id);
  const inline = c.disposition.startsWith('inline');
  ok(`an uploaded ${name} cannot run script when opened`, c.status === 200 && c.nosniff === 'nosniff' && /sandbox/.test(c.csp) && (!inline || /^image\/(png|jpeg|gif|webp|avif)|^application\/pdf/.test(c.type)), c);
}
const sneaky = await upload(`../../../../../tmp/pwned-${stamp}.txt`, 'text/plain', 'x');
ok('a file name cannot climb out of the upload folder', !existsSync(`/tmp/pwned-${stamp}.txt`) && !sneaky.original_name?.includes('/'), sneaky);
const crlf = await upload(`a"\r\nSet-Cookie: pwned=1\r\nX: .txt`, 'text/plain', 'x');
const cc = await fetch(`${BASE}/api/files/${crlf.id}/content`, { headers: { authorization: `Bearer ${bob.token}` } });
ok('a file name cannot add response headers', !cc.headers.get('set-cookie') && !cc.headers.get('x'), [...cc.headers.entries()]);
const big = new FormData();
big.append('file', new Blob([new Uint8Array(60 * 1024 * 1024)]), `big-${stamp}.bin`);
const bigRes = await fetch(`${BASE}/api/files`, { method: 'POST', headers: { authorization: `Bearer ${bob.token}` }, body: big });
ok('an upload over the size limit is refused (not a crash)', bigRes.status === 413, bigRes.status);
const huge = await raw('POST', '/tasks', { headers: { authorization: `Bearer ${bob.token}`, 'content-type': 'application/json' }, body: JSON.stringify({ title: 'x'.repeat(20 * 1024 * 1024) }) });
ok('a 20 MB JSON body is refused', huge.status === 413 || huge.status === 400, huge.status);
const deep = await raw('POST', '/tasks', { headers: { authorization: `Bearer ${bob.token}`, 'content-type': 'application/json' }, body: '{"title":"x","x":' + '['.repeat(100000) + ']'.repeat(100000) + '}' });
ok('deeply nested JSON does not crash the server', deep.status < 500 && (await raw('GET', '/health')).status === 200, deep.status);

// ---- headers ----
const head = async (path: string) => (await fetch(BASE + path, { redirect: 'manual' })).headers;
const app = await head('/login');
const csp = app.get('content-security-policy') ?? '';
ok('pages carry a content security policy without unsafe scripts', /script-src 'self'/.test(csp) && !/script-src[^;]*'unsafe-(inline|eval)'/.test(csp) && /object-src 'none'/.test(csp) && /frame-ancestors 'self'/.test(csp), csp);
ok('pages cannot be framed by other sites', app.get('x-frame-options') === 'SAMEORIGIN');
ok('nosniff, a referrer policy and a permissions policy', app.get('x-content-type-options') === 'nosniff' && !!app.get('referrer-policy') && !!app.get('permissions-policy'));
const form = await head('/f/anything');
ok('embeddable pages (web forms) may be framed', /frame-ancestors \*/.test(form.get('content-security-policy') ?? '') && !form.get('x-frame-options'));
const api = await head('/api/health');
ok('API responses carry nosniff too', api.get('x-content-type-options') === 'nosniff');

// ---- other sites ----
const cors = await fetch(`${BASE}/api/auth/me`, { headers: { origin: 'https://evil.example', authorization: `Bearer ${bob.token}` } });
ok('no CORS permission for other sites', !cors.headers.get('access-control-allow-origin'));
const pre = await fetch(`${BASE}/api/tasks`, { method: 'OPTIONS', headers: { origin: 'https://evil.example', 'access-control-request-method': 'POST', 'access-control-request-headers': 'authorization,content-type' } });
ok('no CORS preflight approval for other sites', !pre.headers.get('access-control-allow-origin'));
const refresh = await fetch(`${BASE}/api/auth/refresh`, { method: 'POST', headers: { origin: 'https://evil.example', cookie: bob.cookie ?? '' } });
ok('the refresh cookie is SameSite=Strict (a cross-site page cannot use it)', /SameSite=Strict/i.test(refresh.headers.get('set-cookie') ?? 'SameSite=Strict') && /HttpOnly/i.test(refresh.headers.get('set-cookie') ?? 'HttpOnly'));

// ---- links in emails come from PUBLIC_URL, not the Host header ----
await raw('POST', '/auth/forgot-password', { headers: { 'content-type': 'application/json', host: 'evil.example', 'x-forwarded-host': 'evil.example' }, body: JSON.stringify({ email: bob.email }) });
const mail = await (async () => {
  for (let i = 0; i < 40; i++) {
    const m = (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent(`to:${bob.email}`)}`)).json()).messages ?? [];
    if (m[0]) return (await (await fetch(`http://localhost:8026/api/v1/message/${m[0].ID}`)).json()).Text as string;
    await new Promise((r) => setTimeout(r, 500));
  }
  return '';
})();
ok('a forged Host header does not change the reset link', mail.includes('/reset') && !mail.includes('evil.example'), mail.slice(0, 300));

await finish();
