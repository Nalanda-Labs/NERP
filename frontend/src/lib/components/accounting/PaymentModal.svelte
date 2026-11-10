<script lang="ts">
	// Recording money received (in) or paid (out), allocated to the customer's invoices or the
	// supplier's bills. Also used for a bank statement line (its amount, date and account fixed).
	import { api, query } from '#lib/api.ts';
	import { accounts, amount, today, type Party } from '#lib/accounting.ts';
	import AccountSelect from '#lib/components/accounting/AccountSelect.svelte';
	import PartyPicker from '#lib/components/accounting/PartyPicker.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Doc = { id: string; label: string; date: string; due_date: string | null; currency: string; balance: string };
	let {
		open = $bindable(false),
		direction = 'in',
		party = null,
		docId = null,
		bankLine = null,
		onsaved
	}: {
		open?: boolean;
		direction?: 'in' | 'out';
		party?: Party | null;
		/** Preselect paying this invoice (in) or bill (out) in full. */
		docId?: string | null;
		/** A statement line: { id, amount, date, account_id, currency } */
		bankLine?: { id: string; amount: string; date: string; account_id: string; currency: string } | null;
		onsaved?: () => void;
	} = $props();

	let who = $state<Party | null>(null);
	let date = $state(today());
	let account = $state('');
	let total = $state('');
	let fx = $state('');
	let reference = $state('');
	let docs = $state<Doc[]>([]);
	let alloc = $state<Record<string, string>>({});
	let busy = $state(false);
	let base = $state('');
	let accountCurrency = $state<string | null>(null);

	const kind = $derived(direction === 'in' ? 'invoice' : 'bill');
	$effect(() => {
		if (!open) return;
		who = party;
		date = bankLine?.date ?? today();
		account = bankLine?.account_id ?? '';
		total = bankLine ? String(Math.abs(Number(bankLine.amount))) : '';
		fx = '';
		reference = '';
		alloc = {};
		api.get<{ base_currency: string }>('/accounting/settings').then((r) => (base = r.base_currency));
		loadDocs();
	});
	async function loadDocs() {
		docs = [];
		if (!who) return;
		if (direction === 'in') {
			const r = await api.get<{ items: { item_id: string; number: number; title: string; date: string; due_date: string | null; currency: string; balance: string }[] }>('/accounting/invoices' + query({ status: 'open', party_id: who.id, limit: 200 }));
			docs = r.items.map((i) => ({ id: i.item_id, label: `Invoice ${i.number} · ${i.title}`, date: i.date, due_date: i.due_date, currency: i.currency, balance: i.balance }));
		} else {
			const r = await api.get<{ items: { id: string; number: number; reference: string | null; date: string; due_date: string | null; currency: string; balance: string }[] }>('/accounting/bills' + query({ status: 'open', party_id: who.id, limit: 200 }));
			docs = r.items.map((b) => ({ id: b.id, label: `Bill ${b.number}${b.reference ? ` · ${b.reference}` : ''}`, date: b.date, due_date: b.due_date, currency: b.currency, balance: b.balance }));
		}
		docs.sort((a, b) => (a.due_date ?? a.date).localeCompare(b.due_date ?? b.date));
		if (docId && docs.some((d) => d.id === docId)) {
			const d = docs.find((x) => x.id === docId)!;
			alloc = { [d.id]: d.balance };
			if (!bankLine) total = d.balance;
		}
	}
	/** Fills allocations oldest first from the amount. */
	function spread() {
		let left = Number(total) || 0;
		const next: Record<string, string> = {};
		for (const d of docs) {
			if (left <= 0) break;
			const a = Math.min(left, Number(d.balance));
			next[d.id] = a.toFixed(2);
			left = Math.round((left - a) * 100) / 100;
		}
		alloc = next;
	}
	const allocated = $derived(Object.values(alloc).reduce((t, v) => t + (Number(v) || 0), 0));
	const rest = $derived(Math.round(((Number(total) || 0) - allocated) * 100) / 100);
	const shownCurrency = $derived(bankLine?.currency ?? accountCurrency ?? base);

	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		const allocations = Object.entries(alloc)
			.filter(([, v]) => Number(v) > 0)
			.map(([doc_id, v]) => ({ doc_kind: kind, doc_id, amount: Number(v).toFixed(2) }));
		try {
			if (bankLine) {
				await api.post(`/accounting/bank/lines/${bankLine.id}/payment`, { party_id: who?.id ?? null, allocations });
			} else {
				await api.post('/accounting/payments', {
					direction,
					date,
					account_id: account,
					party_id: who?.id ?? null,
					amount: Number(total).toFixed(2),
					fx_rate: fx || null,
					reference: reference || null,
					allocations
				});
			}
			toasts.success(direction === 'in' ? 'Payment received' : 'Payment made');
			open = false;
			onsaved?.();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

<Modal bind:open title={direction === 'in' ? 'Money received' : 'Money paid'} size="lg">
	<form id="payment-form" class="space-y-4" onsubmit={save}>
		<div>
			<span class="label">{direction === 'in' ? 'From' : 'To'}</span>
			<PartyPicker bind:value={who} label={direction === 'in' ? 'Customer' : 'Supplier'} onpick={loadDocs} />
		</div>
		{#if !bankLine}
			<div class="grid gap-3 sm:grid-cols-3">
				<div><label class="label" for="pay-date">Date</label><input id="pay-date" type="date" class="input" required bind:value={date} /></div>
				<div class="sm:col-span-2">
					<label class="label" for="pay-account">{direction === 'in' ? 'Into' : 'From'}</label>
					<AccountSelect id="pay-account" subtypes={['bank', 'cash']} required bind:value={account} onchange={(v) => accounts().then((a) => (accountCurrency = a.find((x) => x.id === v)?.currency ?? null))} />
				</div>
				<div><label class="label" for="pay-amount">Amount</label><input id="pay-amount" class="input" inputmode="decimal" required bind:value={total} /></div>
				<div><label class="label" for="pay-ref">Reference</label><input id="pay-ref" class="input" bind:value={reference} /></div>
				<div><label class="label" for="pay-fx">Exchange rate <span class="font-normal text-slate-400">(foreign)</span></label><input id="pay-fx" class="input" inputmode="decimal" placeholder="The CRM's rate" bind:value={fx} /></div>
			</div>
		{:else}
			<p class="rounded-lg bg-slate-50 px-3 py-2 text-sm text-slate-600">{formatDate(bankLine.date)} · {amount(Math.abs(Number(bankLine.amount)), bankLine.currency)}</p>
		{/if}

		{#if who}
			<div>
				<div class="mb-1 flex items-center justify-between">
					<span class="label mb-0">Pays {direction === 'in' ? 'invoices' : 'bills'}</span>
					{#if docs.length}<button type="button" class="text-xs text-brand-600 hover:underline" onclick={spread}>Oldest first</button>{/if}
				</div>
				{#if docs.length}
					<ul class="divide-y divide-slate-100 rounded-lg border border-slate-200" aria-label="Open documents">
						{#each docs as d (d.id)}
							<li class="flex items-center gap-3 px-3 py-2 text-sm" data-doc={d.label}>
								<span class="min-w-0 flex-1"><span class="block truncate">{d.label}</span><span class="text-xs text-slate-500">due {formatDate(d.due_date ?? d.date)} · {amount(d.balance, d.currency)} left</span></span>
								<input class="input w-28 text-right" inputmode="decimal" aria-label="Amount for {d.label}" placeholder="0.00" bind:value={alloc[d.id]} />
							</li>
						{/each}
					</ul>
				{:else}
					<p class="text-sm text-slate-500">Nothing open; the whole amount stays on their account as an advance.</p>
				{/if}
				{#if rest > 0 && docs.length}<p class="mt-1 text-xs text-slate-500">{amount(rest, shownCurrency)} stays on their account as an advance.</p>{/if}
				{#if rest < 0}<p class="mt-1 text-xs font-medium text-red-600">More is allocated than the amount.</p>{/if}
			</div>
		{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="payment-form" disabled={busy || !who || rest < 0}>{direction === 'in' ? 'Record payment received' : 'Record payment made'}</button>
	{/snippet}
</Modal>
