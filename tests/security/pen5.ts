// Code that runs on the server: document templates. LaTeX templates can reach files through
// Lua, so only people with "Manage system" may write them; TeX itself cannot read files or
// run commands; Typst templates cannot read files and are stopped when they run too long;
// a Word "zip bomb" is refused instead of filling the memory.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { admin, person, raw, ok, finish, stamp } from './lib.ts';

const crm = await person('Cora', ['crm.admin']);
const preview = (c: typeof admin, body: unknown) => c.post('/crm/doc-templates/preview', body);
const secret = /root:x:0|nerp_rt|JWT_SECRET|DATABASE_URL|BEGIN (RSA|PRIVATE)/;

// ---- LaTeX: code, so "Manage system" only ----
ok('a CRM admin cannot add a LaTeX template', (await crm.post('/crm/doc-templates', { name: `L ${stamp}`, engine: 'latex', source: '\\documentclass{article}\\begin{document}x\\end{document}' })).status === 400);
ok('…or preview LaTeX code', (await preview(crm, { engine: 'latex', source: '\\documentclass{article}\\begin{document}x\\end{document}' })).status === 400);
const lt = (await admin.post('/crm/doc-templates', { name: `L ${stamp}`, engine: 'latex', source: '\\documentclass{article}\\begin{document}x\\end{document}' })).data;
ok('…or change the code of an existing one', (await crm.patch(`/crm/doc-templates/${lt.id}`, { source: '\\documentclass{article}\\begin{document}y\\end{document}' })).status === 400);
ok('…but may rename it (no code changes)', (await crm.patch(`/crm/doc-templates/${lt.id}`, { name: `L2 ${stamp}` })).status === 200);
ok('a CRM admin still writes Typst templates', (await crm.post('/crm/doc-templates', { name: `T ${stamp}`, engine: 'typst', source: '= Hi' })).status === 200);

{
  for (const [what, src] of [
    ['read a file with \\input', '\\documentclass{article}\\begin{document}\\input{/etc/passwd}\\end{document}'],
    ['read a file with \\openin', '\\documentclass{article}\\begin{document}\\newread\\f\\openin\\f=/etc/passwd \\read\\f to\\l \\l\\end{document}'],
    ['run a command with \\write18', '\\documentclass{article}\\begin{document}\\immediate\\write18{id > /tmp/nerp-pwned}ok\\end{document}'],
    ['run a command with Lua', '\\documentclass{article}\\begin{document}\\directlua{local p = io.popen("id"); tex.print(p and p:read("*a") or "blocked")}\\end{document}'],
  ] as const) {
    const r = await preview(admin, { engine: 'latex', source: src });
    const body = r.status === 200 ? '(pdf)' : JSON.stringify(r.data);
    ok(`even the admin's LaTeX cannot ${what}`, !secret.test(body) && !/uid=\d+/.test(body), body.slice(0, 200));
  }
}

// ---- Typst: no files, and a time limit ----
for (const src of ['#read("/etc/passwd")', '#image("/etc/passwd")', '#include "/etc/passwd"', '#json("/proc/self/environ")', '#read("../../backend/.env")', '#csv("/etc/passwd")']) {
  const r = await preview(crm, { engine: 'typst', source: src });
  ok(`Typst ${src} reads nothing`, r.status === 400 && !secret.test(JSON.stringify(r.data)), r.data);
}
const t = Date.now();
const looping = preview(crm, { engine: 'typst', source: '#for i in range(100000000000) {}' });
await new Promise((r) => setTimeout(r, 3000));
const h = Date.now();
ok('the server answers while a template loops', (await raw('GET', '/health')).status === 200 && Date.now() - h < 2000);
const lr = await looping;
ok('a looping Typst template is stopped with an explanation', lr.status === 400 && /too long|too much|stopped/.test(lr.data?.error?.message ?? ''), [lr.status, lr.data, `${Date.now() - t} ms`]);

// ---- a Word zip bomb ----
const bomb = join(tmpdir(), `nerp-bomb-${stamp}.docx`);
execFileSync('python3', ['-c', `
import zipfile, sys
with zipfile.ZipFile(sys.argv[1], 'w', zipfile.ZIP_DEFLATED) as z:
    z.writestr('[Content_Types].xml', '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>')
    z.writestr('word/document.xml', '<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{{seller.name}}</w:t></w:r></w:p></w:body></w:document>')
    with z.open('word/media/bomb.bin', 'w', force_zip64=True) as f:
        chunk = bytes(1 << 20)
        for _ in range(300): f.write(chunk)
`, bomb]);
const bytes = readFileSync(bomb);
ok('the bomb is small when packed', bytes.length < 2 * 1024 * 1024, bytes.length);
const fd = new FormData();
fd.append('file', new Blob([bytes], { type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' }), `bomb-${stamp}.docx`);
const file = (await crm.post('/files', fd)).data;
const pr = await preview(crm, { engine: 'office', file_id: file.id });
ok('a Word template that unpacks to 300 MB is refused', pr.status === 400 && /100 MB/.test(pr.data?.error?.message ?? ''), [pr.status, pr.data]);
ok('…and the server is fine', (await raw('GET', '/health')).status === 200);

await finish();
