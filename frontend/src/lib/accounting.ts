// Accounting: shared types and helpers for the books' pages.
import { api } from '#lib/api.ts';
import { money } from '#lib/crm.svelte.ts';

export type Account = {
	id: string;
	code: string;
	name: string;
	type: 'asset' | 'liability' | 'equity' | 'income' | 'expense';
	subtype: string;
	parent_id: string | null;
	currency: string | null;
	description: string | null;
	is_active: boolean;
	balance: string;
	used: boolean;
};
export type Tax = { id: string; name: string; rate: string; scope: 'sales' | 'purchases'; account_id: string; is_active: boolean };
export type Party = { id: string; name: string; kind: 'company' | 'contact'; company_kind?: string | null };
export type Settings = {
	ready: boolean;
	fiscal_year_start: number;
	lock_date: string | null;
	receivable_account: string | null;
	payable_account: string | null;
	sales_account: string | null;
	retained_earnings: string | null;
	opening_balances: string | null;
	fx_account: string | null;
	rounding_account: string | null;
	mark_invoices_paid: boolean;
};

export const SUBTYPES: [string, string, string][] = [
	['bank', 'asset', 'Bank'],
	['cash', 'asset', 'Cash'],
	['receivable', 'asset', 'Accounts receivable'],
	['current_asset', 'asset', 'Current asset'],
	['inventory', 'asset', 'Inventory'],
	['fixed_asset', 'asset', 'Fixed asset'],
	['tax_receivable', 'asset', 'Tax receivable'],
	['payable', 'liability', 'Accounts payable'],
	['current_liability', 'liability', 'Current liability'],
	['tax_payable', 'liability', 'Tax payable'],
	['long_term_liability', 'liability', 'Long-term liability'],
	['equity', 'equity', 'Equity'],
	['retained_earnings', 'equity', 'Retained earnings'],
	['income', 'income', 'Income'],
	['other_income', 'income', 'Other income'],
	['cost_of_sales', 'expense', 'Cost of sales'],
	['expense', 'expense', 'Expense'],
	['other_expense', 'expense', 'Other expense']
];
export const subtypeLabel = (s: string) => SUBTYPES.find((x) => x[0] === s)?.[2] ?? s;
export const TYPES: [Account['type'], string][] = [
	['asset', 'Assets'],
	['liability', 'Liabilities'],
	['equity', 'Equity'],
	['income', 'Income'],
	['expense', 'Expenses']
];

let accountsCache: Promise<Account[]> | null = null;
/** The chart of accounts (cached for the page; `fresh` reloads). */
export function accounts(fresh = false): Promise<Account[]> {
	if (fresh || !accountsCache) accountsCache = api.get<{ items: Account[] }>('/accounting/accounts').then((r) => r.items);
	return accountsCache;
}
let taxesCache: Promise<Tax[]> | null = null;
export function taxes(fresh = false): Promise<Tax[]> {
	if (fresh || !taxesCache) taxesCache = api.get<{ items: Tax[] }>('/accounting/taxes').then((r) => r.items);
	return taxesCache;
}

let baseCache: Promise<string> | null = null;
export function baseCurrency(): Promise<string> {
	baseCache ??= api.get<{ base_currency: string }>('/accounting/settings').then((r) => r.base_currency);
	return baseCache;
}

/** An amount in the base currency (or another one). */
export const amount = (v: string | number | null | undefined, currency: string) => money(v ?? 0, currency);

/** Debit-positive balance shown the way accountants read it for the account's type. */
export function natural(balance: string | number, type: Account['type']): number {
	const n = Number(balance);
	return type === 'asset' || type === 'expense' ? n : -n;
}

export const today = () => new Date().toISOString().slice(0, 10);

export const STATUS_BADGE: Record<string, string> = {
	unposted: 'bg-slate-100 text-slate-600',
	draft: 'bg-slate-100 text-slate-600',
	open: 'bg-sky-50 text-sky-700',
	posted: 'bg-sky-50 text-sky-700',
	paid: 'bg-emerald-50 text-emerald-700',
	credited: 'bg-amber-50 text-amber-800',
	void: 'bg-red-50 text-red-700'
};
