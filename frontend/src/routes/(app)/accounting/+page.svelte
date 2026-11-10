<script lang="ts">
	// The books at a glance, or setting them up.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { amount } from '#lib/accounting.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Summary = {
		ready: boolean;
		base_currency: string;
		lock_date: string | null;
		money: { account_id: string; code: string; name: string; currency: string | null; balance: string; balance_currency: string | null }[];
		receivable: string;
		receivable_overdue: string;
		overdue_invoices: number;
		payable: string;
		payable_overdue: string;
		overdue_bills: number;
		year: { from: string; income: string; net_profit: string };
		month: { from: string; income: string; net_profit: string };
		unposted_invoices: number;
		unmatched_bank_lines: number;
		drafts: number;
	};
	let s = $state<Summary | null>(null);
	let fyStart = $state(1);
	let busy = $state(false);
	const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];

	const load = async () => (s = await api.get<Summary>('/accounting/reports/summary'));
	onMount(load);

	async function setup() {
		busy = true;
		try {
			await api.post('/accounting/setup', { fiscal_year_start: fyStart });
			toasts.success('The books are set up');
			await load();
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	const cur = $derived(s?.base_currency ?? '');
</script>

<svelte:head><title>Accounting · NERP</title></svelte:head>

<div class="space-y-5">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">Accounting</h1>
		<p class="text-sm text-slate-500">The company's books: double-entry, in {cur || 'the CRM\'s base currency'}.</p>
	</div>

	{#if s && !s.ready}
		<div class="card max-w-2xl space-y-4 p-6" data-setup>
			<h2 class="text-lg font-semibold text-slate-900">Set up the books</h2>
			<p class="text-sm text-slate-600">
				NERP keeps the books in the CRM's base currency and starts you with a simple chart of accounts (cash and bank, receivables, payables, tax,
				equity, sales and the usual expenses). You can change every account afterwards.
			</p>
			{#if session.can(PERM.ACCOUNTING_ADMIN)}
				<div class="max-w-xs">
					<label class="label" for="fy">The financial year starts in</label>
					<select id="fy" class="input" bind:value={fyStart}>{#each MONTHS as m, i (m)}<option value={i + 1}>{m}</option>{/each}</select>
				</div>
				<button class="btn btn-primary" disabled={busy} onclick={setup}>Set up accounting</button>
			{:else}
				<p class="text-sm text-slate-500">Ask an administrator of the books to set them up.</p>
			{/if}
		</div>
	{:else if s}
		<div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
			<a href="/accounting/banking" class="card p-5 hover:ring-1 hover:ring-brand-200">
				<p class="text-sm text-slate-500">Bank and cash</p>
				<p class="mt-1 text-2xl font-semibold text-slate-900" data-stat="money">{amount(s.money.reduce((t, m) => t + Number(m.balance), 0), cur)}</p>
				<ul class="mt-2 space-y-0.5 text-xs text-slate-500">
					{#each s.money as m (m.account_id)}<li class="flex justify-between gap-2"><span class="truncate">{m.name}</span><span>{m.currency ? amount(m.balance_currency, m.currency) : amount(m.balance, cur)}</span></li>{/each}
				</ul>
			</a>
			<a href="/accounting/invoices?status=open" class="card p-5 hover:ring-1 hover:ring-brand-200">
				<p class="text-sm text-slate-500">Customers owe</p>
				<p class="mt-1 text-2xl font-semibold text-slate-900" data-stat="receivable">{amount(s.receivable, cur)}</p>
				{#if s.overdue_invoices}<p class="mt-2 text-xs font-medium text-red-600">{amount(s.receivable_overdue, cur)} overdue on {s.overdue_invoices} invoice{s.overdue_invoices === 1 ? '' : 's'}</p>{/if}
			</a>
			<a href="/accounting/bills?status=open" class="card p-5 hover:ring-1 hover:ring-brand-200">
				<p class="text-sm text-slate-500">You owe suppliers</p>
				<p class="mt-1 text-2xl font-semibold text-slate-900" data-stat="payable">{amount(s.payable, cur)}</p>
				{#if s.overdue_bills}<p class="mt-2 text-xs font-medium text-red-600">{amount(s.payable_overdue, cur)} overdue on {s.overdue_bills} bill{s.overdue_bills === 1 ? '' : 's'}</p>{/if}
			</a>
			<a href="/accounting/reports" class="card p-5 hover:ring-1 hover:ring-brand-200">
				<p class="text-sm text-slate-500">Profit this year</p>
				<p class="mt-1 text-2xl font-semibold {Number(s.year.net_profit) < 0 ? 'text-red-600' : 'text-slate-900'}" data-stat="profit">{amount(s.year.net_profit, cur)}</p>
				<p class="mt-2 text-xs text-slate-500">Since {formatDate(s.year.from)} · this month {amount(s.month.net_profit, cur)}</p>
			</a>
		</div>

		<div class="card max-w-3xl">
			<h2 class="border-b border-slate-100 px-5 py-3 font-semibold text-slate-900">To do</h2>
			<ul class="divide-y divide-slate-100 text-sm">
				<li><a class="flex items-center gap-3 px-5 py-3 hover:bg-slate-50" href="/accounting/invoices?status=unposted"><Icon name="file" class="size-4 text-slate-400" /><span class="flex-1">CRM invoices not yet in the books</span><span class="badge {s.unposted_invoices ? 'bg-amber-50 text-amber-800' : 'bg-slate-100 text-slate-500'}">{s.unposted_invoices}</span></a></li>
				<li><a class="flex items-center gap-3 px-5 py-3 hover:bg-slate-50" href="/accounting/banking"><Icon name="building" class="size-4 text-slate-400" /><span class="flex-1">Bank statement lines to reconcile</span><span class="badge {s.unmatched_bank_lines ? 'bg-amber-50 text-amber-800' : 'bg-slate-100 text-slate-500'}">{s.unmatched_bank_lines}</span></a></li>
				<li><a class="flex items-center gap-3 px-5 py-3 hover:bg-slate-50" href="/accounting/journal?status=draft"><Icon name="edit" class="size-4 text-slate-400" /><span class="flex-1">Drafts (bills and journal entries)</span><span class="badge bg-slate-100 text-slate-500">{s.drafts}</span></a></li>
			</ul>
			{#if s.lock_date}<p class="border-t border-slate-100 px-5 py-3 text-xs text-slate-500"><Icon name="lock" class="mr-1 inline size-3.5" />The books are locked up to {formatDate(s.lock_date)}.</p>{/if}
		</div>
	{/if}
</div>
