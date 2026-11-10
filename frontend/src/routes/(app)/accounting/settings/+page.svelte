<script lang="ts">
	// How the books are kept: default accounts, the financial year, the lock date, taxes,
	// opening balances and year-end closing.
	import { api } from '#lib/api.ts';
	import { accounts, amount, taxes, today, type Account, type Party, type Settings, type Tax } from '#lib/accounting.ts';
	import AccountSelect from '#lib/components/accounting/AccountSelect.svelte';
	import PartyPicker from '#lib/components/accounting/PartyPicker.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
	let s = $state<Settings | null>(null);
	let base = $state('');
	let taxList = $state<Tax[]>([]);
	let accountList = $state<Account[]>([]);
	async function load() {
		const r = await api.get<{ settings: Settings; base_currency: string }>('/accounting/settings');
		s = r.settings;
		base = r.base_currency;
		taxList = await taxes(true);
		accountList = await accounts(true);
	}
	$effect(() => {
		load();
	});

	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.put('/accounting/settings', { ...s, lock_date: s!.lock_date || null });
			toasts.success('Settings saved');
			load();
		} catch (err) {
			toasts.error(err);
		}
	}

	// Taxes
	let tax = $state<Partial<Tax> | null>(null);
	async function saveTax(e: SubmitEvent) {
		e.preventDefault();
		try {
			if (tax!.id) await api.patch(`/accounting/taxes/${tax!.id}`, { name: tax!.name, account_id: tax!.account_id, is_active: tax!.is_active });
			else await api.post('/accounting/taxes', tax as Record<string, unknown>);
			tax = null;
			taxList = await taxes(true);
		} catch (err) {
			toasts.error(err);
		}
	}

	// Opening balances
	let opening = $state<{ date: string; lines: { account_id: string; debit: string; credit: string; party: Party | null }[] } | null>(null);
	const openingDiff = $derived(opening ? opening.lines.reduce((t, l) => t + (Number(l.debit) || 0) - (Number(l.credit) || 0), 0) : 0);
	async function saveOpening(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.post('/accounting/opening', {
				date: opening!.date,
				lines: opening!.lines.filter((l) => l.account_id).map((l) => ({ account_id: l.account_id, debit: (Number(l.debit) || 0).toFixed(2), credit: (Number(l.credit) || 0).toFixed(2), party_id: l.party?.id ?? null }))
			});
			toasts.success('Opening balances posted');
			opening = null;
		} catch (err) {
			toasts.error(err);
		}
	}

	// Year-end closing: the last few year ends.
	const yearEnds = $derived.by(() => {
		if (!s) return [];
		const out: string[] = [];
		const now = new Date();
		for (let k = 0; k < 6; k++) {
			const end = new Date(Date.UTC(now.getUTCFullYear() - k, s.fiscal_year_start - 1, 1) - 86400000);
			if (end < now) out.push(end.toISOString().slice(0, 10));
		}
		return out;
	});
	let closeEnd = $state('');
	async function closeYear() {
		if (!closeEnd || !confirm(`Close the year ending ${formatDate(closeEnd)}? Its profit moves to retained earnings and the books are locked up to that day.`)) return;
		try {
			await api.post('/accounting/close', { year_end: closeEnd });
			toasts.success('Year closed');
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	const accountName = (id: string) => accountList.find((a) => a.id === id)?.name ?? '';
</script>

<svelte:head><title>Accounting settings · NERP</title></svelte:head>

<div class="max-w-3xl space-y-5">
	<h1 class="text-2xl font-semibold text-slate-900">Accounting settings</h1>
	{#if s && !s.ready}
		<div class="card p-6 text-sm text-slate-600">Set up the books first on the <a class="text-brand-700 hover:underline" href="/accounting">Accounting</a> page.</div>
	{:else if s}
		<form class="card space-y-4 p-6" onsubmit={save}>
			<p class="text-sm text-slate-500">The books are kept in {base} (the CRM's base currency).</p>
			<div class="grid gap-4 sm:grid-cols-2">
				<div><label class="label" for="s-fy">Financial year starts in</label><select id="s-fy" class="input" bind:value={s.fiscal_year_start}>{#each MONTHS as m, i (m)}<option value={i + 1}>{m}</option>{/each}</select></div>
				<div><label class="label" for="s-lock">Locked up to (nothing on or before can change)</label><input id="s-lock" type="date" class="input" bind:value={s.lock_date} /></div>
			</div>
			<h2 class="pt-2 font-semibold text-slate-900">Default accounts</h2>
			<div class="grid gap-4 sm:grid-cols-2">
				<div><label class="label" for="d-ar">Receivables (customers)</label><AccountSelect id="d-ar" subtypes={['receivable']} bind:value={s.receivable_account} /></div>
				<div><label class="label" for="d-ap">Payables (suppliers)</label><AccountSelect id="d-ap" subtypes={['payable']} bind:value={s.payable_account} /></div>
				<div><label class="label" for="d-sales">Sales (invoice lines without their own account)</label><AccountSelect id="d-sales" subtypes={['income', 'other_income']} bind:value={s.sales_account} /></div>
				<div><label class="label" for="d-re">Retained earnings (year-end closing)</label><AccountSelect id="d-re" subtypes={['retained_earnings', 'equity']} bind:value={s.retained_earnings} /></div>
				<div><label class="label" for="d-ob">Opening balances</label><AccountSelect id="d-ob" subtypes={['equity', 'retained_earnings']} bind:value={s.opening_balances} /></div>
				<div><label class="label" for="d-fx">Exchange differences</label><AccountSelect id="d-fx" types={['income', 'expense']} bind:value={s.fx_account} /></div>
				<div><label class="label" for="d-round">Rounding differences</label><AccountSelect id="d-round" types={['income', 'expense']} bind:value={s.rounding_account} /></div>
			</div>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={s.mark_invoices_paid} /> Move an invoice to "Paid" in the CRM when it is fully paid</label>
			<button class="btn btn-primary">Save settings</button>
		</form>

		<section class="card">
			<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3"><h2 class="font-semibold text-slate-900">Taxes</h2><button class="btn btn-secondary" onclick={() => (tax = { name: '', rate: '', scope: 'sales', account_id: '', is_active: true })}>Add tax</button></div>
			<ul class="divide-y divide-slate-100 text-sm">
				{#each taxList as t (t.id)}
					<li class="flex items-center gap-3 px-5 py-2 {t.is_active ? '' : 'text-slate-400'}" data-tax={t.name}>
						<span class="flex-1">{t.name}</span><span class="w-16 text-right">{Number(t.rate)}%</span><span class="w-28 text-slate-500">{t.scope === 'sales' ? 'on sales' : 'on purchases'}</span><span class="w-48 truncate text-slate-500">{accountName(t.account_id)}</span>
						<button class="text-brand-600 hover:underline" onclick={() => (tax = { ...t })}>Edit</button>
					</li>
				{:else}
					<li class="px-5 py-6 text-center text-slate-500">No taxes yet. CRM invoices with a tax rate need a sales tax at that rate.</li>
				{/each}
			</ul>
		</section>

		<section class="card space-y-3 p-5">
			<h2 class="font-semibold text-slate-900">Opening balances</h2>
			<p class="text-sm text-slate-600">Balances carried over from your previous books on the day you start; whatever doesn't balance goes to the opening balances account.</p>
			<button class="btn btn-secondary" onclick={() => (opening = { date: today(), lines: [{ account_id: '', debit: '', credit: '', party: null }, { account_id: '', debit: '', credit: '', party: null }] })}>Enter opening balances</button>
		</section>

		<section class="card space-y-3 p-5">
			<h2 class="font-semibold text-slate-900">Year-end closing</h2>
			<p class="text-sm text-slate-600">Moves the year's profit (income and expenses) to retained earnings and locks the books up to the year's last day. To reopen a year, move the lock date back and reverse its closing entry in the journal.</p>
			<div class="flex gap-2">
				<select class="input w-48" aria-label="Year ending" bind:value={closeEnd}><option value="">Year ending…</option>{#each yearEnds as y (y)}<option value={y}>{formatDate(y)}</option>{/each}</select>
				<button class="btn btn-secondary" disabled={!closeEnd} onclick={closeYear}>Close the year</button>
			</div>
		</section>
	{/if}
</div>

<Modal open={!!tax} title={tax?.id ? 'Edit tax' : 'New tax'} onclose={() => (tax = null)}>
	{#if tax}
		<form id="tax-form" class="grid gap-3 sm:grid-cols-2" onsubmit={saveTax}>
			<div class="sm:col-span-2"><label class="label" for="t-name">Name</label><input id="t-name" class="input" required bind:value={tax.name} /></div>
			<div><label class="label" for="t-rate">Rate (%)</label><input id="t-rate" class="input" inputmode="decimal" required disabled={!!tax.id} bind:value={tax.rate} /></div>
			<div><label class="label" for="t-scope">On</label><select id="t-scope" class="input" disabled={!!tax.id} bind:value={tax.scope}><option value="sales">Sales (owed)</option><option value="purchases">Purchases (reclaimable)</option></select></div>
			<div class="sm:col-span-2"><label class="label" for="t-acc">Account</label><AccountSelect id="t-acc" subtypes={tax.scope === 'sales' ? ['tax_payable'] : ['tax_receivable', 'tax_payable']} required bind:value={tax.account_id} /></div>
			{#if tax.id}<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={tax.is_active} /> Active</label>{/if}
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (tax = null)}>Cancel</button>
		<button class="btn btn-primary" form="tax-form">Save</button>
	{/snippet}
</Modal>

<Modal open={!!opening} title="Opening balances" size="lg" onclose={() => (opening = null)}>
	{#if opening}
		<form id="opening-form" class="space-y-3" onsubmit={saveOpening}>
			<div class="w-48"><label class="label" for="o-date">As of</label><input id="o-date" type="date" class="input" required bind:value={opening.date} /></div>
			<table class="w-full text-sm">
				<thead class="text-left text-xs text-slate-500 uppercase"><tr><th>Account</th><th>Customer / supplier</th><th class="w-28 text-right">Debit</th><th class="w-28 text-right">Credit</th></tr></thead>
				<tbody>
					{#each opening.lines as l, i (i)}
						<tr><td class="pr-2 pb-2"><AccountSelect label="Opening line {i + 1} account" bind:value={l.account_id} /></td><td class="pr-2 pb-2"><PartyPicker bind:value={l.party} placeholder="For receivables and payables" /></td><td class="pr-2 pb-2"><input class="input text-right" inputmode="decimal" aria-label="Opening line {i + 1} debit" bind:value={l.debit} /></td><td class="pb-2"><input class="input text-right" inputmode="decimal" aria-label="Opening line {i + 1} credit" bind:value={l.credit} /></td></tr>
					{/each}
				</tbody>
			</table>
			<button type="button" class="text-sm text-brand-600 hover:underline" onclick={() => opening!.lines.push({ account_id: '', debit: '', credit: '', party: null })}>+ Line</button>
			{#if Math.abs(openingDiff) > 0.004}<p class="text-xs text-slate-500">{amount(Math.abs(openingDiff), base)} goes to the opening balances account to balance.</p>{/if}
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (opening = null)}>Cancel</button>
		<button class="btn btn-primary" form="opening-form">Post opening balances</button>
	{/snippet}
</Modal>
