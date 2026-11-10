<script lang="ts">
	// One stock document: edit while a draft, then post it (applies it to stock) or cancel a
	// posted one (undoes it).
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import CrmPicker from '#lib/components/crm/CrmPicker.svelte';
	import { crm, money } from '#lib/crm.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { KIND, STATUS, qty, type DocKind, type Warehouse } from '#lib/inventory.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Line = { product_id: string; name: string; sku: string | null; unit: string; quantity: number | string; price: number | string | null; in_stock: string | null; applied?: string | null };
	type Doc = {
		id: string;
		kind: DocKind;
		number: string;
		status: 'draft' | 'posted' | 'cancelled';
		doc_date: string;
		from_warehouse: string | null;
		to_warehouse: string | null;
		supplier_id: string | null;
		supplier_name: string | null;
		reference: string | null;
		currency: string | null;
		comment: string | null;
		created_by_name: string | null;
		posted_at: string | null;
		items: Line[];
	};
	let doc = $state<Doc | null>(null);
	let missing = $state(false);
	let warehouses = $state<Warehouse[]>([]);
	let dirty = $state(false);
	let busy = $state(false);

	$effect(() => {
		const id = page.params.id;
		doc = null;
		crm.load().catch(() => {});
		Promise.all([api.get<Doc>(`/inventory/documents/${id}`), api.get<{ items: Warehouse[] }>('/inventory/warehouses')])
			.then(([d, w]) => ((doc = d), (warehouses = w.items), (dirty = false)))
			.catch(() => (missing = true));
	});
	const editable = $derived(doc?.status === 'draft' && (session.can(PERM.INVENTORY) || session.can(PERM.CRM_ADMIN)));
	const active = $derived(warehouses.filter((w) => w.is_active));
	const cost = $derived(doc?.kind === 'receipt' ? doc.items.reduce((a, l) => a + Number(l.quantity || 0) * Number(l.price || 0), 0) : 0);

	// Adding products.
	let term = $state('');
	let found = $state<{ id: string; name: string; sku: string | null; unit: string; price: string }[]>([]);
	let timer: ReturnType<typeof setTimeout> | undefined;
	function find() {
		clearTimeout(timer);
		if (!term.trim()) return void (found = []);
		timer = setTimeout(async () => {
			found = (await api.get<{ items: typeof found }>('/crm/products' + query({ q: term.trim(), limit: 10 }))).items.filter((p) => !doc?.items.some((l) => l.product_id === p.id));
		}, 200);
	}
	function add(p: (typeof found)[number]) {
		doc!.items.push({ product_id: p.id, name: p.name, sku: p.sku, unit: p.unit, quantity: doc!.kind === 'count' ? 0 : 1, price: doc!.kind === 'receipt' ? null : null, in_stock: null });
		term = '';
		found = [];
		dirty = true;
	}

	function body() {
		const d = doc!;
		return {
			from_warehouse: d.from_warehouse,
			to_warehouse: d.to_warehouse,
			supplier_id: d.supplier_id,
			reference: d.reference,
			currency: d.currency,
			comment: d.comment,
			doc_date: d.doc_date,
			items: d.items.map((l) => ({ product_id: l.product_id, quantity: Number(l.quantity), price: l.price === null || l.price === '' ? null : Number(l.price) }))
		};
	}
	async function save(): Promise<boolean> {
		busy = true;
		try {
			doc = await api.patch<Doc>(`/inventory/documents/${doc!.id}`, body());
			dirty = false;
			return true;
		} catch (e) {
			toasts.error(e);
			return false;
		} finally {
			busy = false;
		}
	}
	async function post() {
		if (dirty && !(await save())) return;
		busy = true;
		try {
			doc = await api.post<Doc>(`/inventory/documents/${doc!.id}/post`, {});
			toasts.success('Posted: stock updated');
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	async function cancel() {
		if (!confirm(`Cancel ${doc!.number}? Its changes to stock are undone.`)) return;
		busy = true;
		try {
			doc = await api.post<Doc>(`/inventory/documents/${doc!.id}/cancel`, {});
			toasts.success('Cancelled: stock restored');
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	async function remove() {
		if (!confirm(`Delete the draft ${doc!.number}?`)) return;
		await api.del(`/inventory/documents/${doc!.id}`);
		goto('/inventory/documents');
	}
	const touch = () => (dirty = true);
</script>

<svelte:head><title>{doc ? `${doc.number} · ${KIND[doc.kind].name}` : 'Stock document'} · NERP</title></svelte:head>

<a href="/inventory/documents" class="mb-4 inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="list" class="size-4" /> Stock documents</a>

{#if missing}
	<div class="card p-10 text-center text-slate-500">This document doesn't exist.</div>
{:else if doc}
	<div class="max-w-5xl space-y-5">
		<div class="card p-6">
			<div class="flex flex-wrap items-start gap-3">
				<div class="min-w-0 flex-1">
					<p class="text-sm text-slate-500">{KIND[doc.kind].name}</p>
					<h1 class="flex items-center gap-3 text-2xl font-semibold text-slate-900">{doc.number} <span class="badge text-sm {STATUS[doc.status][1]}" data-status={doc.status}>{STATUS[doc.status][0]}</span></h1>
					<p class="mt-1 text-sm text-slate-500">{KIND[doc.kind].hint}{doc.posted_at ? ` Posted ${formatDateTime(doc.posted_at)}.` : ''}</p>
				</div>
				<div class="flex gap-2">
					{#if editable}
						<button class="btn btn-ghost hover:text-red-600" onclick={remove}>Delete draft</button>
						<button class="btn btn-secondary" disabled={busy || !dirty} onclick={save}>Save draft</button>
						<button class="btn btn-primary" disabled={busy} onclick={post}>Post</button>
					{:else if doc.status === 'posted' && (session.can(PERM.INVENTORY) || session.can(PERM.CRM_ADMIN))}
						<button class="btn btn-secondary" disabled={busy} onclick={cancel}>Cancel document</button>
					{/if}
				</div>
			</div>

			<div class="mt-5 grid gap-4 sm:grid-cols-3">
				{#if doc.kind === 'writeoff' || doc.kind === 'transfer'}
					<div>
						<label class="label" for="d-from">From</label>
						<select id="d-from" class="input" disabled={!editable} bind:value={doc.from_warehouse} onchange={touch}>{#each editable ? active : warehouses as w (w.id)}<option value={w.id}>{w.name}</option>{/each}</select>
					</div>
				{/if}
				{#if doc.kind !== 'writeoff'}
					<div>
						<label class="label" for="d-to">{doc.kind === 'count' ? 'Warehouse counted' : doc.kind === 'transfer' ? 'To' : 'Into'}</label>
						<select id="d-to" class="input" disabled={!editable} bind:value={doc.to_warehouse} onchange={touch}>
							{#if doc.kind === 'transfer'}<option value={null}>Choose…</option>{/if}
							{#each editable ? active : warehouses as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
						</select>
					</div>
				{/if}
				<div><label class="label" for="d-date">Date</label><input id="d-date" class="input" type="date" disabled={!editable} bind:value={doc.doc_date} onchange={touch} /></div>
				{#if doc.kind === 'receipt'}
					<div>
						<span class="label">Supplier</span>
						{#if doc.supplier_id}
							<div class="flex items-center gap-2"><a class="text-brand-600 hover:underline" href="/crm/company/{doc.supplier_id}">{doc.supplier_name}</a>{#if editable}<button class="btn btn-ghost p-1" aria-label="Remove supplier" onclick={() => ((doc!.supplier_id = null), (doc!.supplier_name = null), touch())}><Icon name="x" class="size-4" /></button>{/if}</div>
						{:else if editable}
							<CrmPicker types={['company']} placeholder="Find a company…" onpick={(h) => ((doc!.supplier_id = h.id), (doc!.supplier_name = h.title), touch())} />
						{:else}<p class="text-slate-400">—</p>{/if}
					</div>
					<div><label class="label" for="d-ref">Supplier's invoice no.</label><input id="d-ref" class="input" disabled={!editable} bind:value={doc.reference} oninput={touch} /></div>
					<div>
						<label class="label" for="d-cur">Currency</label>
						<select id="d-cur" class="input" disabled={!editable} bind:value={doc.currency} onchange={touch}>{#each crm.config?.currencies ?? [] as c (c.code)}<option>{c.code}</option>{/each}</select>
					</div>
				{/if}
				<div class="sm:col-span-3"><label class="label" for="d-com">Comment</label><input id="d-com" class="input" disabled={!editable} placeholder={doc.kind === 'writeoff' ? 'Why, e.g. damaged in transit' : ''} bind:value={doc.comment} oninput={touch} /></div>
			</div>
		</div>

		<section class="card" aria-labelledby="lines-h">
			<div class="flex items-center gap-3 border-b border-slate-100 px-5 py-3">
				<h2 id="lines-h" class="font-medium text-slate-900">Products</h2>
				{#if doc.kind === 'receipt' && cost}<span class="ml-auto text-sm text-slate-600">Cost <span class="font-semibold text-slate-900 tabular-nums" data-cost>{money(cost, doc.currency)}</span></span>{/if}
			</div>
			<table class="w-full text-sm">
				<thead class="text-left text-xs text-slate-500">
					<tr>
						<th class="px-5 py-2">Product</th>
						<th class="px-3 py-2 text-right">{doc.kind === 'count' ? 'Expected' : 'In stock'}</th>
						<th class="px-3 py-2 text-right">{doc.kind === 'count' ? 'Found' : 'Quantity'}</th>
						{#if doc.kind === 'count' && doc.status !== 'draft'}<th class="px-3 py-2 text-right">Difference</th>{/if}
						{#if doc.kind === 'receipt'}<th class="px-3 py-2 text-right">Price per unit</th><th class="px-3 py-2 text-right">Amount</th>{/if}
						<th class="w-10"></th>
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-50">
					{#each doc.items as l, i (l.product_id)}
						<tr data-line={l.name}>
							<td class="px-5 py-2"><span class="font-medium text-slate-900">{l.name}</span>{#if l.sku}<span class="ml-2 text-xs text-slate-400">{l.sku}</span>{/if}</td>
							<td class="px-3 py-2 text-right text-slate-500 tabular-nums">{l.in_stock === null ? '—' : qty(l.in_stock)}</td>
							<td class="px-3 py-2 text-right">
								{#if editable}<input class="input ml-auto w-24 text-right" type="number" min="0" step="any" aria-label="Quantity of {l.name}" bind:value={l.quantity} oninput={touch} />{:else}<span class="tabular-nums">{qty(l.quantity)}</span>{/if}
								<span class="text-xs text-slate-400">{l.unit}</span>
							</td>
							{#if doc.kind === 'count' && doc.status !== 'draft'}<td class="px-3 py-2 text-right tabular-nums">{l.applied ? (Number(l.applied) > 0 ? '+' : '') + qty(l.applied) : '0'}</td>{/if}
							{#if doc.kind === 'receipt'}
								<td class="px-3 py-2 text-right">{#if editable}<input class="input ml-auto w-28 text-right" type="number" min="0" step="0.01" aria-label="Price of {l.name}" bind:value={l.price} oninput={touch} />{:else}<span class="tabular-nums">{l.price === null ? '—' : money(l.price, doc.currency)}</span>{/if}</td>
								<td class="px-3 py-2 text-right tabular-nums">{l.price === null || l.price === '' ? '' : money(Number(l.quantity) * Number(l.price), doc.currency)}</td>
							{/if}
							<td class="pr-3">{#if editable}<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Remove {l.name}" onclick={() => (doc!.items.splice(i, 1), touch())}><Icon name="trash" class="size-4" /></button>{/if}</td>
						</tr>
					{:else}
						<tr><td colspan="6" class="px-5 py-8 text-center text-slate-500">No products yet.</td></tr>
					{/each}
				</tbody>
			</table>
			{#if editable}
				<div class="relative border-t border-slate-100 px-5 py-3">
					<input class="input max-w-sm" type="search" placeholder="Add a product…" aria-label="Add a product" bind:value={term} oninput={find} />
					{#if found.length}
						<ul class="absolute z-10 mt-1 w-full max-w-sm overflow-hidden rounded-xl border border-slate-200 bg-white shadow-g2">
							{#each found as p (p.id)}
								<li><button class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-slate-50" onclick={() => add(p)}><span class="flex-1">{p.name}</span><span class="text-xs text-slate-400">{p.sku ?? ''}</span></button></li>
							{/each}
						</ul>
					{/if}
				</div>
			{/if}
		</section>
		{#if doc.created_by_name}<p class="text-xs text-slate-400">Made by {doc.created_by_name}</p>{/if}
	</div>
{:else}
	<div class="card p-10 text-center text-slate-400">Loading…</div>
{/if}
