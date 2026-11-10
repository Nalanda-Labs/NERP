// Runs the security suites one after another against a running server
// (http://localhost:8080) and prints a summary. Usage:
//   node run.ts           every suite
//   node run.ts pen2 pen4 just these
import { spawnSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const suites = readdirSync(here)
  .filter((f) => /^pen\d+\.ts$/.test(f))
  .sort((a, b) => Number(a.match(/\d+/)![0]) - Number(b.match(/\d+/)![0]))
  .filter((f) => !args.length || args.includes(f.replace('.ts', '')));
let failed = 0;
for (const f of suites) {
  const started = Date.now();
  const r = spawnSync(process.execPath, [f], { cwd: here, encoding: 'utf8', timeout: 1_800_000 });
  const out = `${r.stdout ?? ''}${r.stderr ?? ''}`;
  const fails = out.split('\n').filter((l) => l.startsWith('FAIL'));
  const ok = r.status === 0 && !fails.length;
  if (!ok) failed++;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${f.padEnd(9)} ${((Date.now() - started) / 1000).toFixed(0).padStart(4)}s  ${out.trim().split('\n').at(-1) ?? ''}`);
  for (const l of fails.slice(0, 8)) console.log('       ' + l.slice(0, 400));
}
console.log(`\n${suites.length - failed} of ${suites.length} security suites passed`);
process.exit(failed ? 1 : 0);
