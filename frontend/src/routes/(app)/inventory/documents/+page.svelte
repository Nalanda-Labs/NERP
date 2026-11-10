<script lang="ts">
	// Inventory → Documents: receipts, write-offs, transfers and stock counts.
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { money } from '#lib/crm.svelte.ts';
	import { KIND, STATUS, newDocument, qty, type DocKind } from '#lib/inventory.ts';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Doc = { id: string; kind: DocKind; number: string; status: string; doc_date: string; from_name: string | null; to_name: string | null; supplier_name: string | null; reference: string | null; lines: number; units: string; cost: string | null; currency: string | null };
	let items = $state<Doc[]>([]);
	let total = $state(0);
	let kind = $state('');
	let status = $state('');
	let q = $state('');
	let seq = 0;
	async function load() {
		const mine = ++seq;
		const r = await api.get<{ items: Doc[]; total: number }>('/inventory/documents' + query({ kind: kind || undefined, status: status || undefined, q: q.trim() || undefined, limit: 200 }));
		if (mine !== seq) return;
		items = r.items;
		total = r.total;
	}
	onMount(load);
	let timer: ReturnType<typeof setTimeout> | undefined;
	const search = () => (clearTimeout(timer), (timer = setTimeout(load, 250)));
	const canManage = $derived(session.can(PERM.INVENTORY) || session.can(PERM.CRM_ADMIN));
	function where(d: Doc) {
		if (d.kind === 'transfer') return `${d.from_name ?? '—'} → ${d.to_name ?? '—'}`;
		return d.from_name ?? d.to_name ?? '—';
	}
</script>

<svelte:head><title>Stock documents · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Stock documents</h1>
		<span class="text-sm text-slate-500">{total}</span>
		{#if canManage}
			<div class="ml-auto flex flex-wrap gap-2">
				{#each Object.entries(KIND) as [k, v] (k)}
					<button class="btn {k === 'receipt' ? 'btn-primary' : 'btn-secondary'}" onclick={() => newDocument(k as DocKind)}>{v.action}</button>
				{/each}
			</div>
		{/if}
	</div>
	<div class="flex flex-wrap gap-3">
		<input class="input max-w-xs" type="search" placeholder="Number, reference or comment" aria-label="Search documents" bind:value={q} oninput={search} />
		<select class="input w-44" aria-label="Kind" bind:value={kind} onchange={load}>
			<option value="">All kinds</option>
			{#each Object.entries(KIND) as [k, v] (k)}<option value={k}>{v.name}</option>{/each}
		</select>
		<select class="input w-40" aria-label="Status" bind:value={status} onchange={load}>
			<option value="">Any status</option>
			{#each Object.entries(STATUS) as [k, v] (k)}<option value={k}>{v[0]}</option>{/each}
		</select>
	</div>
	<div class="card overflow-x-auto">
		<table class="min-w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
				<tr><th class="px-4 py-3">Number</th><th class="px-4 py-3">Kind</th><th class="px-4 py-3">Date</th><th class="px-4 py-3">Warehouse</th><th class="px-4 py-3">Supplier / reference</th><th class="px-4 py-3 text-right">Units</th><th class="px-4 py-3 text-right">Cost</th><th class="px-4 py-3">Status</th></tr>
			</thead>
			<tbody class="divide-y divide-slate-50">
				{#each items as d (d.id)}
					<tr class="hover:bg-slate-50" data-doc={d.number}>
						<td class="px-4 py-2.5"><a class="font-medium text-brand-600 hover:underline" href="/inventory/documents/{d.id}">{d.number}</a></td>
						<td class="px-4 py-2.5 text-slate-700">{KIND[d.kind].name}</td>
						<td class="px-4 py-2.5 text-slate-600">{formatDate(d.doc_date)}</td>
						<td class="px-4 py-2.5 text-slate-700">{where(d)}</td>
						<td class="px-4 py-2.5 text-slate-600">{[d.supplier_name, d.reference].filter(Boolean).join(' · ')}</td>
						<td class="px-4 py-2.5 text-right tabular-nums">{qty(d.units)}</td>
						<td class="px-4 py-2.5 text-right tabular-nums">{d.cost !== null ? money(d.cost, d.currency) : ''}</td>
						<td class="px-4 py-2.5"><span class="badge {STATUS[d.status][1]}">{STATUS[d.status][0]}</span></td>
					</tr>
				{:else}
					<tr><td colspan="8" class="px-4 py-14 text-center text-slate-500">No documents yet.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
