import crypto from 'node:crypto';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api', MP = 'http://localhost:8026/api/v1';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : extra); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));


async function mailTo(addr, subjectHas) {
  for (let i = 0; i < 40; i++) {
    const r = await (await fetch(`${MP}/search?query=${encodeURIComponent('to:' + addr)}&limit=5`)).json();
    const m = r.messages?.find((x) => x.Subject.includes(subjectHas));
    if (m) return (await (await fetch(`${MP}/message/${m.ID}`)).json());
    await sleep(250);
  }
  return null;
}

function totp(secretB32, t = Date.now() / 1000) {
  const alpha = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'; let bits = '';
  for (const c of secretB32.replace(/=+$/, '')) bits += alpha.indexOf(c).toString(2).padStart(5, '0');
  const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
  const ctr = Buffer.alloc(8); ctr.writeBigUInt64BE(BigInt(Math.floor(t / 30)));
  const h = crypto.createHmac('sha1', key).update(ctr).digest(); const o = h[19] & 15;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1e6).padStart(6, '0');
}

const stamp = Date.now().toString(36);
const daveEmail = `dave.${stamp}@acme.test`;

// --- admin + realtime ---
const admin = new Client(null, 'Mozilla/5.0 (X11; Linux x86_64) Firefox/140.0');
let r: any = await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
ok('admin login', r.status === 200, JSON.stringify(r.data));
const adminWs = await admin.ws();
ok('ws hello', adminWs.events.some((e) => e.type === 'hello'));
// Its own department (a fresh database has none; the stamp gets it cleaned up).
const sales = (await admin.post('/departments', { name: `Sales ${stamp}` })).data;

// --- invitation ---
r = await admin.post('/users', { email: daveEmail, first_name: 'Dave', last_name: 'Diaz', send_invite: true, department_ids: [sales.id] });
ok('invite user', r.status === 200 && r.data.invite_pending === true, JSON.stringify(r.data));
const daveId = r.data.id;
r = await new Client().post('/auth/login', { email: daveEmail, password: 'whatever123' });
ok('pending user cannot log in', r.status === 401);
let mail = await mailTo(daveEmail, 'invited you');
ok('invitation email delivered', !!mail, 'no mail');
const inviteToken = mail?.Text.match(/accept-invite\?token=([\w-]+)/)?.[1];
ok('invite link in email (text + html)', !!inviteToken && mail.HTML.includes('Accept invitation'));
r = await new Client().get(`/auth/invite/${inviteToken}`);
ok('invite info', r.data.email === daveEmail && r.data.company_name === 'Acme Ltd', JSON.stringify(r.data));
const dave = new Client(null, 'Mozilla/5.0 (Macintosh) Safari/605.1.15');
r = await dave.post('/auth/accept-invite', { token: inviteToken, password: 'davepassword1' });
ok('accept invite -> signed in', r.status === 200 && r.data.user.invite_pending === false, JSON.stringify(r.data));
r = await new Client().post('/auth/accept-invite', { token: inviteToken, password: 'davepassword1' });
ok('invite link is single-use', r.status === 400);
await sleep(400);
ok('admin got live "accepted" notification', adminWs.events.some((e) => e.type === 'notification' && e.data.kind === 'user.invite_accepted'), JSON.stringify(adminWs.events));

// --- notifications ---
r = await dave.get('/notifications');
ok('dave has welcome + department notifications', ['user.welcome', 'department.added'].every((k) => r.data.items.some((n) => n.kind === k)), JSON.stringify(r.data.items.map((n) => n.kind)));
r = await dave.get('/notifications/unread-count');
const unread = r.data.count;
const daveWs = await dave.ws();
await admin.put(`/users/${daveId}/roles`, { role_ids: [(await admin.get('/roles')).data.find((x) => x.code === 'hr').id] });
await sleep(400);
ok('dave gets live role notification + me.changed', daveWs.events.some((e) => e.type === 'notification' && e.data.kind === 'user.roles') && daveWs.events.some((e) => e.type === 'me.changed'));
r = await dave.post('/notifications/read-all');
ok('read-all', r.data.unread === 0 && unread > 0, JSON.stringify(r.data));
await sleep(300);
ok('read sync event', daveWs.events.some((e) => e.type === 'notifications.read'));

// --- sessions ---
const dave2 = new Client(null, 'Mozilla/5.0 (Windows NT 10.0) Chrome/150.0');
await dave2.post('/auth/login', { email: daveEmail, password: 'davepassword1' });
const dave2Ws = await dave2.ws();
r = await dave.get('/auth/sessions');
ok('two sessions listed, one current', r.data.length === 2 && r.data.filter((s) => s.current).length === 1, JSON.stringify(r.data));
const other = r.data.find((s) => !s.current);
ok('session records device', other.user_agent.includes('Chrome'));
r = await dave.del(`/auth/sessions/${other.id}`);
await sleep(400);
ok('revoked session: access token rejected', (await dave2.get('/auth/me')).status === 401);
ok('revoked session: refresh rejected', (await dave2.post('/auth/refresh')).status === 401);
ok('revoked session: websocket told + closed', dave2Ws.events.some((e) => e.type === 'session.revoked') && dave2Ws.closed);
ok('current session still fine', (await dave.get('/auth/me')).status === 200);

// --- forgot / reset password ---
r = await new Client().post('/auth/forgot-password', { email: daveEmail });
ok('forgot-password ok', r.status === 200);
r = await new Client().post('/auth/forgot-password', { email: 'nobody@nowhere.test' });
ok('forgot-password does not reveal unknown emails', r.status === 200 && r.data.ok === true);
mail = await mailTo(daveEmail, 'Reset your');
const resetToken = mail?.Text.match(/reset-password\?token=([\w-]+)/)?.[1];
ok('reset email delivered', !!resetToken);
r = await new Client().post('/auth/reset-password', { token: resetToken, password: 'newdavepass2' });
ok('reset password', r.status === 200, JSON.stringify(r.data));
await sleep(300);
ok('reset signs out all sessions', (await dave.get('/auth/me')).status === 401 && daveWs.closed);
ok('reset link single-use', (await new Client().post('/auth/reset-password', { token: resetToken, password: 'xxxxxxxxx1' })).status === 400);
const d3 = new Client();
ok('login with new password', (await d3.post('/auth/login', { email: daveEmail, password: 'newdavepass2' })).status === 200);

// --- two-factor ---
r = await d3.post('/auth/2fa/setup', { password: 'wrong' });
ok('2fa setup needs password', r.status === 400);
r = await d3.post('/auth/2fa/setup', { password: 'newdavepass2' });
ok('2fa setup gives secret + QR', !!r.data.secret && r.data.qr_png_base64.length > 100 && r.data.otpauth_url.startsWith('otpauth://totp/'), JSON.stringify(r.data).slice(0, 200));
const secret = r.data.secret;
ok('2fa enable rejects bad code', (await d3.post('/auth/2fa/enable', { code: '000000' })).status === 400);
r = await d3.post('/auth/2fa/enable', { code: totp(secret) });
ok('2fa enabled, 10 recovery codes', r.status === 200 && r.data.recovery_codes?.length === 10, JSON.stringify(r.data));
const codes = r.data.recovery_codes;
r = await new Client().post('/auth/login', { email: daveEmail, password: 'newdavepass2' });
ok('login now asks for 2fa', r.data.two_factor_required === true && !!r.data.challenge && !r.data.access_token);
const challenge = r.data.challenge;
ok('wrong 2fa code rejected', (await new Client().post('/auth/login/2fa', { challenge, code: '123456' })).status === 401);
const d4 = new Client();
r = await d4.post('/auth/login/2fa', { challenge, code: totp(secret, Date.now() / 1000 + 30) });
ok('2fa login with TOTP', r.status === 200 && !!r.data.access_token, JSON.stringify(r.data));
r = await new Client().post('/auth/login', { email: daveEmail, password: 'newdavepass2' });
const d5 = new Client();
r = await d5.post('/auth/login/2fa', { challenge: r.data.challenge, code: codes[0] });
ok('2fa login with recovery code', r.status === 200);
r = await new Client().post('/auth/login', { email: daveEmail, password: 'newdavepass2' });
ok('recovery code is single-use', (await new Client().post('/auth/login/2fa', { challenge: r.data.challenge, code: codes[0] })).status === 401);
ok('2fa status shows 9 codes left', (await d5.get('/auth/2fa')).data.recovery_codes_remaining === 9);
ok('user shows two_factor_enabled', (await admin.get(`/users/${daveId}`)).data.two_factor_enabled === true);
r = await admin.post(`/users/${daveId}/2fa/reset`);
ok('admin resets 2fa', r.status === 200 && (await admin.get(`/users/${daveId}`)).data.two_factor_enabled === false);

// --- admin: jobs + email test + revoke all sessions ---
r = await admin.get('/admin/system');
ok('system info', r.status === 200 && r.data.jobs.done > 0, JSON.stringify(r.data));
r = await admin.get('/admin/jobs?kind=email.send&limit=5');
ok('jobs list hides email bodies', r.data.items.length > 0 && !('html' in r.data.items[0].payload) && r.data.items[0].payload.subject, JSON.stringify(r.data.items[0]));
r = await admin.post('/admin/email/test', {});
ok('test email queued', r.status === 200);
ok('test email delivered', !!(await mailTo('admin@acme.test', 'test email')));
ok('employee cannot see jobs', (await d5.get('/admin/jobs')).status === 403);
r = await admin.get(`/users/${daveId}/sessions`);
ok('admin sees user sessions', r.data.length >= 2, JSON.stringify(r.data));
await admin.post(`/users/${daveId}/sessions/revoke`);
ok('admin sign-out-everywhere', (await d5.get('/auth/me')).status === 401 && (await d4.get('/auth/me')).status === 401);

// --- logout kills session ---
r = await admin.post('/auth/logout');
ok('logout revokes access token immediately', (await admin.get('/auth/me')).status === 401);

adminWs.close(); daveWs.close();
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
