// Authenticator-app codes (RFC 6238, SHA-1, 6 digits, 30 seconds), as a phone app makes them.
import crypto from 'node:crypto';

export function totp(secretB32: string, t = Date.now() / 1000): string {
  const alpha = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  let bits = '';
  for (const c of secretB32.replace(/=+$/, '')) bits += alpha.indexOf(c).toString(2).padStart(5, '0');
  const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
  const ctr = Buffer.alloc(8);
  ctr.writeBigUInt64BE(BigInt(Math.floor(t / 30)));
  const h = crypto.createHmac('sha1', key).update(ctr).digest();
  const o = h[19] & 15;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1e6).padStart(6, '0');
}
