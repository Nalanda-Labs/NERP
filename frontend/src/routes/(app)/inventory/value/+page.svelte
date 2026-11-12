<script lang="ts">
	// What the stock is worth and what it cost to sell: the cost method (FIFO or average), the
	// value per product, cost of goods sold for a period, and posting stock to the books.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { accounts as loadAccounts } from '#lib/accounting.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Settings = {
		cost_method: 'fifo' | 'average';
		post_to_books: boolean;
		inventory_account: string | null;
		cogs_account: string | null;
		receipt_account: string | null;
		adjust_account: string | null;
	};
	type Valuation = {
		method: string;
		currency: string;
		total: string;
		items: { product_id: string; name: string; sku: string | null; unit: string; quantity: string; unit_cost: string; value: string }[];
	};
	type Cogs = {
		from: string;
		to: string;
		sold: string;
		written_off: string;
		counted: string;
		items: { product_id: string; name: string; kind: string; quantity: string; cost: string }[];
	};
	let settings = $state<Settings | null>(null);
	let accountingReady = $state(false);
	let accounts = $state<{ id: string; code: string; name: string; subtype: string }[]>([]);
	let warehouses = $state<{ id: string; name: string; is_active: boolean }[]>([]);
	let warehouse = $state('');
	let value = $state<Valuation | null>(null);
	let cogs = $state<Cogs | null>(null);
	const today = new Date().toISOString().slice(0, 10);
	let from = $state(new Date(Date.now() - 30 * 86400e3).toISOString().slice(0, 10));
	let to = $state(today);
	let saving = $state(false);

	const money = (v: string | number) => Number(v).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
	async function loadValue() {
		value = await api.get<Valuation>(`/inventory/valuation${warehouse ? `?warehouse_id=${warehouse}` : ''}`);
	}
	async function loadCogs() {
		cogs = await api.get<Cogs>(`/inventory/cogs?from=${from}&to=${to}`);
	}
	onMount(async () => {
		try {
			const s = await api.get<{ settings: Settings; accounting_ready: boolean }>('/inventory/settings');
			settings = s.settings;
			accountingReady = s.accounting_ready;
			warehouses = (await api.get<{ items: typeof warehouses }>('/inventory/warehouses')).items;
			await Promise.all([loadValue(), loadCogs()]);
			if (accountingReady) accounts = (await loadAccounts().catch(() => [])) as typeof accounts;
		} catch (e) {
			toasts.error(e);
		}
	});
	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const s = await api.put<{ settings: Settings }>('/inventory/settings', settings!);
			settings = s.settings;
			await loadValue();
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
	const KIND: Record<string, string> = { sold: 'Sold', written_off: 'Written off', counted: 'Count differences' };
</script>

<svelte:head><title>Stock value · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<h1 class="text-2xl font-semibold text-slate-900">Stock value</h1>

	{#if settings}
		<form class="card space-y-3 p-5 text-sm" onsubmit={save} aria-label="Costing" data-costing>
			<h2 class="font-semibold text-slate-900">How stock is costed</h2>
			<div class="flex flex-wrap gap-4" role="radiogroup" aria-label="Cost method">
				<label class="flex items-start gap-2"><input type="radio" name="method" value="fifo" bind:group={settings.cost_method} data-method="fifo" /><span><strong>FIFO</strong> — goods leave at what the oldest goods on the shelf cost</span></label>
				<label class="flex items-start gap-2"><input type="radio" name="method" value="average" bind:group={settings.cost_method} data-method="average" /><span><strong>Average</strong> — goods leave at the warehouse's moving average cost</span></label>
			</div>
			<p class="text-xs text-slate-500">
				Receipts bring goods in at their purchase price, transfers keep their cost, returns come back at the cost they left
				with. The method can be changed at any time; it applies from then on. Deals' gross profit uses this cost for products
				whose stock is counted.
			</p>
			<label class="flex items-center gap-2"><input type="checkbox" bind:checked={settings.post_to_books} disabled={!accountingReady} data-post-books /> Post stock movements to the books</label>
			{#if !accountingReady}
				<p class="text-xs text-slate-500">Set up accounting first to post stock to the books.</p>
			{:else if settings.post_to_books}
				<div class="grid gap-3 sm:grid-cols-2">
					{#each [['inventory_account', 'Inventory (asset)'], ['cogs_account', 'Cost of goods sold'], ['receipt_account', 'Goods received (credited on receipts)'], ['adjust_account', 'Stock adjustments (write-offs, counts)']] as [k, label] (k)}
						<div>
							<label class="label" for="acc-{k}">{label}</label>
							<select id="acc-{k}" class="input" bind:value={settings[k as 'inventory_account']} required>
								<option value={null}>Choose…</option>
								{#each accounts as a (a.id)}<option value={a.id}>{a.code} {a.name}</option>{/each}
							</select>
						</div>
					{/each}
				</div>
				<p class="text-xs text-slate-500">
					Each movement is posted: goods sold debit cost of goods sold and credit inventory; receipts debit inventory and
					credit goods received (then match the supplier's bill against that account); write-offs and count differences go
					to stock adjustments. Transfers between warehouses aren't posted.
				</p>
			{/if}
			<button class="btn btn-primary" disabled={saving}>{saving ? 'Saving…' : 'Save'}</button>
		</form>
	{/if}

	{#if value}
		<section class="card" aria-label="Valuation" data-valuation>
			<div class="flex flex-wrap items-center gap-3 border-b border-slate-100 px-5 py-3.5">
				<h2 class="font-semibold text-slate-900">What the stock is worth</h2>
				<select class="input w-48" aria-label="Warehouse" bind:value={warehouse} onchange={loadValue}>
					<option value="">All warehouses</option>
					{#each warehouses.filter((w) => w.is_active) as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
				</select>
				<span class="ml-auto text-lg font-semibold tabular-nums" data-total>{money(value.total)} {value.currency}</span>
			</div>
			<table class="w-full text-sm">
				<thead class="text-left text-xs text-slate-500"><tr><th class="px-5 py-2">Product</th><th class="text-right">Quantity</th><th class="text-right">Unit cost</th><th class="px-5 text-right">Value</th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each value.items as r (r.product_id)}
						<tr data-product={r.name}>
							<td class="px-5 py-2">{r.name}{#if r.sku}<span class="text-xs text-slate-400"> · {r.sku}</span>{/if}</td>
							<td class="text-right tabular-nums">{Number(r.quantity).toLocaleString()} {r.unit}</td>
							<td class="text-right tabular-nums" data-unit-cost>{Number(r.unit_cost).toLocaleString(undefined, { maximumFractionDigits: 4 })}</td>
							<td class="px-5 text-right tabular-nums" data-value>{money(r.value)}</td>
						</tr>
					{:else}<tr><td colspan="4" class="px-5 py-4 text-slate-500">Nothing in stock.</td></tr>{/each}
				</tbody>
			</table>
		</section>
	{/if}

	{#if cogs}
		<section class="card" aria-label="Cost of goods sold" data-cogs>
			<div class="flex flex-wrap items-center gap-3 border-b border-slate-100 px-5 py-3.5">
				<h2 class="font-semibold text-slate-900">Cost of goods sold</h2>
				<input class="input w-40" type="date" bind:value={from} onchange={loadCogs} aria-label="From" />
				<span class="text-slate-400">to</span>
				<input class="input w-40" type="date" bind:value={to} onchange={loadCogs} aria-label="To" />
			</div>
			<div class="grid gap-3 p-5 sm:grid-cols-3">
				{#each [['sold', cogs.sold], ['written_off', cogs.written_off], ['counted', cogs.counted]] as [k, v] (k)}
					<div class="rounded-lg bg-slate-50 p-3" data-cogs-kind={k}><p class="text-xs text-slate-500">{KIND[k]}</p><p class="text-lg font-semibold tabular-nums">{money(v)}</p></div>
				{/each}
			</div>
			{#if cogs.items.length}
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500"><tr><th class="px-5 py-2">Product</th><th></th><th class="text-right">Quantity</th><th class="px-5 text-right">Cost</th></tr></thead>
					<tbody class="divide-y divide-slate-50">
						{#each cogs.items as r (`${r.product_id}-${r.kind}`)}
							<tr><td class="px-5 py-2">{r.name}</td><td class="text-slate-500">{KIND[r.kind] ?? r.kind}</td><td class="text-right tabular-nums">{Number(r.quantity).toLocaleString()}</td><td class="px-5 text-right tabular-nums">{money(r.cost)}</td></tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</section>
	{/if}
</div>
