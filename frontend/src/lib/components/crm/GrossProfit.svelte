<script lang="ts">
	// Gross profit and margin of the records won in the period: net sales of their product rows
	// minus those rows' costs, by manager, product and month, and the most profitable records.
	// Rows without a known cost are left out of profit and margin and shown apart.
	import { api, query } from '#lib/api.ts';
	import { money } from '#lib/crm.svelte.ts';

	type Pl = { revenue: string; cost: string; profit: string; margin: string | null; uncosted_revenue: string; deals: number };
	type Report = {
		base_currency: string;
		total: Pl;
		without_products: number;
		capped: boolean;
		by_manager: (Pl & { user_id: string | null; name: string })[];
		by_product: (Pl & { product_id: string | null; name: string; quantity: string })[];
		by_month: (Pl & { month: string })[];
		deals: (Pl & { id: string; number: number; title: string; assigned_name: string | null })[];
	};

	let { typeCode, from, to, category, users }: { typeCode: string; from: string; to: string; category: string; users: string } = $props();
	let report = $state<Report | null>(null);
	let error = $state<string | null>(null);
	let view = $state<'manager' | 'product' | 'month' | 'deal'>('manager');
	let seq = 0;

	$effect(() => {
		const q = query({ from, to, category_id: category || undefined, users: users || undefined });
		const n = ++seq;
		api.get<Report>(`/crm/analytics/${typeCode}/profit${q}`)
			.then((r) => n === seq && ((report = r), (error = null)))
			.catch((e) => n === seq && (error = e instanceof Error ? e.message : String(e)));
	});

	const cur = $derived(report?.base_currency ?? '');
	const pct = (m: string | null) => (m === null ? '—' : `${Number(m).toFixed(1)}%`);
	const rows = $derived.by(() => {
		if (!report) return [];
		switch (view) {
			case 'manager': return report.by_manager.map((r) => ({ key: r.user_id ?? 'none', label: r.name || 'Nobody', pl: r, extra: `${r.deals}` }));
			case 'product': return report.by_product.map((r) => ({ key: r.product_id ?? r.name, label: r.name, pl: r, extra: Number(r.quantity).toLocaleString() }));
			case 'month': return report.by_month.map((r) => ({ key: r.month, label: new Date(`${r.month}-01T00:00:00`).toLocaleDateString(undefined, { month: 'long', year: 'numeric' }), pl: r, extra: `${r.deals}` }));
			default: return report.deals.map((r) => ({ key: r.id, label: `#${r.number} ${r.title}`, pl: r, extra: r.assigned_name ?? '—', href: `/crm/${typeCode}/${r.id}` }));
		}
	});
	const extraHead = $derived(view === 'product' ? 'Sold' : view === 'deal' ? 'Responsible' : 'Won');
</script>

<section class="card p-5" aria-labelledby="gp-h" data-gross-profit-report>
	<div class="flex flex-wrap items-center gap-3">
		<h2 id="gp-h" class="font-medium text-slate-900">Gross profit</h2>
		<div class="ml-auto flex rounded-full bg-slate-100 p-0.5 text-xs" role="tablist" aria-label="Gross profit by">
			{#each [['manager', 'Managers'], ['product', 'Products'], ['month', 'Months'], ['deal', 'Records']] as [k, l] (k)}
				<button role="tab" aria-selected={view === k} class="rounded-full px-3 py-1 {view === k ? 'bg-white font-medium text-slate-900 shadow-sm' : 'text-slate-600'}" onclick={() => (view = k as typeof view)}>{l}</button>
			{/each}
		</div>
	</div>
	{#if error}
		<p class="py-6 text-sm text-red-700">{error}</p>
	{:else if report}
		{@const t = report.total}
		<dl class="mt-4 grid grid-cols-2 gap-4 sm:grid-cols-4">
			<div><dt class="text-xs text-slate-500">Net sales</dt><dd class="text-lg font-semibold text-slate-900 tabular-nums">{money(t.revenue, cur)}</dd></div>
			<div><dt class="text-xs text-slate-500">Cost</dt><dd class="text-lg font-semibold text-slate-900 tabular-nums">{money(t.cost, cur)}</dd></div>
			<div><dt class="text-xs text-slate-500">Gross profit</dt><dd class="text-lg font-semibold text-slate-900 tabular-nums" data-total-profit>{money(t.profit, cur)}</dd></div>
			<div><dt class="text-xs text-slate-500">Margin</dt><dd class="text-lg font-semibold text-slate-900 tabular-nums" data-total-margin>{pct(t.margin)}</dd></div>
		</dl>
		{#if Number(t.uncosted_revenue) || report.without_products}
			<p class="mt-2 text-xs text-amber-800">
				Left out:
				{#if Number(t.uncosted_revenue)}{money(t.uncosted_revenue, cur)} of sales whose products have no cost price{/if}{#if Number(t.uncosted_revenue) && report.without_products}; {/if}{#if report.without_products}{report.without_products} won without product rows{/if}.
			</p>
		{/if}
		{#if rows.length}
			<div class="mt-4 overflow-x-auto">
				<table class="min-w-full text-sm">
					<thead class="text-left text-xs text-slate-500">
						<tr><th class="py-2 pr-3 font-medium"></th><th class="px-3 py-2 text-right font-medium">{extraHead}</th><th class="px-3 py-2 text-right font-medium">Net sales</th><th class="px-3 py-2 text-right font-medium">Cost</th><th class="px-3 py-2 text-right font-medium">Gross profit</th><th class="py-2 pl-3 text-right font-medium">Margin</th></tr>
					</thead>
					<tbody class="divide-y divide-slate-50 tabular-nums">
						{#each rows as r (r.key)}
							<tr>
								<td class="max-w-72 truncate py-2 pr-3 text-slate-800">{#if 'href' in r && r.href}<a class="hover:underline" href={r.href}>{r.label}</a>{:else}{r.label}{/if}</td>
								<td class="px-3 py-2 text-right text-slate-600">{r.extra}</td>
								<td class="px-3 py-2 text-right">{money(r.pl.revenue, cur)}</td>
								<td class="px-3 py-2 text-right">{money(r.pl.cost, cur)}</td>
								<td class="px-3 py-2 text-right font-medium {Number(r.pl.profit) < 0 ? 'text-red-700' : 'text-slate-900'}">{money(r.pl.profit, cur)}</td>
								<td class="py-2 pl-3 text-right">{pct(r.pl.margin)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{:else}
			<p class="py-6 text-center text-sm text-slate-500">Nothing won with products in this period.</p>
		{/if}
		{#if report.capped}<p class="mt-2 text-xs text-slate-400">Only the first 50,000 records won in the period are counted; choose a shorter period.</p>{/if}
	{:else}
		<p class="py-6 text-sm text-slate-400">Loading…</p>
	{/if}
</section>
