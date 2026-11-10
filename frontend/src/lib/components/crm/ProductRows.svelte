<script lang="ts">
	// Product rows on a deal or lead. Totals shown here are a live preview; the server
	// recomputes them with exact decimals when the rows are saved.
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import { money } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';

	type Row = {
		product_id: string | null;
		name: string;
		price: string;
		quantity: string;
		unit: string;
		discount_kind: 'percent' | 'amount';
		discount_value: string;
		tax_rate: string | null;
		tax_included: boolean;
	};
	type Product = { id: string; name: string; sku: string | null; price: string; currency: string; tax_rate: string | null; tax_included: boolean; unit: string };

	let { itemId, currency, canEdit, onchanged }: { itemId: string; currency: string; canEdit: boolean; onchanged?: () => void } = $props();

	let rows = $state<Row[]>([]);
	let editing = $state(false);
	let saving = $state(false);
	let search = $state('');
	let found = $state<Product[]>([]);

	const num = (s: string | null) => Number(s ?? 0) || 0;
	function line(r: Row) {
		const price = num(r.price);
		const disc = r.discount_kind === 'amount' ? Math.min(num(r.discount_value), price) : (price * Math.min(num(r.discount_value), 100)) / 100;
		const d = price - disc;
		const rate = num(r.tax_rate) / 100;
		const [net, tax] = r.tax_included ? [d / (1 + rate), d - d / (1 + rate)] : [d, d * rate];
		const q = num(r.quantity);
		return { net: net * q, tax: tax * q, total: (net + tax) * q, discount: disc * q };
	}
	const totals = $derived(
		rows.reduce((t, r) => {
			const l = line(r);
			return { subtotal: t.subtotal + l.net, tax: t.tax + l.tax, total: t.total + l.total, discount: t.discount + l.discount };
		}, { subtotal: 0, tax: 0, total: 0, discount: 0 })
	);

	async function load() {
		const d = await api.get<{ rows: { row: Row }[] }>(`/crm/items/${itemId}/products`);
		rows = d.rows.map((x) => ({ ...x.row, price: String(Number(x.row.price)), quantity: String(Number(x.row.quantity)), discount_value: String(Number(x.row.discount_value)), tax_rate: x.row.tax_rate === null ? null : String(Number(x.row.tax_rate)) }));
	}
	onMount(load);

	let timer: ReturnType<typeof setTimeout>;
	function find() {
		clearTimeout(timer);
		timer = setTimeout(async () => (found = (await api.get<{ items: Product[] }>('/crm/products' + query({ q: search, limit: 8 }))).items), 200);
	}
	// Price types (Wholesale…): rows from the catalog take that price instead of the base one.
	let priceTypes = $state<{ id: string; name: string }[]>([]);
	let priceType = $state('');
	onMount(() => {
		api.get<{ items: { id: string; name: string }[] }>('/crm/price-types').then((r) => (priceTypes = r.items)).catch(() => {});
	});
	async function pricesOf(ids: string[]): Promise<Record<string, string>> {
		if (!priceType || !ids.length) return {};
		return (await api.post<{ prices: Record<string, string> }>('/crm/prices', { price_type_id: priceType, product_ids: ids })).prices;
	}
	/** Changing the price type re-prices the catalog rows already there. */
	async function reprice() {
		const ids = rows.map((r) => r.product_id).filter((x): x is string => !!x);
		const prices = priceType ? await pricesOf(ids) : (await api.post<{ prices: Record<string, string> }>('/crm/prices', { price_type_id: null, product_ids: ids })).prices;
		for (const r of rows) if (r.product_id && prices[r.product_id] !== undefined) r.price = String(Number(prices[r.product_id]));
	}
	async function addProduct(p: Product | null) {
		const price = p ? ((await pricesOf([p.id]))[p.id] ?? p.price) : '0';
		rows = [
			...rows,
			p
				? { product_id: p.id, name: p.name, price: String(Number(price)), quantity: '1', unit: p.unit, discount_kind: 'percent', discount_value: '0', tax_rate: p.tax_rate === null ? null : String(Number(p.tax_rate)), tax_included: p.tax_included }
				: { product_id: null, name: '', price: '0', quantity: '1', unit: 'pcs', discount_kind: 'percent', discount_value: '0', tax_rate: null, tax_included: false }
		];
		search = '';
		found = [];
	}

	async function save() {
		saving = true;
		try {
			await api.put(`/crm/items/${itemId}/products`, {
				// Money travels as decimal strings so no float rounding creeps in.
				rows: rows.map((r) => ({
					...r,
					price: String(r.price ?? 0),
					quantity: String(r.quantity ?? 1),
					discount_value: String(r.discount_value ?? 0),
					tax_rate: r.tax_rate === '' || r.tax_rate === null || r.tax_rate === undefined ? null : String(r.tax_rate)
				}))
			});
			editing = false;
			await load();
			onchanged?.();
			toasts.success('Products saved');
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}
</script>

<section class="card">
	<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
		<h2 class="font-medium text-slate-900">Products</h2>
		{#if canEdit && !editing}<button class="btn btn-ghost text-sm" onclick={() => (editing = true)}><Icon name="edit" class="size-4" /> {rows.length ? 'Edit' : 'Add products'}</button>{/if}
	</div>
	{#if rows.length || editing}
		<div class="overflow-x-auto">
			<table class="min-w-full text-sm">
				<thead class="text-left text-xs font-medium text-slate-500">
					<tr>
						<th class="px-4 py-2">Product</th>
						<th class="px-2 py-2 text-right">Price</th>
						<th class="px-2 py-2 text-right">Qty</th>
						<th class="px-2 py-2 text-right">Discount</th>
						<th class="px-2 py-2 text-right">Tax %</th>
						<th class="px-4 py-2 text-right">Total</th>
						{#if editing}<th></th>{/if}
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-50">
					{#each rows as r, i (i)}
						<tr>
							{#if editing}
								<td class="px-4 py-1.5"><input class="input" bind:value={r.name} aria-label="Product name" /></td>
								<td class="px-2 py-1.5"><input class="input w-24 text-right" type="number" min="0" step="0.01" bind:value={r.price} aria-label="Price" /></td>
								<td class="px-2 py-1.5"><input class="input w-20 text-right" type="number" min="0.0001" step="any" bind:value={r.quantity} aria-label="Quantity" /></td>
								<td class="px-2 py-1.5">
									<div class="flex gap-1">
										<input class="input w-20 text-right" type="number" min="0" step="0.01" bind:value={r.discount_value} aria-label="Discount" />
										<select class="input w-16" bind:value={r.discount_kind} aria-label="Discount type"><option value="percent">%</option><option value="amount">{currency}</option></select>
									</div>
								</td>
								<td class="px-2 py-1.5">
									<div class="flex items-center gap-1">
										<input class="input w-20 text-right" type="number" min="0" max="100" step="0.01" bind:value={r.tax_rate} aria-label="Tax rate" />
										<label class="flex items-center gap-1 text-xs whitespace-nowrap text-slate-500" title="Price includes tax"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={r.tax_included} /> incl.</label>
									</div>
								</td>
							{:else}
								<td class="px-4 py-2 text-slate-800">{r.name}</td>
								<td class="px-2 py-2 text-right">{money(r.price, currency)}</td>
								<td class="px-2 py-2 text-right">{Number(r.quantity)} {r.unit}</td>
								<td class="px-2 py-2 text-right text-slate-500">{num(r.discount_value) ? (r.discount_kind === 'percent' ? `${Number(r.discount_value)}%` : money(r.discount_value, currency)) : '—'}</td>
								<td class="px-2 py-2 text-right text-slate-500">{r.tax_rate !== null ? `${Number(r.tax_rate)}%${r.tax_included ? ' incl.' : ''}` : '—'}</td>
							{/if}
							<td class="px-4 py-2 text-right font-medium whitespace-nowrap">{money(line(r).total, currency)}</td>
							{#if editing}<td class="pr-3"><button class="text-slate-300 hover:text-red-600" aria-label="Remove row" onclick={() => (rows = rows.filter((_, j) => j !== i))}><Icon name="x" class="size-4" /></button></td>{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
		{#if editing}
			<div class="relative flex flex-wrap gap-2 px-4 py-3">
				<div class="relative w-72">
					<input class="input" placeholder="Add from catalog…" bind:value={search} oninput={find} onblur={() => setTimeout(() => (found = []), 150)} />
					{#if found.length}
						<ul class="absolute z-20 mt-1 w-full rounded-xl bg-white py-1 shadow-g2">
							{#each found as p (p.id)}
								<li><button type="button" class="flex w-full justify-between gap-2 px-3 py-2 text-left text-sm hover:bg-slate-50" onmousedown={() => addProduct(p)}><span class="truncate">{p.name}</span><span class="text-slate-400">{money(p.price, p.currency)}</span></button></li>
							{/each}
						</ul>
					{/if}
				</div>
				<button class="btn btn-secondary" onclick={() => addProduct(null)}>Add custom row</button>
				{#if priceTypes.length}
					<label class="ml-auto flex items-center gap-2 text-sm text-slate-600">Prices
						<select class="input w-40" aria-label="Price type" bind:value={priceType} onchange={reprice}>
							<option value="">Base price</option>
							{#each priceTypes as t (t.id)}<option value={t.id}>{t.name}</option>{/each}
						</select>
					</label>
				{/if}
			</div>
		{/if}
		<dl class="ml-auto grid w-72 grid-cols-2 gap-y-1 px-5 py-3 text-sm">
			<dt class="text-slate-500">Subtotal</dt><dd class="text-right">{money(totals.subtotal, currency)}</dd>
			{#if totals.discount}<dt class="text-slate-500">Discount</dt><dd class="text-right">−{money(totals.discount, currency)}</dd>{/if}
			<dt class="text-slate-500">Tax</dt><dd class="text-right">{money(totals.tax, currency)}</dd>
			<dt class="font-medium text-slate-900">Total</dt><dd class="text-right font-semibold text-slate-900">{money(totals.total, currency)}</dd>
		</dl>
		{#if editing}
			<div class="flex gap-2 border-t border-slate-100 px-5 py-3">
				<button class="btn btn-primary" disabled={saving} onclick={save}>Save products</button>
				<button class="btn btn-secondary" onclick={() => ((editing = false), load())}>Cancel</button>
			</div>
		{/if}
	{:else}
		<p class="px-5 py-4 text-sm text-slate-400">No products yet.</p>
	{/if}
</section>
