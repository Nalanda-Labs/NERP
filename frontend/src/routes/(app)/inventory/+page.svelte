<script lang="ts">
	// Inventory → Stock: counted products and what each warehouse holds, with each product's
	// movement history.
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import { KIND, REASON, newDocument, qty, type DocKind, type Warehouse } from '#lib/inventory.ts';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Row = { product_id: string; name: string; sku: string | null; unit: string; total: string; per: Record<string, string> };
	let rows = $state<Row[]>([]);
	let total = $state(0);
	let warehouses = $state<Warehouse[]>([]);
	let q = $state('');
	let level = $state<'all' | 'low' | 'out'>('all');
	let warehouse = $state('');
	let seq = 0;
	async function load() {
		const mine = ++seq;
		const r = await api.get<{ items: Row[]; total: number; warehouses: Warehouse[] }>(
			'/inventory/stock' + query({ q: q.trim() || undefined, state: level === 'all' ? undefined : level, warehouse: warehouse || undefined, limit: 500 })
		);
		if (mine !== seq) return;
		rows = r.items;
		total = r.total;
		warehouses = r.warehouses;
	}
	onMount(load);
	let timer: ReturnType<typeof setTimeout> | undefined;
	const search = () => (clearTimeout(timer), (timer = setTimeout(load, 250)));
	const shown = $derived(warehouses.filter((w) => w.is_active && (!warehouse || w.id === warehouse)));
	const canManage = $derived(session.can(PERM.INVENTORY) || session.can(PERM.CRM_ADMIN));

	type History = {
		name: string;
		unit: string;
		total: string | null;
		warehouses: { id: string; name: string; quantity: string }[];
		movements: { id: string; warehouse: string; delta: string; balance: string; reason: string; document_id: string | null; document_number: string | null; item_id: string | null; item_number: number | null; item_code: string | null; user_name: string | null; created_at: string }[];
	};
	let history = $state<History | null>(null);
	let historyOpen = $state(false);
	async function show(r: Row) {
		history = await api.get<History>(`/inventory/products/${r.product_id}`);
		historyOpen = true;
	}
	const level_ = (n: number) => (n <= 0 ? 'text-red-700 font-medium' : n <= 5 ? 'text-amber-800 font-medium' : 'text-slate-800');
</script>

<svelte:head><title>Stock · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Stock</h1>
		<span class="text-sm text-slate-500">{total} product{total === 1 ? '' : 's'} counted</span>
		{#if canManage}
			<div class="ml-auto flex flex-wrap gap-2">
				{#each Object.entries(KIND) as [k, v] (k)}
					<button class="btn {k === 'receipt' ? 'btn-primary' : 'btn-secondary'}" onclick={() => newDocument(k as DocKind)}>{v.action}</button>
				{/each}
			</div>
		{/if}
	</div>
	<div class="flex flex-wrap items-center gap-3">
		<input class="input max-w-xs" type="search" placeholder="Search name or SKU" aria-label="Search name or SKU" bind:value={q} oninput={search} />
		<select class="input w-48" aria-label="Warehouse" bind:value={warehouse} onchange={load}>
			<option value="">All warehouses</option>
			{#each warehouses.filter((w) => w.is_active) as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
		</select>
		<div class="flex gap-1" role="group" aria-label="Show">
			{#each [['all', 'All'], ['low', '5 or fewer'], ['out', 'Out of stock']] as [v, label] (v)}
				<button class="rounded-full px-3 py-1.5 text-sm font-medium {level === v ? 'bg-slate-900 text-white' : 'bg-white text-slate-700 ring-1 ring-slate-200 hover:bg-slate-100'}" aria-pressed={level === v} onclick={() => ((level = v as typeof level), load())}>{label}</button>
			{/each}
		</div>
	</div>
	<div class="card overflow-x-auto">
		<table class="min-w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
				<tr>
					<th class="px-4 py-3">Product</th>
					<th class="px-4 py-3">SKU</th>
					{#each shown as w (w.id)}<th class="px-4 py-3 text-right">{w.name}</th>{/each}
					{#if !warehouse}<th class="px-4 py-3 text-right">Total</th>{/if}
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-50">
				{#each rows as r (r.product_id)}
					<tr class="cursor-pointer hover:bg-slate-50" data-stock-row={r.name} onclick={() => show(r)}>
						<td class="px-4 py-2.5 font-medium text-slate-900">{r.name}</td>
						<td class="px-4 py-2.5 text-slate-500">{r.sku ?? ''}</td>
						{#each shown as w (w.id)}
							{@const n = Number(r.per[w.id] ?? 0)}
							<td class="px-4 py-2.5 text-right tabular-nums {level_(n)}" data-wh={w.name}>{qty(n)}</td>
						{/each}
						{#if !warehouse}<td class="px-4 py-2.5 text-right font-semibold tabular-nums {level_(Number(r.total))}" data-total>{qty(r.total)} <span class="font-normal text-slate-400">{r.unit}</span></td>{/if}
					</tr>
				{:else}
					<tr><td colspan={shown.length + 3} class="px-4 py-14 text-center text-slate-500">{q || level !== 'all' ? 'Nothing matches.' : 'No product has its stock counted yet. Receive goods, or set “In stock” on a product.'}</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>

<Modal bind:open={historyOpen} title={history?.name ?? ''}>
	{#if history}
		<div class="mb-4 flex flex-wrap gap-2">
			{#each history.warehouses as w (w.id)}
				<span class="rounded-lg bg-slate-50 px-3 py-1.5 text-sm"><span class="text-slate-500">{w.name}</span> <span class="font-medium tabular-nums">{qty(w.quantity)}</span></span>
			{/each}
			<span class="rounded-lg bg-slate-900 px-3 py-1.5 text-sm text-white">Total <span class="font-medium tabular-nums">{qty(history.total)} {history.unit}</span></span>
		</div>
		<div class="max-h-96 overflow-y-auto">
			<table class="w-full text-sm" aria-label="Movements">
				<thead class="text-left text-xs text-slate-500"><tr><th class="py-2">When</th><th>What</th><th>Warehouse</th><th class="text-right">Change</th><th class="text-right">Then</th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each history.movements as m (m.id)}
						<tr data-movement={m.reason}>
							<td class="py-2 pr-3 whitespace-nowrap text-slate-500">{formatDateTime(m.created_at)}</td>
							<td class="pr-3">
								{#if m.document_id}<a class="text-brand-600 hover:underline" href="/inventory/documents/{m.document_id}">{m.document_number}</a>{#if m.reason === 'document_cancelled'} cancelled{/if}
								{:else if m.item_id}<a class="text-brand-600 hover:underline" href="/crm/{m.item_code}/{m.item_id}">{REASON[m.reason] ?? m.reason} #{m.item_number}</a>
								{:else}{REASON[m.reason] ?? m.reason}{/if}
								{#if m.user_name}<span class="text-slate-400"> · {m.user_name}</span>{/if}
							</td>
							<td class="pr-3 text-slate-600">{m.warehouse}</td>
							<td class="text-right tabular-nums {Number(m.delta) < 0 ? 'text-red-700' : 'text-emerald-700'}">{Number(m.delta) > 0 ? '+' : ''}{qty(m.delta)}</td>
							<td class="text-right text-slate-600 tabular-nums">{qty(m.balance)}</td>
						</tr>
					{:else}
						<tr><td colspan="5" class="py-6 text-center text-slate-500">No movements yet.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (historyOpen = false)}>Close</button>
	{/snippet}
</Modal>
