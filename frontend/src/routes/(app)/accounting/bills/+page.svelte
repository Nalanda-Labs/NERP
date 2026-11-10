<script lang="ts">
	// Bills from suppliers.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import { amount, STATUS_BADGE } from '#lib/accounting.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Row = { id: string; number: number | null; reference: string | null; party_name: string | null; date: string; due_date: string | null; currency: string; total: string; status: string; balance: string; overdue_days: number | null };
	const TABS = [['draft', 'Drafts'], ['open', 'Open'], ['overdue', 'Overdue'], ['paid', 'Paid'], ['void', 'Void'], ['all', 'All']] as const;
	const status = $derived(page.url.searchParams.get('status') ?? 'open');
	let rows = $state<Row[] | null>(null);
	let q = $state('');
	async function load() {
		rows = (await api.get<{ items: Row[] }>('/accounting/bills' + query({ status, q: q || undefined, limit: 500 }))).items;
	}
	$effect(() => {
		status;
		load();
	});
	let t: ReturnType<typeof setTimeout> | undefined;
	const search = () => (clearTimeout(t), (t = setTimeout(load, 250)));
</script>

<svelte:head><title>Bills · Accounting · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Bills</h1>
			<p class="text-sm text-slate-500">What suppliers invoice you: expenses, the tax on them, and what you owe.</p>
		</div>
		<div class="flex gap-2">
			<input class="input w-56" type="search" placeholder="Supplier or reference" aria-label="Search bills" bind:value={q} oninput={search} />
			{#if session.can(PERM.ACCOUNTING_POST) || session.can(PERM.ACCOUNTING_ADMIN)}<a class="btn btn-primary" href="/accounting/bills/new"><Icon name="plus" class="size-4" /> New bill</a>{/if}
		</div>
	</div>
	<div class="flex flex-wrap gap-1 border-b border-slate-200" role="tablist">
		{#each TABS as [k, label] (k)}
			<button role="tab" aria-selected={status === k} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {status === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}" onclick={() => goto(`?status=${k}`, { replaceState: true })}>{label}</button>
		{/each}
	</div>
	{#if rows}
		<div class="card overflow-x-auto">
			<table class="w-full text-sm">
				<thead class="border-b border-slate-100 text-left text-xs text-slate-500 uppercase">
					<tr><th class="px-4 py-2">No.</th><th class="px-4 py-2">Supplier</th><th class="px-4 py-2">Their reference</th><th class="px-4 py-2">Date</th><th class="px-4 py-2">Due</th><th class="px-4 py-2 text-right">Total</th><th class="px-4 py-2 text-right">Left to pay</th><th class="px-4 py-2">Status</th></tr>
				</thead>
				<tbody class="divide-y divide-slate-100">
					{#each rows as r (r.id)}
						<tr class="cursor-pointer hover:bg-slate-50" onclick={() => goto(`/accounting/bills/${r.id}`)} data-bill={r.reference ?? r.id}>
							<td class="px-4 py-2 text-slate-500">{r.number ?? '—'}</td>
							<td class="px-4 py-2 font-medium text-slate-800">{r.party_name ?? '—'}</td>
							<td class="px-4 py-2">{r.reference ?? ''}</td>
							<td class="px-4 py-2 whitespace-nowrap">{formatDate(r.date)}</td>
							<td class="px-4 py-2 whitespace-nowrap {r.overdue_days ? 'font-medium text-red-600' : ''}">{formatDate(r.due_date)}</td>
							<td class="px-4 py-2 text-right whitespace-nowrap">{amount(r.total, r.currency)}</td>
							<td class="px-4 py-2 text-right whitespace-nowrap">{r.status === 'open' || r.status === 'paid' ? amount(r.balance, r.currency) : ''}</td>
							<td class="px-4 py-2"><span class="badge {STATUS_BADGE[r.status]} capitalize">{r.status}</span></td>
						</tr>
					{:else}
						<tr><td colspan="8" class="px-4 py-10 text-center text-slate-500">No bills here.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>
