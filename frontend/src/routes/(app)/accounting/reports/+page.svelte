<script lang="ts">
	// Reports: profit and loss, balance sheet, trial balance, ledgers, ageing, tax, statements.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, downloadUrl, query } from '#lib/api.ts';
	import { amount, baseCurrency, today, type Party } from '#lib/accounting.ts';
	import AccountSelect from '#lib/components/accounting/AccountSelect.svelte';
	import PartyPicker from '#lib/components/accounting/PartyPicker.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	const TABS = [['pl', 'Profit and loss'], ['bs', 'Balance sheet'], ['tb', 'Trial balance'], ['ledger', 'Ledger'], ['ar', 'Receivables'], ['ap', 'Payables'], ['tax', 'Tax'], ['statement', 'Statement']] as const;
	const tab = $derived(page.url.searchParams.get('tab') ?? 'pl');
	const firstOfYear = () => `${new Date().getFullYear()}-01-01`;
	let from = $state(firstOfYear());
	let to = $state(today());
	let account = $state(page.url.searchParams.get('account') ?? '');
	let party = $state<Party | null>(null);
	let partyKind = $state<'receivable' | 'payable'>('receivable');
	// The report shown, and which tab it belongs to (so a tab never draws another's data).
	let loaded = $state<{ tab: string; data: any } | null>(null);
	const data = $derived(loaded?.tab === tab ? loaded.data : null);
	let cur = $state('');
	baseCurrency().then((c) => (cur = c));

	const PATH: Record<string, string> = { pl: 'profit-loss', bs: 'balance-sheet', tb: 'trial-balance', ledger: 'ledger', ar: 'aging', ap: 'aging', tax: 'tax', statement: 'statement' };
	const params = $derived.by(() => {
		switch (tab) {
			case 'pl': case 'tax': return { from, to };
			case 'bs': return { as_of: to };
			case 'tb': return { from, to };
			case 'ledger': return { account_id: account || undefined, from, to };
			case 'ar': return { kind: 'receivable', as_of: to };
			case 'ap': return { kind: 'payable', as_of: to };
			case 'statement': return { party_id: party?.id, kind: partyKind, from, to };
		}
		return {};
	});
	async function load() {
		const t = tab;
		loaded = null;
		if (t === 'ledger' && !account) return;
		if (t === 'statement' && !party) return;
		try {
			const d = await api.get(`/accounting/reports/${PATH[t]}` + query(params as Record<string, string>));
			if (t === tab) loaded = { tab: t, data: d };
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		tab;
		params;
		load();
	});
	const csv = () => downloadUrl(`/api/accounting/reports/${PATH[tab]}` + query({ ...(params as Record<string, string>), format: 'csv' }), `${PATH[tab]}.csv`).catch(toasts.error);
	const m = (v: unknown) => amount(v as string, cur);
	const neg = (v: unknown) => (Number(v) < 0 ? 'text-red-600' : '');
</script>

<svelte:head><title>Reports · Accounting · NERP</title></svelte:head>

<div class="space-y-5">
	<h1 class="text-2xl font-semibold text-slate-900">Reports</h1>
	<div class="flex flex-wrap gap-1 border-b border-slate-200" role="tablist">
		{#each TABS as [k, label] (k)}
			<button role="tab" aria-selected={tab === k} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {tab === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}" onclick={() => goto(`?tab=${k}`, { replaceState: true })}>{label}</button>
		{/each}
	</div>

	<div class="flex flex-wrap items-end gap-3 text-sm">
		{#if !['bs', 'ar', 'ap'].includes(tab)}<div><label class="label" for="r-from">From</label><input id="r-from" type="date" class="input" bind:value={from} /></div>{/if}
		<div><label class="label" for="r-to">{['bs', 'ar', 'ap'].includes(tab) ? 'As of' : 'To'}</label><input id="r-to" type="date" class="input" bind:value={to} /></div>
		{#if tab === 'ledger'}<div class="min-w-72"><label class="label" for="r-acc">Account</label><AccountSelect id="r-acc" bind:value={account} /></div>{/if}
		{#if tab === 'statement'}
			<div class="min-w-72"><span class="label">Customer or supplier</span><PartyPicker bind:value={party} /></div>
			<div><label class="label" for="r-kind">Account</label><select id="r-kind" class="input" bind:value={partyKind}><option value="receivable">What they owe us</option><option value="payable">What we owe them</option></select></div>
		{/if}
		<button class="btn btn-secondary ml-auto" onclick={csv} disabled={!data}><Icon name="download" class="size-4" /> CSV</button>
	</div>

	{#if data}
		<div class="card max-w-5xl overflow-x-auto p-5" data-report={tab}>
			{#if tab === 'pl'}
				<table class="w-full text-sm">
					{#each data.sections as s (s.key)}
						{#if s.items.length}
							<tbody>
								<tr><th colspan="2" class="pt-3 text-left font-semibold text-slate-900">{s.title}</th></tr>
								{#each s.items as i (i.account_id)}<tr><td class="py-1 pl-4"><a class="hover:text-brand-700" href="?tab=ledger&account={i.account_id}">{i.code} · {i.name}</a></td><td class="py-1 text-right">{m(i.amount)}</td></tr>{/each}
								<tr class="border-t border-slate-100 font-medium"><td class="py-1 pl-4">Total {s.title.toLowerCase()}</td><td class="py-1 text-right">{m(s.total)}</td></tr>
							</tbody>
						{/if}
						{#if s.key === 'cost_of_sales'}<tbody><tr class="font-semibold"><td class="pt-3">Gross profit</td><td class="pt-3 text-right {neg(data.gross_profit)}">{m(data.gross_profit)}</td></tr></tbody>{/if}
					{/each}
					<tfoot><tr class="border-t-2 border-slate-300 text-base font-semibold"><td class="pt-3">Net profit</td><td class="pt-3 text-right {neg(data.net_profit)}" data-net-profit>{m(data.net_profit)}</td></tr></tfoot>
				</table>
			{:else if tab === 'bs'}
				<div class="grid gap-8 md:grid-cols-2">
					{#each [['assets', 'Assets'], ['liabilities', 'Liabilities'], ['equity', 'Equity']] as [k, title] (k)}
						<table class="w-full self-start text-sm">
							<tbody>
								<tr><th colspan="2" class="text-left font-semibold text-slate-900">{title}</th></tr>
								{#each data[k].items as i (i.code + i.name)}<tr><td class="py-1 pl-4">{i.code ? `${i.code} · ` : ''}{i.name}</td><td class="py-1 text-right">{m(i.amount)}</td></tr>{/each}
								<tr class="border-t border-slate-200 font-medium"><td class="py-1">Total {title.toLowerCase()}</td><td class="py-1 text-right" data-total={k}>{m(data[k].total)}</td></tr>
							</tbody>
						</table>
					{/each}
				</div>
				<p class="mt-4 text-sm {data.balanced ? 'text-emerald-700' : 'font-medium text-red-600'}" data-balanced={data.balanced}>Assets {m(data.assets.total)} = liabilities and equity {m(data.liabilities_and_equity)}{data.balanced ? '' : ' — out of balance!'}</p>
			{:else if tab === 'tb'}
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500 uppercase"><tr><th class="py-1">Account</th><th class="py-1 text-right">Opening</th><th class="py-1 text-right">Debits</th><th class="py-1 text-right">Credits</th><th class="py-1 text-right">Debit balance</th><th class="py-1 text-right">Credit balance</th></tr></thead>
					<tbody class="divide-y divide-slate-100">{#each data.items as i (i.account_id)}<tr><td class="py-1">{i.code} · {i.name}</td><td class="py-1 text-right">{m(i.opening)}</td><td class="py-1 text-right">{m(i.period_debit)}</td><td class="py-1 text-right">{m(i.period_credit)}</td><td class="py-1 text-right">{Number(i.debit) ? m(i.debit) : ''}</td><td class="py-1 text-right">{Number(i.credit) ? m(i.credit) : ''}</td></tr>{/each}</tbody>
					<tfoot class="border-t-2 border-slate-300 font-semibold"><tr><td class="py-1">Total</td><td></td><td></td><td></td><td class="py-1 text-right">{m(data.total_debit)}</td><td class="py-1 text-right">{m(data.total_credit)}</td></tr></tfoot>
				</table>
			{:else if tab === 'ledger'}
				<h2 class="mb-2 font-semibold">{data.code} · {data.name}</h2>
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500 uppercase"><tr><th class="py-1">Date</th><th class="py-1">Entry</th><th class="py-1">Details</th><th class="py-1 text-right">Debit</th><th class="py-1 text-right">Credit</th><th class="py-1 text-right">Balance</th></tr></thead>
					<tbody class="divide-y divide-slate-100">
						<tr class="text-slate-500"><td colspan="5" class="py-1">Opening balance</td><td class="py-1 text-right">{m(data.opening)}</td></tr>
						{#each data.items as i, k (k)}<tr><td class="py-1 whitespace-nowrap">{formatDate(i.date)}</td><td class="py-1"><a class="text-brand-700 hover:underline" href="/accounting/journal/{i.entry_id}">{i.entry_number}</a></td><td class="py-1">{i.description ?? i.memo ?? ''}{i.party_name && !(i.description ?? i.memo ?? '').includes(i.party_name) ? ` · ${i.party_name}` : ''}</td><td class="py-1 text-right">{Number(i.debit) ? m(i.debit) : ''}</td><td class="py-1 text-right">{Number(i.credit) ? m(i.credit) : ''}</td><td class="py-1 text-right">{m(i.balance)}</td></tr>{/each}
					</tbody>
					<tfoot class="border-t-2 border-slate-300 font-semibold"><tr><td colspan="5" class="py-1">Closing balance</td><td class="py-1 text-right" data-closing>{m(data.closing)}</td></tr></tfoot>
				</table>
			{:else if tab === 'ar' || tab === 'ap'}
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500 uppercase"><tr><th class="py-1">{tab === 'ar' ? 'Customer' : 'Supplier'}</th>{#each data.buckets as b (b)}<th class="py-1 text-right">{b === 'current' ? 'Not due' : `${b} days`}</th>{/each}<th class="py-1 text-right">Total</th></tr></thead>
					<tbody class="divide-y divide-slate-100">
						{#each data.items as i (i.party_name)}
							<tr><td class="py-1"><details><summary class="cursor-pointer">{i.party_name}</summary><ul class="mt-1 space-y-0.5 pl-4 text-xs text-slate-500">{#each i.documents as d (d.id)}<li>{d.label} · {formatDate(d.due_date ?? d.date)} · {amount(d.balance, d.currency)}</li>{/each}</ul></details></td>{#each i.buckets as b, k (k)}<td class="py-1 text-right align-top">{Number(b) ? m(b) : ''}</td>{/each}<td class="py-1 text-right align-top font-medium">{m(i.total)}</td></tr>
						{:else}<tr><td colspan="7" class="py-8 text-center text-slate-500">Nothing outstanding.</td></tr>{/each}
					</tbody>
					<tfoot class="border-t-2 border-slate-300 font-semibold"><tr><td class="py-1">Total</td>{#each data.totals as t, k (k)}<td class="py-1 text-right">{m(t)}</td>{/each}<td class="py-1 text-right">{m(data.total)}</td></tr></tfoot>
				</table>
			{:else if tab === 'tax'}
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500 uppercase"><tr><th class="py-1">Tax</th><th class="py-1">On</th><th class="py-1 text-right">Taxable amount</th><th class="py-1 text-right">Tax</th></tr></thead>
					<tbody class="divide-y divide-slate-100">{#each data.items as i (i.tax_id)}<tr><td class="py-1">{i.name} ({Number(i.rate)}%)</td><td class="py-1">{i.scope}</td><td class="py-1 text-right">{m(i.base)}</td><td class="py-1 text-right">{m(i.tax)}</td></tr>{:else}<tr><td colspan="4" class="py-8 text-center text-slate-500">No tax in this period.</td></tr>{/each}</tbody>
					<tfoot class="border-t-2 border-slate-300"><tr><td colspan="3" class="py-1">Owed on sales</td><td class="py-1 text-right">{m(data.sales_tax)}</td></tr><tr><td colspan="3" class="py-1">Reclaimable on purchases</td><td class="py-1 text-right">{m(data.purchase_tax)}</td></tr><tr class="font-semibold"><td colspan="3" class="py-1">Net to pay</td><td class="py-1 text-right">{m(data.net)}</td></tr></tfoot>
				</table>
			{:else if tab === 'statement'}
				<h2 class="mb-2 font-semibold">{data.party_name}</h2>
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500 uppercase"><tr><th class="py-1">Date</th><th class="py-1">Details</th><th class="py-1 text-right">Charges</th><th class="py-1 text-right">Payments</th><th class="py-1 text-right">Balance</th></tr></thead>
					<tbody class="divide-y divide-slate-100"><tr class="text-slate-500"><td colspan="4" class="py-1">Opening balance</td><td class="py-1 text-right">{m(data.opening)}</td></tr>{#each data.items as i, k (k)}<tr><td class="py-1">{formatDate(i.date)}</td><td class="py-1">{i.details}</td><td class="py-1 text-right">{Number(i.charge) ? m(i.charge) : ''}</td><td class="py-1 text-right">{Number(i.payment) ? m(i.payment) : ''}</td><td class="py-1 text-right">{m(i.balance)}</td></tr>{/each}</tbody>
					<tfoot class="border-t-2 border-slate-300 font-semibold"><tr><td colspan="4" class="py-1">Balance</td><td class="py-1 text-right">{m(data.closing)}</td></tr></tfoot>
				</table>
			{/if}
		</div>
	{:else if tab === 'ledger' && !account}
		<p class="text-sm text-slate-500">Choose an account.</p>
	{:else if tab === 'statement' && !party}
		<p class="text-sm text-slate-500">Choose a customer or supplier.</p>
	{/if}
</div>
