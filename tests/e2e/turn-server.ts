// A throwaway TURN server for the video-call tests: the installed coturn (turnserver) run as
// the current user on 127.0.0.1:3478 with a fixed test account, its files in the output
// folder. Start NERP with the matching WEBRTC_ICE_SERVERS (TURN_ICE_SERVERS below).
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdirSync, openSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { OUTPUT } from './output.ts';

export const TURN_PORT = 3478;
export const TURN_USER = 'nerp-test';
export const TURN_PASSWORD = 'nerp-test-turn';
export const TURN_ICE_SERVERS = [{ urls: `turn:127.0.0.1:${TURN_PORT}?transport=udp`, username: TURN_USER, credential: TURN_PASSWORD }];
export const TURNSERVER = ['/usr/bin/turnserver', '/usr/sbin/turnserver', '/usr/local/bin/turnserver'].find((p) => existsSync(p));

export interface Turn { log(): string; stop(): void }

export async function startTurn(name: string): Promise<Turn> {
  const dir = join(OUTPUT, name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const logFile = join(dir, 'turn.log');
  const out = openSync(logFile, 'a');
  const proc: ChildProcess = spawn(TURNSERVER!, [
    '-n', // no configuration file
    '--listening-ip=127.0.0.1', '--relay-ip=127.0.0.1', `--listening-port=${TURN_PORT}`,
    '--min-port=49160', '--max-port=49260',
    '--lt-cred-mech', `--user=${TURN_USER}:${TURN_PASSWORD}`, '--realm=nerp.test',
    '--no-tls',
    // Both browsers are on this machine, so the relayed peers are loopback addresses.
    '--allow-loopback-peers',
    `--pidfile=${join(dir, 'turn.pid')}`, `--userdb=${join(dir, 'turn.db')}`,
    '--log-file=stdout', '--verbose',
  ], { stdio: ['ignore', out, out] });
  proc.unref();
  const log = () => (existsSync(logFile) ? readFileSync(logFile, 'utf8') : '');
  for (let i = 0; i < 100 && !log().includes('Relay ports initialization done'); i++) {
    if (proc.exitCode !== null) throw new Error('turnserver did not start:\n' + log().slice(-2000));
    await new Promise((r) => setTimeout(r, 100));
  }
  await new Promise((r) => setTimeout(r, 300));
  return { log, stop: () => { proc.kill('SIGTERM'); rmSync(dir, { recursive: true, force: true }); } };
}
