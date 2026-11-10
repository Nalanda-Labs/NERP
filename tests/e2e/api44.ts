// Accounting: setup, permissions, chart of accounts, taxes, journal entries (drafts, posting,
// reversal, the lock date), CRM invoices in the books (tax, rounding, foreign currency, the
// CRM lock, voiding), bills, payments (partial, full, exchange differences, advances, two at
// once), bank statements (import, re-import, match, record, pay, ignore), reports (trial
// balance, profit and loss, balance sheet, ledger, ageing, tax, statement, CSV) and year-end
// closing. After each part the books are checked directly: every entry balances, the trial
// balance and balance sheet balance, receivables equal what customers owe.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const q1 = async (sql: string, args: unknown[] = []) => (await db.query(sql, args)).rows;
const day = (offset = 0) => { const d = new Date(); d.setUTCDate(d.getUTCDate() + offset); return d.toISOString().slice(0, 10); };
const n = (x: unknown) => Number(x);
const user = async (first: string, perms: string[]) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Books' })).data;
  if (perms.length) {
    const role = (await admin.post('/roles', { code: `acc_${first.toLowerCase()}_${stamp}`, name: `Accounting ${first} ${stamp}`, permissions: perms })).data;
    const emp = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
    await admin.put(`/users/${u.id}/roles`, { role_ids: [emp.id, role.id] });
  }
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};

/** The books, checked from the database and the reports. */
const invariants = async (when: string) => {
  const unbalanced = await q1(`SELECT e.id, e.number, SUM(l.debit) d, SUM(l.credit) c FROM acc_entries e JOIN acc_lines l ON l.entry_id = e.id
                               WHERE e.status <> 'draft' GROUP BY e.id, e.number HAVING SUM(l.debit) <> SUM(l.credit) OR SUM(l.debit) = 0`);
  const both = await q1(`SELECT id FROM acc_lines WHERE debit <> 0 AND credit <> 0 OR debit < 0 OR credit < 0`);
  const dates = await q1(`SELECT l.id FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id WHERE l.date <> e.date`);
  const nonumber = await q1(`SELECT id FROM acc_entries WHERE status <> 'draft' AND number IS NULL`);
  const voids = await q1(`SELECT e.id FROM acc_entries e WHERE e.status = 'void' AND (e.reversed_by IS NULL OR NOT EXISTS (SELECT 1 FROM acc_entries r WHERE r.id = e.reversed_by AND r.reverses = e.id))`);
  const tb = (await admin.get('/accounting/reports/trial-balance')).data;
  const bs = (await admin.get('/accounting/reports/balance-sheet')).data;
  ok(`${when}: every entry balances`, unbalanced.length === 0, unbalanced);
  ok(`${when}: lines are a debit or a credit, dated as their entry`, both.length === 0 && dates.length === 0);
  ok(`${when}: posted entries are numbered; voids point at their reversal`, nonumber.length === 0 && voids.length === 0, { nonumber, voids });
  ok(`${when}: the trial balance balances`, tb.balanced === true && tb.total_debit === tb.total_credit, [tb.total_debit, tb.total_credit]);
  ok(`${when}: the balance sheet balances`, bs.balanced === true, [bs.assets?.total, bs.liabilities_and_equity]);
};

const settingsBefore = (await admin.get('/accounting/settings')).data;
let accounts: Record<string, any> = {};
const acct = (code: string) => accounts[code];
const refreshAccounts = async () => { accounts = Object.fromEntries((await admin.get('/accounting/accounts')).data.items.map((a: any) => [a.code, a])); };

try {
  // ---- access ----
  const nobody = await user('Nora', []);
  const viewer = await user('Vera', ['accounting.view']);
  const clerk = await user('Clay', ['accounting.post']);
  ok('no rights: nothing', (await nobody.get('/accounting/settings')).status === 403 && (await nobody.get('/accounting/reports/trial-balance')).status === 403 && (await nobody.get('/accounting/invoices')).status === 403);

  // ---- setup ----
  if (!settingsBefore.settings.ready) {
    ok('before setup: nothing can be posted', (await admin.post('/accounting/entries', { date: day(), lines: [], post: true })).status === 400);
    ok('only administrators set it up', (await clerk.post('/accounting/setup', {})).status === 403);
    const r = await admin.post('/accounting/setup', { fiscal_year_start: 1 });
    ok('set up with the starter chart', r.status === 200 && r.data.settings.ready && r.data.settings.receivable_account, r.data);
  }
  ok('set up once', (await admin.post('/accounting/setup', {})).status === 409);
  await refreshAccounts();
  ok('the starter chart', acct('1100')?.subtype === 'receivable' && acct('2000')?.subtype === 'payable' && acct('3100')?.subtype === 'retained_earnings' && acct('4000')?.type === 'income');
  const S = (await admin.get('/accounting/settings')).data;
  const base = S.base_currency;

  // ---- accounts ----
  const mk = async (code: string, name: string, subtype: string, extra: object = {}) => (await admin.post('/accounting/accounts', { code: `T${stamp}-${code}`, name: `${name} ${stamp}`, subtype, ...extra })).data.id;
  const bank = await mk('1010', 'Test bank', 'bank');
  const income = await mk('4000', 'Test sales', 'income');
  const expense = await mk('6000', 'Test costs', 'expense');
  const taxPay = await mk('2100', 'Test tax payable', 'tax_payable');
  const taxRec = await mk('1300', 'Test tax receivable', 'tax_receivable');
  ok('accounts made', bank && income && expense && taxPay && taxRec);
  ok('codes are unique', (await admin.post('/accounting/accounts', { code: `T${stamp}-1010`, name: 'x', subtype: 'bank' })).status === 409);
  ok('kinds are checked', (await admin.post('/accounting/accounts', { code: `T${stamp}-X`, name: 'x', subtype: 'magic' })).status === 400);
  ok('only bank and cash in another currency', (await admin.post('/accounting/accounts', { code: `T${stamp}-Y`, name: 'x', subtype: 'expense', currency: 'EUR' })).status === 400);
  ok('clerks don\'t change the chart', (await clerk.post('/accounting/accounts', { code: `T${stamp}-Z`, name: 'x', subtype: 'expense' })).status === 403);
  ok('viewers read it', (await viewer.get('/accounting/accounts')).status === 200 && (await viewer.post('/accounting/entries', { date: day(), lines: [], post: true })).status === 403);
  const spare = await mk('6999', 'Spare', 'expense');
  ok('an unused account can be deleted', (await admin.del(`/accounting/accounts/${spare}`)).status === 200);
  ok('a default account can\'t be deactivated', (await admin.patch(`/accounting/accounts/${acct('1100').id}`, { is_active: false })).status === 400);

  // ---- taxes ----
  ok('a sales tax goes to a tax payable account', (await admin.post('/accounting/taxes', { name: `Bad ${stamp}`, rate: '12.5', scope: 'sales', account_id: expense })).status === 400);
  ok('rates are percentages', (await admin.post('/accounting/taxes', { name: `Bad ${stamp}`, rate: '120', scope: 'sales', account_id: taxPay })).status === 400);
  const salesTax = (await admin.post('/accounting/taxes', { name: `Sales tax ${stamp}`, rate: '12.5', scope: 'sales', account_id: taxPay })).data.id;
  const buyTax = (await admin.post('/accounting/taxes', { name: `Purchase tax ${stamp}`, rate: '12.5', scope: 'purchases', account_id: taxRec })).data.id;
  ok('taxes made', salesTax && buyTax);

  // ---- journal entries ----
  const L = (account: string, debit: string | number, credit: string | number, more: object = {}) => ({ account_id: account, debit: String(debit), credit: String(credit), ...more });
  ok('unbalanced: refused', (await clerk.post('/accounting/entries', { date: day(), lines: [L(bank, 100, 0), L(income, 0, 90)], post: true })).status === 400);
  ok('one line: refused', (await clerk.post('/accounting/entries', { date: day(), lines: [L(bank, 0, 0)], post: true })).status === 400);
  ok('negative: refused', (await clerk.post('/accounting/entries', { date: day(), lines: [L(bank, -5, 0), L(income, -5, 0)], post: true })).status === 400);
  ok('three decimals: refused', (await clerk.post('/accounting/entries', { date: day(), lines: [L(bank, '1.005', 0), L(income, 0, '1.005')], post: true })).status === 400);
  ok('both sides on a line: refused', (await clerk.post('/accounting/entries', { date: day(), lines: [L(bank, 5, 5), L(income, 0, 0)], post: true })).status === 400);
  let r = await clerk.post('/accounting/entries', { date: day(-2), memo: `Draft ${stamp}`, lines: [L(bank, 500, 0), L(income, 0, 400)] });
  const draft = r.data;
  ok('a draft may be unbalanced', r.status === 200 && draft.status === 'draft' && draft.number === null, r.data);
  ok('and isn\'t posted so', (await clerk.post(`/accounting/entries/${draft.id}/post`)).status === 400);
  r = await clerk.put(`/accounting/entries/${draft.id}`, { date: day(-2), memo: `Capital in ${stamp}`, lines: [L(bank, 500, 0), L(income, 0, 500)], post: true });
  const posted = r.data;
  ok('fixed and posted: numbered', r.status === 200 && posted.status === 'posted' && posted.number > 0, r.data);
  ok('posted entries don\'t change', (await clerk.put(`/accounting/entries/${posted.id}`, { date: day(), lines: [L(bank, 1, 0), L(income, 0, 1)] })).status === 400 && (await clerk.del(`/accounting/entries/${posted.id}`)).status === 400);
  r = await clerk.post(`/accounting/entries/${posted.id}/reverse`, {});
  ok('reversed: void, with a reversing entry', r.data.status === 'void' && r.data.reversed_by_number > posted.number, r.data);
  const rev = (await admin.get(`/accounting/entries/${r.data.reversed_by}`)).data;
  ok('the reversal swaps every line', rev.lines.length === 2 && rev.lines.some((l: any) => l.account_id === bank && n(l.credit) === 500) && rev.lines.some((l: any) => l.account_id === income && n(l.debit) === 500), rev.lines);
  ok('only once', (await clerk.post(`/accounting/entries/${posted.id}/reverse`, {})).status === 400);
  const twoNumbers = await Promise.all([1, 2, 3].map(() => clerk.post('/accounting/entries', { date: day(), memo: `Parallel ${stamp}`, lines: [L(bank, 1, 0), L(income, 0, 1)], post: true })));
  const nums = twoNumbers.map((x) => x.data.number).sort((a, b) => a - b);
  ok('numbers without gaps, even posted at once', nums[1] === nums[0] + 1 && nums[2] === nums[1] + 1, nums);

  // ---- the lock date ----
  const old = (await clerk.post('/accounting/entries', { date: day(-3), memo: `Old ${stamp}`, lines: [L(bank, 10, 0), L(income, 0, 10)], post: true })).data;
  ok('clerks don\'t move the lock date', (await clerk.put('/accounting/settings', { lock_date: day(-1) })).status === 403);
  await admin.put('/accounting/settings', { lock_date: day(-1) });
  ok('locked: nothing posted on or before it', (await clerk.post('/accounting/entries', { date: day(-1), lines: [L(bank, 1, 0), L(income, 0, 1)], post: true })).status === 400);
  ok('a locked entry is reversed into an open day', (await clerk.post(`/accounting/entries/${old.id}/reverse`, { date: day() })).status === 200);
  await admin.put('/accounting/settings', { lock_date: settingsBefore.settings.lock_date });
  await invariants('after journal entries');

  // ---- invoices from the CRM ----
  const company = (await admin.post('/crm/company/items', { company_name: `Buyer ${stamp}` })).data;
  const invoice = async (rows: object[], extra: object = {}) => {
    const inv = (await admin.post('/crm/invoice/items', { title: `Invoice ${stamp}`, company_id: company.id, currency: base, ...extra })).data;
    if (rows.length) await admin.put(`/crm/items/${inv.id}/products`, { rows });
    return inv;
  };
  const inv1 = await invoice([
    { name: 'Licence', price: '100', quantity: '3', tax_rate: '12.5' },       // 300 + 37.50
    { name: 'Setup', price: '99.99', quantity: '1', discount_kind: 'percent', discount_value: '10', tax_rate: '12.5' }, // 89.99 + 11.25 (11.24875)
    { name: 'Book', price: '20', quantity: '2' },                            // 40, no tax
  ], { close_date: day(-45) });
  ok('posting needs the right to post', (await viewer.post(`/accounting/invoices/${inv1.id}/post`, {})).status === 403);
  r = await clerk.post(`/accounting/invoices/${inv1.id}/post`, { date: day(-40) });
  ok('an invoice posted', r.status === 200 && r.data.status === 'open', r.data);
  const crmTotal = n((await admin.get(`/crm/invoice/items/${inv1.id}`)).data.opportunity);
  ok('its total is the CRM\'s', n(r.data.total) === crmTotal && crmTotal === 478.74, [r.data.total, crmTotal]);
  const e1 = (await admin.get(`/accounting/entries/${r.data.entry_id}`)).data;
  const sum = (lines: any[], f: (l: any) => boolean, k: 'debit' | 'credit') => lines.filter(f).reduce((s, l) => s + n(l[k]), 0);
  ok('receivables debited with the total, for the customer', sum(e1.lines, (l) => l.account_id === acct('1100').id && l.party_id === company.id, 'debit') === 478.74);
  ok('sales credited with the net', Math.abs(sum(e1.lines, (l) => l.account_id === acct('4000').id, 'credit') - 429.99) < 0.001);
  ok('tax credited, to the tax\'s account', Math.abs(sum(e1.lines, (l) => l.account_id === taxPay, 'credit') - 48.75) < 0.001, e1.lines);
  ok('posting twice: refused', (await clerk.post(`/accounting/invoices/${inv1.id}/post`, {})).status === 409);

  // The CRM lock.
  ok('locked: products', (await admin.put(`/crm/items/${inv1.id}/products`, { rows: [{ name: 'x', price: '1', quantity: '1' }] })).status === 400);
  ok('locked: amount', (await admin.patch(`/crm/invoice/items/${inv1.id}`, { opportunity: '1', is_manual_amount: true })).status === 400);
  ok('locked: client', (await admin.patch(`/crm/invoice/items/${inv1.id}`, { company_id: null })).status === 400);
  ok('locked: deleting', (await admin.del(`/crm/invoice/items/${inv1.id}`)).status === 400);
  ok('other fields still change', (await admin.patch(`/crm/invoice/items/${inv1.id}`, { title: `Invoice A ${stamp}` })).status === 200);
  ok('saving the form unchanged is fine', (await admin.patch(`/crm/invoice/items/${inv1.id}`, { opportunity: String(crmTotal), currency: base, company_id: company.id })).status === 200);

  const inv7 = await invoice([{ name: 'Odd', price: '10', quantity: '1', tax_rate: '7' }]);
  r = await clerk.post(`/accounting/invoices/${inv7.id}/post`, {});
  ok('a rate with no sales tax: explained', r.status === 400 && /no sales tax at 7%/.test(r.data.error?.message), r.data);
  const invManual = await invoice([{ name: 'A', price: '10', quantity: '1' }]);
  await admin.patch(`/crm/invoice/items/${invManual.id}`, { opportunity: '15', is_manual_amount: true });
  r = await clerk.post(`/accounting/invoices/${invManual.id}/post`, {});
  ok('an amount that differs from the products: refused', r.status === 400 && /differs from its products/.test(r.data.error?.message), r.data);
  const noClient = (await admin.post('/crm/invoice/items', { title: `Nobody ${stamp}`, currency: base })).data;
  await admin.put(`/crm/items/${noClient.id}/products`, { rows: [{ name: 'A', price: '10', quantity: '1' }] });
  ok('no client: refused', (await clerk.post(`/accounting/invoices/${noClient.id}/post`, {})).status === 400);
  const empty = await invoice([]);
  ok('nothing to invoice: refused', (await clerk.post(`/accounting/invoices/${empty.id}/post`, {})).status === 400);

  // Voiding and posting again.
  const inv2 = await invoice([{ name: 'Support', price: '200', quantity: '1', tax_rate: '12.5' }]);
  await clerk.post(`/accounting/invoices/${inv2.id}/post`, {});
  r = await clerk.post(`/accounting/invoices/${inv2.id}/void`, {});
  ok('voided: unlocked in the CRM', r.data.status === 'void' && (await admin.put(`/crm/items/${inv2.id}/products`, { rows: [{ name: 'Support', price: '240', quantity: '1', tax_rate: '12.5' }] })).status === 200);
  r = await clerk.post(`/accounting/invoices/${inv2.id}/post`, {});
  ok('and posted again with the new amount', r.data.status === 'open' && n(r.data.total) === 270, r.data);
  await invariants('after invoices');

  // ---- payments ----
  const pay = (body: object) => clerk.post('/accounting/payments', { direction: 'in', date: day(), account_id: bank, ...body });
  r = await pay({ amount: '100.00', allocations: [{ doc_kind: 'invoice', doc_id: inv1.id, amount: '100.00' }] });
  ok('a part payment', r.status === 200 && r.data.party_id === company.id && n(r.data.unallocated) === 0, r.data);
  let i1 = (await admin.get(`/accounting/invoices/${inv1.id}`)).data;
  ok('the invoice is still open', i1.status === 'open' && n(i1.balance) === 378.74, i1);
  ok('more than is owed: refused', (await pay({ amount: '400', allocations: [{ doc_kind: 'invoice', doc_id: inv1.id, amount: '400' }] })).status === 400);
  ok('allocations over the payment: refused', (await pay({ amount: '10', allocations: [{ doc_kind: 'invoice', doc_id: inv1.id, amount: '20' }] })).status === 400);
  ok('a bill can\'t take money in', (await pay({ amount: '10', allocations: [{ doc_kind: 'bill', doc_id: inv1.id, amount: '10' }] })).status === 400);
  const other = (await admin.post('/crm/company/items', { company_name: `Someone else ${stamp}` })).data;
  ok('someone else\'s invoice: refused', (await pay({ party_id: other.id, amount: '10', allocations: [{ doc_kind: 'invoice', doc_id: inv1.id, amount: '10' }] })).status === 400);
  ok('only through bank or cash', (await pay({ account_id: income, amount: '10', party_id: company.id })).status === 400);
  // Two payments of the whole balance at once: only one gets through.
  const both = await Promise.all([1, 2].map(() => pay({ amount: '378.74', allocations: [{ doc_kind: 'invoice', doc_id: inv1.id, amount: '378.74' }] })));
  ok('two full payments at once: one gets through', both.map((x) => x.status).sort().join() === '200,400', both.map((x) => [x.status, x.data?.error]));
  i1 = (await admin.get(`/accounting/invoices/${inv1.id}`)).data;
  ok('paid', i1.status === 'paid' && n(i1.balance) === 0, i1);
  const crmInv = (await admin.get(`/crm/invoice/items/${inv1.id}`)).data;
  ok('and "Paid" in the CRM', crmInv.stage_semantics === 'success', crmInv.stage_name);
  const okPay = both.find((x) => x.status === 200)!.data;
  r = await clerk.post(`/accounting/payments/${okPay.id}/void`, {});
  i1 = (await admin.get(`/accounting/invoices/${inv1.id}`)).data;
  ok('a voided payment reopens the invoice', r.data.status === 'void' && i1.status === 'open' && n(i1.balance) === 378.74, i1);
  ok('an invoice with payments can\'t be voided', (await clerk.post(`/accounting/invoices/${inv1.id}/void`, {})).status === 400);
  await pay({ amount: '378.74', allocations: [{ doc_kind: 'invoice', doc_id: inv1.id, amount: '378.74' }] });

  // An advance, allocated later.
  r = await pay({ party_id: company.id, amount: '300.00' });
  const advance = r.data;
  ok('an advance: not allocated', n(advance.unallocated) === 300);
  r = await clerk.post(`/accounting/payments/${advance.id}/allocate`, { allocations: [{ doc_kind: 'invoice', doc_id: inv2.id, amount: '270.00' }] });
  ok('allocated later: the invoice is paid', n(r.data.unallocated) === 30 && (await admin.get(`/accounting/invoices/${inv2.id}`)).data.status === 'paid', r.data);
  ok('not more than is left on it', (await clerk.post(`/accounting/payments/${advance.id}/allocate`, { allocations: [{ doc_kind: 'invoice', doc_id: inv1.id, amount: '31' }] })).status === 400);

  // Receivables equal what the customer owes (base currency): open invoices minus the advance left.
  const arParty = async (party: string) => n((await q1(`SELECT COALESCE(SUM(l.debit - l.credit), 0) AS b FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id
                                                     WHERE e.status <> 'draft' AND l.party_id = $1 AND l.account_id = $2`, [party, acct('1100').id]))[0].b);
  ok('receivables = what the customer owes', await arParty(company.id) === -30, await arParty(company.id));
  await invariants('after payments');

  // ---- foreign currency ----
  const cur = (await admin.get('/crm/currencies')).data;
  if (!(cur.items ?? cur).some((c: any) => c.code === 'EUR')) await admin.post('/crm/currencies', { code: 'EUR', rate: '1.10' });
  const eurBank = await mk('1020', 'Test EUR bank', 'bank', { currency: 'EUR' });
  const euro = await invoice([{ name: 'Export', price: '100', quantity: '1' }], { currency: 'EUR' });
  r = await clerk.post(`/accounting/invoices/${euro.id}/post`, { fx_rate: '1.2345' });
  const ee = (await admin.get(`/accounting/entries/${r.data.entry_id}`)).data;
  const arLine = ee.lines.find((l: any) => l.account_id === acct('1100').id);
  ok('a euro invoice: booked in the base currency at its rate, euros kept', n(arLine.debit) === 123.45 && arLine.currency === 'EUR' && n(arLine.amount_currency) === 100, arLine);
  ok('paying it from a base-currency account: refused', (await pay({ amount: '100', allocations: [{ doc_kind: 'invoice', doc_id: euro.id, amount: '100' }] })).status === 400);
  r = await pay({ account_id: eurBank, amount: '100.00', fx_rate: '1.30', allocations: [{ doc_kind: 'invoice', doc_id: euro.id, amount: '100.00' }] });
  const pe = (await admin.get(`/accounting/entries/${r.data.entry_id}`)).data;
  ok('paid at a better rate: bank 130.00, receivable cleared at 123.45, gain 6.55',
    pe.lines.some((l: any) => l.account_id === eurBank && n(l.debit) === 130) && pe.lines.some((l: any) => l.account_id === acct('1100').id && n(l.credit) === 123.45)
    && pe.lines.some((l: any) => l.account_id === acct('7900').id && n(l.credit) === 6.55), pe.lines);
  ok('the euro bank shows euros', n((await admin.get(`/accounting/bank/${eurBank}`)).data.book_balance) === 100);
  await invariants('after foreign currency');

  // ---- bills ----
  const supplier = (await admin.post('/crm/company/items', { company_name: `Supplier ${stamp}` })).data;
  const bill = { party_id: supplier.id, reference: `INV-${stamp}`, date: day(-10), due_date: day(-5), lines: [
    { description: 'Paper', account_id: expense, quantity: '10', unit_price: '4.50', tax_id: buyTax },  // 45 + 5.63
    { description: 'Postage', account_id: expense, quantity: '1', unit_price: '7.00' },                  // 7
  ] };
  ok('bills go to expense accounts', (await clerk.post('/accounting/bills', { ...bill, lines: [{ description: 'x', account_id: bank, unit_price: '1' }] })).status === 400);
  ok('with purchase taxes', (await clerk.post('/accounting/bills', { ...bill, lines: [{ description: 'x', account_id: expense, unit_price: '1', tax_id: salesTax }] })).status === 400);
  r = await clerk.post('/accounting/bills', bill);
  const b1 = r.data;
  ok('a draft bill', r.status === 200 && b1.status === 'draft' && n(b1.subtotal) === 52 && n(b1.tax) === 5.63 && n(b1.total) === 57.63, r.data);
  r = await clerk.put(`/accounting/bills/${b1.id}`, { ...bill, tax_inclusive: true, lines: [{ description: 'Paper', account_id: expense, quantity: '1', unit_price: '56.25', tax_id: buyTax }] });
  ok('prices with tax included', n(r.data.subtotal) === 50 && n(r.data.tax) === 6.25 && n(r.data.total) === 56.25, r.data);
  r = await clerk.post(`/accounting/bills/${b1.id}/post`, {});
  ok('posted: numbered', r.data.status === 'open' && r.data.number > 0, r.data);
  const be = (await admin.get(`/accounting/entries/${r.data.entry_id}`)).data;
  ok('expense and reclaimable tax debited, payables credited', be.lines.some((l: any) => l.account_id === expense && n(l.debit) === 50) && be.lines.some((l: any) => l.account_id === taxRec && n(l.debit) === 6.25) && be.lines.some((l: any) => l.account_id === acct('2000').id && n(l.credit) === 56.25 && l.party_id === supplier.id), be.lines);
  ok('posted bills don\'t change', (await clerk.put(`/accounting/bills/${b1.id}`, bill)).status === 400 && (await clerk.del(`/accounting/bills/${b1.id}`)).status === 400);
  r = await clerk.post('/accounting/payments', { direction: 'out', date: day(), account_id: bank, amount: '56.25', allocations: [{ doc_kind: 'bill', doc_id: b1.id, amount: '56.25' }] });
  ok('paid', r.status === 200 && (await admin.get(`/accounting/bills/${b1.id}`)).data.status === 'paid', r.data);
  const b2 = (await clerk.post('/accounting/bills', { ...bill, reference: `INV2-${stamp}` })).data;
  await clerk.post(`/accounting/bills/${b2.id}/post`, {});
  ok('a bill voided', (await clerk.post(`/accounting/bills/${b2.id}/void`, {})).data.status === 'void');
  await invariants('after bills');

  // ---- bank statements ----
  const inv3 = await invoice([{ name: 'Service', price: '80', quantity: '1' }]);
  await clerk.post(`/accounting/invoices/${inv3.id}/post`, {});
  const deposit = (await pay({ amount: '250.00', party_id: company.id, reference: `DEP-${stamp}` })).data;
  const csv = `Date,Description,Reference,Amount\n${day()},Customer deposit ${stamp},DEP-${stamp},250.00\n${day()},Bank fee ${stamp},,-12.50\n${day()},Cash sale ${stamp},,112.50\n${day()},Buyer pays invoice ${stamp},,80.00\n${day()},Mystery ${stamp},,1.00\n`;
  const fd = new FormData(); fd.append('file', new Blob([csv], { type: 'text/csv' }), `statement-${stamp}.csv`);
  const file = (await clerk.req('POST', '/files', fd)).data;
  r = await clerk.post(`/accounting/bank/${bank}/import`, { file_id: file.id });
  ok('a statement imported', r.status === 200 && r.data.added === 5 && r.data.skipped === 0, r.data);
  r = await clerk.post(`/accounting/bank/${bank}/import`, { file_id: file.id });
  ok('importing it again adds nothing', r.data.added === 0 && r.data.skipped === 5, r.data);
  ok('only into bank accounts', (await clerk.post(`/accounting/bank/${expense}/import`, { file_id: file.id })).status === 400);
  let lines = (await clerk.get(`/accounting/bank/${bank}/lines`)).data.items as any[];
  const line = (text: string) => lines.find((l) => l.description.startsWith(text));
  const dep = line('Customer deposit');
  ok('the deposit is suggested for its payment', dep.suggestions.some((s: any) => n(s.amount) === 250), dep.suggestions);
  const depLine = dep.suggestions.find((s: any) => n(s.amount) === 250).line_id;
  ok('matching a different amount: refused', (await clerk.post(`/accounting/bank/lines/${line('Bank fee').id}/match`, { line_id: depLine })).status === 400);
  ok('matched', (await clerk.post(`/accounting/bank/lines/${dep.id}/match`, { line_id: depLine })).status === 200);
  ok('a journal line matches once', (await clerk.post(`/accounting/bank/lines/${line('Mystery').id}/match`, { line_id: depLine })).status === 400);
  r = await clerk.post(`/accounting/bank/lines/${line('Bank fee').id}/record`, { account_id: expense });
  ok('a bank fee recorded as an expense', r.data.lines?.some((l: any) => l.account_id === expense && n(l.debit) === 12.5) && r.data.lines.some((l: any) => l.account_id === bank && n(l.credit) === 12.5), r.data);
  r = await clerk.post(`/accounting/bank/lines/${line('Cash sale').id}/record`, { account_id: income, tax_id: salesTax });
  ok('a cash sale with tax included: split', r.data.lines?.some((l: any) => l.account_id === income && n(l.credit) === 100) && r.data.lines.some((l: any) => l.account_id === taxPay && n(l.credit) === 12.5), r.data);
  ok('customers\' money is a payment, not an entry', (await clerk.post(`/accounting/bank/lines/${line('Buyer pays').id}/record`, { account_id: acct('1100').id })).status === 400);
  r = await clerk.post(`/accounting/bank/lines/${line('Buyer pays').id}/payment`, { allocations: [{ doc_kind: 'invoice', doc_id: inv3.id, amount: '80.00' }] });
  ok('recorded as the customer\'s payment of an invoice', r.status === 200 && (await admin.get(`/accounting/invoices/${inv3.id}`)).data.status === 'paid', r.data);
  ok('ignored', (await clerk.post(`/accounting/bank/lines/${line('Mystery').id}/ignore`)).status === 200);
  const sumry = (await admin.get(`/accounting/bank/${bank}`)).data;
  ok('everything on the statement accounted for', sumry.unmatched === 0 && sumry.statement_lines === 5, sumry);
  ok('a recorded line already used: refused', (await clerk.post(`/accounting/bank/lines/${line('Bank fee').id}/record`, { account_id: expense })).status === 400);
  await invariants('after bank reconciliation');

  // ---- reports ----
  const pl = (await viewer.get(`/accounting/reports/profit-loss?from=${day(-60)}&to=${day()}`)).data;
  const testSales = pl.sections.find((s: any) => s.key === 'income').items.find((i: any) => i.account_id === income);
  // Posted to it: 500 and 10 (both reversed), three entries of 1 (posted at once), the cash sale's 100.
  ok('profit and loss: the test sales account', n(testSales?.amount) === 103, testSales);
  ok('net profit = income - costs', Math.abs(n(pl.net_profit) - (n(pl.sections[0].total) - n(pl.sections[1].total) - n(pl.sections[2].total) + n(pl.sections[3].total) - n(pl.sections[4].total))) < 0.001);
  const led = (await viewer.get(`/accounting/reports/ledger?account_id=${bank}`)).data;
  let running = 0, okRun = true;
  for (const i of led.items) { running = Math.round((running + n(i.debit) - n(i.credit)) * 100) / 100; if (running !== n(i.balance)) okRun = false; }
  ok('the ledger\'s running balance', okRun && n(led.closing) === running, led.closing);
  const late = await invoice([{ name: 'Late', price: '64', quantity: '1' }], { close_date: day(-45) });
  await clerk.post(`/accounting/invoices/${late.id}/post`, { date: day(-50) });
  const ag = (await viewer.get('/accounting/reports/aging?kind=receivable')).data;
  const mine = ag.items.find((i: any) => i.party_id === company.id);
  ok('ageing: an invoice 45 days overdue in 31-60', mine?.documents.some((d: any) => d.bucket === '31-60' && n(d.balance) === 64), mine);
  ok('ageing: the advance left on the account as a credit', mine?.documents.some((d: any) => /not allocated/.test(d.label) && n(d.balance) === -30), mine?.documents);
  ok('ageing adds up to the receivables account', n(mine?.total) === await arParty(company.id), [mine?.total, await arParty(company.id)]);
  const ap = (await viewer.get('/accounting/reports/aging?kind=payable')).data;
  ok('payables ageing', Array.isArray(ap.items) && ap.kind === 'payable');
  const tax = (await viewer.get(`/accounting/reports/tax?from=${day(-60)}&to=${day()}`)).data;
  const st = tax.items.find((t: any) => t.tax_id === salesTax), pt = tax.items.find((t: any) => t.tax_id === buyTax);
  ok('tax report: on sales', st && n(st.tax) === 48.75 + 30 + 12.5 && n(st.base) === 389.99 + 240 + 100, st);
  ok('tax report: on purchases', pt && n(pt.tax) === 6.25 && n(pt.base) === 50, pt);
  const stmt = (await viewer.get(`/accounting/reports/statement?party_id=${company.id}`)).data;
  ok('the customer\'s statement ends at what they owe', n(stmt.closing) === await arParty(company.id), [stmt.closing, await arParty(company.id)]);
  const raw = await viewer.req('GET', '/accounting/reports/trial-balance?format=csv', undefined, true);
  const text = await raw.text();
  ok('reports download as CSV', raw.headers.get('content-type')?.includes('text/csv') && text.startsWith('Code,Account') && /Total/.test(text));
  const summary = (await viewer.get('/accounting/reports/summary')).data;
  ok('the overview', summary.ready && summary.money.some((m: any) => m.account_id === bank) && typeof summary.receivable === 'string');

  // ---- year-end closing (far in the past, so it touches only this test's entries) ----
  await admin.put('/accounting/settings', { fiscal_year_start: 1 });
  await clerk.post('/accounting/entries', { date: '2001-06-01', memo: `Old sale ${stamp}`, lines: [L(bank, 1000, 0), L(income, 0, 1000)], post: true });
  await clerk.post('/accounting/entries', { date: '2001-07-01', memo: `Old cost ${stamp}`, lines: [L(expense, 300, 0), L(bank, 0, 300)], post: true });
  ok('only on a year\'s last day', (await admin.post('/accounting/close', { year_end: '2001-06-30' })).status === 400);
  ok('only administrators close', (await clerk.post('/accounting/close', { year_end: '2001-12-31' })).status === 403);
  r = await admin.post('/accounting/close', { year_end: '2001-12-31' });
  ok('the year closed', r.status === 200 && r.data.entry_id && r.data.lock_date === '2001-12-31', r.data);
  const closing = (await admin.get(`/accounting/entries/${r.data.entry_id}`)).data;
  ok('income and expenses to retained earnings', closing.lines.some((l: any) => l.account_id === income && n(l.debit) === 1000) && closing.lines.some((l: any) => l.account_id === expense && n(l.credit) === 300) && closing.lines.some((l: any) => l.account_id === acct('3100').id && n(l.credit) === 700), closing.lines);
  const pl01 = (await admin.get('/accounting/reports/profit-loss?from=2001-01-01&to=2001-12-31')).data;
  ok('that year\'s profit still shows (closing left out)', n(pl01.net_profit) === 700, pl01.net_profit);
  const bs01 = (await admin.get('/accounting/reports/balance-sheet?as_of=2001-12-31')).data;
  ok('its balance sheet: profit in retained earnings', bs01.balanced && !bs01.equity.items.some((i: any) => i.name === 'Profit not yet closed') && bs01.equity.items.some((i: any) => i.account_id === acct('3100').id && n(i.amount) === 700), bs01.equity);
  ok('closed once', (await admin.post('/accounting/close', { year_end: '2001-12-31' })).status === 409);
  ok('locked', (await clerk.post('/accounting/entries', { date: '2001-12-01', lines: [L(bank, 1, 0), L(income, 0, 1)], post: true })).status === 400);
  ok('reopening needs the lock moved first', (await admin.post(`/accounting/entries/${r.data.entry_id}/reverse`, {})).status === 400);
  await admin.put('/accounting/settings', { lock_date: null });
  ok('reopened', (await admin.post(`/accounting/entries/${r.data.entry_id}/reverse`, {})).status === 200);
  const bs01b = (await admin.get('/accounting/reports/balance-sheet?as_of=2001-12-31')).data;
  ok('the profit is open again', bs01b.balanced && bs01b.equity.items.some((i: any) => i.name === 'Profit not yet closed' && n(i.amount) === 700), bs01b.equity);
  await invariants('at the end');

  // ---- the CRM side ----
  ok('a customer in the books stays in the recycle bin', (await admin.del(`/crm/company/items/${supplier.id}`)).status === 200 && (await admin.del(`/crm/recycle-bin/${supplier.id}`)).status === 400);
  ok('the CRM base currency stays while there are entries', (await admin.patch('/crm/currencies/EUR', { is_base: true })).status === 400);
} finally {
  await admin.put('/accounting/settings', { lock_date: settingsBefore.settings.lock_date, fiscal_year_start: settingsBefore.settings.fiscal_year_start }).catch(() => {});
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
