<script lang="ts">
	// Payments received and made.
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import { amount, STATUS_BADGE } from '#lib/accounting.ts';
	import PaymentModal from '#lib/components/accounting/PaymentModal.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Row = { id: string; number: number; direction: 'in' | 'out'; date: string; account_name: string; party_name: string | null; currency: string; amount: string; status: string; unallocated: string; reference: string | null };
	type Detail = Row & { account_id: string; party_id: string | null; fx_rate: string; memo: string | null; entry_id: string | null; entry_number: number | null; allocated: string; allocations: { doc_kind: string; doc_id: string; amount: string; label: string }[] };
	let dir = $state<'' | 'in' | 'out'>('');
	let onlyOpen = $state(false);
	let rows = $state<Row[] | null>(null);
	const canPost = session.can(PERM.ACCOUNTING_POST) || session.can(PERM.ACCOUNTING_ADMIN);

	async function load() {
		rows = (await api.get<{ items: Row[] }>('/accounting/payments' + query({ direction: dir || undefined, unallocated: onlyOpen || undefined, limit: 500 }))).items;
	}
	$effect(() => {
		dir;
		onlyOpen;
		load();
	});

	let newOpen = $state(false);
	let newDir = $state<'in' | 'out'>('in');

	let detail = $state<Detail | null>(null);
	let docs = $state<{ id: string; label: string; balance: string }[]>([]);
	let alloc = $state<Record<string, string>>({});
	async function show(id: string) {
		detail = await api.get<Detail>(`/accounting/payments/${id}`);
		docs = [];
		alloc = {};
		if (detail.status === 'posted' && Number(detail.unallocated) > 0 && detail.party_id) {
			if (detail.direction === 'in') {
				const r = await api.get<{ items: { item_id: string; number: number; title: string; balance: string; currency: string }[] }>('/accounting/invoices' + query({ status: 'open', party_id: detail.party_id }));
				docs = r.items.filter((i) => i.currency === detail!.currency).map((i) => ({ id: i.item_id, label: `Invoice ${i.number} · ${i.title}`, balance: i.balance }));
			} else {
				const r = await api.get<{ items: { id: string; number: number; reference: string | null; balance: string; currency: string }[] }>('/accounting/bills' + query({ status: 'open', party_id: detail.party_id }));
				docs = r.items.filter((b) => b.currency === detail!.currency).map((b) => ({ id: b.id, label: `Bill ${b.number}${b.reference ? ` · ${b.reference}` : ''}`, balance: b.balance }));
			}
		}
	}
	$effect(() => {
		const id = page.url.searchParams.get('id');
		if (id) show(id);
	});
	async function allocate() {
		const allocations = Object.entries(alloc).filter(([, v]) => Number(v) > 0).map(([doc_id, v]) => ({ doc_kind: detail!.direction === 'in' ? 'invoice' : 'bill', doc_id, amount: Number(v).toFixed(2) }));
		try {
			await api.post(`/accounting/payments/${detail!.id}/allocate`, { allocations });
			toasts.success('Allocated');
			await show(detail!.id);
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function voidIt() {
		if (!confirm(`Void payment ${detail!.number}? A reversing entry is posted and what it paid is open again.`)) return;
		try {
			await api.post(`/accounting/payments/${detail!.id}/void`, {});
			await show(detail!.id);
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Payments · Accounting · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Payments</h1>
			<p class="text-sm text-slate-500">Money received from customers and paid to suppliers.</p>
		</div>
		{#if canPost}
			<div class="flex gap-2">
				<button class="btn btn-secondary" onclick={() => ((newDir = 'out'), (newOpen = true))}>Money paid</button>
				<button class="btn btn-primary" onclick={() => ((newDir = 'in'), (newOpen = true))}>Money received</button>
			</div>
		{/if}
	</div>
	<div class="flex flex-wrap items-center gap-3 text-sm">
		<select class="input w-44" aria-label="Direction" bind:value={dir}><option value="">In and out</option><option value="in">Received</option><option value="out">Paid</option></select>
		<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={onlyOpen} /> Only with money not allocated</label>
	</div>
	{#if rows}
		<div class="card overflow-x-auto">
			<table class="w-full text-sm">
				<thead class="border-b border-slate-100 text-left text-xs text-slate-500 uppercase"><tr><th class="px-4 py-2">No.</th><th class="px-4 py-2">Date</th><th class="px-4 py-2">Who</th><th class="px-4 py-2">Account</th><th class="px-4 py-2">Reference</th><th class="px-4 py-2 text-right">Amount</th><th class="px-4 py-2 text-right">Not allocated</th><th class="px-4 py-2">Status</th></tr></thead>
				<tbody class="divide-y divide-slate-100">
					{#each rows as r (r.id)}
						<tr class="cursor-pointer hover:bg-slate-50" onclick={() => show(r.id)} data-payment={r.number}>
							<td class="px-4 py-2 text-slate-500">{r.number}</td>
							<td class="px-4 py-2 whitespace-nowrap">{formatDate(r.date)}</td>
							<td class="px-4 py-2 font-medium">{r.party_name ?? '—'}</td>
							<td class="px-4 py-2">{r.account_name}</td>
							<td class="px-4 py-2">{r.reference ?? ''}</td>
							<td class="px-4 py-2 text-right whitespace-nowrap {r.direction === 'in' ? 'text-emerald-700' : ''}">{r.direction === 'in' ? '+' : '−'}{amount(r.amount, r.currency)}</td>
							<td class="px-4 py-2 text-right whitespace-nowrap">{Number(r.unallocated) > 0 && r.status === 'posted' ? amount(r.unallocated, r.currency) : ''}</td>
							<td class="px-4 py-2"><span class="badge {STATUS_BADGE[r.status]} capitalize">{r.status}</span></td>
						</tr>
					{:else}
						<tr><td colspan="8" class="px-4 py-10 text-center text-slate-500">No payments yet.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<PaymentModal bind:open={newOpen} direction={newDir} onsaved={load} />

<Modal open={!!detail} title="Payment {detail?.number ?? ''}" onclose={() => (detail = null)} size="lg">
	{#if detail}
		<div class="space-y-4 text-sm" data-payment-detail>
			<dl class="grid grid-cols-2 gap-x-4 gap-y-1">
				<dt class="text-slate-500">{detail.direction === 'in' ? 'From' : 'To'}</dt><dd>{detail.party_name ?? '—'}</dd>
				<dt class="text-slate-500">Date</dt><dd>{formatDate(detail.date)}</dd>
				<dt class="text-slate-500">Account</dt><dd>{detail.account_name}</dd>
				<dt class="text-slate-500">Amount</dt><dd>{amount(detail.amount, detail.currency)}{detail.fx_rate !== '1' && Number(detail.fx_rate) !== 1 ? ` at ${detail.fx_rate}` : ''}</dd>
				<dt class="text-slate-500">Status</dt><dd><span class="badge {STATUS_BADGE[detail.status]} capitalize">{detail.status}</span>{#if detail.entry_number}<a class="ml-2 text-brand-700 hover:underline" href="/accounting/journal/{detail.entry_id}">Entry {detail.entry_number}</a>{/if}</dd>
			</dl>
			<div>
				<h3 class="mb-1 font-medium">Pays</h3>
				<ul class="divide-y divide-slate-100 rounded-lg border border-slate-200">
					{#each detail.allocations as a (a.doc_id)}<li class="flex justify-between px-3 py-2"><span>{a.label}</span><span>{amount(a.amount, detail.currency)}</span></li>{:else}<li class="px-3 py-2 text-slate-500">Nothing yet.</li>{/each}
					{#if Number(detail.unallocated) > 0}<li class="flex justify-between px-3 py-2 font-medium"><span>Not allocated</span><span>{amount(detail.unallocated, detail.currency)}</span></li>{/if}
				</ul>
			</div>
			{#if docs.length && canPost}
				<div>
					<h3 class="mb-1 font-medium">Allocate what is left</h3>
					<ul class="divide-y divide-slate-100 rounded-lg border border-slate-200">
						{#each docs as d (d.id)}<li class="flex items-center gap-3 px-3 py-2"><span class="flex-1">{d.label} <span class="text-xs text-slate-500">({amount(d.balance, detail.currency)} left)</span></span><input class="input w-28 text-right" inputmode="decimal" aria-label="Allocate to {d.label}" bind:value={alloc[d.id]} /></li>{/each}
					</ul>
					<button class="btn btn-secondary mt-2" onclick={allocate}>Allocate</button>
				</div>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		{#if detail?.status === 'posted' && canPost}<button class="btn btn-ghost mr-auto text-red-600" onclick={voidIt}>Void payment</button>{/if}
		<button class="btn btn-secondary" onclick={() => (detail = null)}>Close</button>
	{/snippet}
</Modal>
