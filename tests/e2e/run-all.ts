// Runs the end-to-end suites one after another against a running server
// (http://localhost:8080) and prints a summary. Usage:
//   node run-all.ts            every suite
//   node run-all.ts api        only the API suites (or: ui)
//   node run-all.ts api15 ui15 just these
import { spawnSync } from 'node:child_process';
import { OUTPUT } from './output.ts';
import { readdirSync, statSync } from 'node:fs';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const started = Date.now();
const args = process.argv.slice(2);
const num = (f) => Number(f.match(/\d+/)[0]);
const all = readdirSync(here)
  .filter((f) => /^(api|ui)\d+\.ts$/.test(f))
  .sort((a, b) => (a.startsWith('api') === b.startsWith('api') ? num(a) - num(b) : a.startsWith('api') ? -1 : 1));
const chosen = all.filter((f) => {
  const name = f.replace('.ts', '');
  if (!args.length) return true;
  return args.some((a) => a === name || (a === 'api' && name.startsWith('api')) || (a === 'ui' && name.startsWith('ui')));
});
let failedSuites = 0, skipped = 0;
for (const f of chosen) {
  const started = Date.now();
  const r = spawnSync(process.execPath, [f], { cwd: here, encoding: 'utf8', timeout: 600_000 });
  const out = `${r.stdout ?? ''}${r.stderr ?? ''}`;
  const fails = out.split('\n').filter((l) => l.startsWith('FAIL'));
  const ok = r.status === 0 && !fails.length;
  if (!ok) failedSuites++;
  if (ok && /^skipped:/m.test(out)) skipped++;
  const last = out.trim().split('\n').at(-1) ?? '';
  console.log(`${!ok ? 'FAIL' : /^skipped:/m.test(out) ? 'skip' : 'ok  '} ${f.padEnd(10)} ${((Date.now() - started) / 1000).toFixed(0).padStart(4)}s  ${last}`);
  for (const l of fails.slice(0, 5)) console.log('       ' + l.slice(0, 300));
  for (const l of out.split('\n').filter((l) => l.startsWith('cleanup'))) console.log('       ' + l.slice(0, 300));
}
const shots = readdirSync(OUTPUT).filter((f) => f.startsWith('fail') && f.endsWith('.png') && statSync(`${OUTPUT}/${f}`).mtimeMs >= started);
if (shots.length) console.log(`\nScreenshots of failed steps: ${OUTPUT}`);
console.log(`\n${chosen.length - failedSuites - skipped} of ${chosen.length} suites passed${skipped ? `, ${skipped} skipped` : ''}`);
process.exit(failedSuites ? 1 : 0);
