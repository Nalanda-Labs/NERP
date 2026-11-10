// Screenshots and other files the suites write go to a folder outside the repository:
// NERP_E2E_OUTPUT, or nerp-e2e in the system's temporary folder. Imported (through
// cleanup.ts) by every suite, so relative paths like `fail4-1.png` land there.
import { mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export const OUTPUT = process.env.NERP_E2E_OUTPUT ?? join(tmpdir(), 'nerp-e2e');
mkdirSync(OUTPUT, { recursive: true });
process.chdir(OUTPUT);
