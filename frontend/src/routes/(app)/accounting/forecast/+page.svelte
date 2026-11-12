<script lang="ts">
	// Cash forecast: the money expected in and out week by week — open invoices (on their due
	// date plus how late the client usually pays), deals by their chance, bills — and the bank
	// balance it leads to.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Item = { kind: string; dir: string; id: string; label: string; party: string | null; date: string; amount: string; full: string; chance: string | null; link: string };
	type Week = { start: string; invoices_in: string; deals_in: string; bills_out: string; net: string; balance: string; best: string; worst: string; items: Item[] };
	type Forecast = {
		currency: string;
		opening: string;
		weeks: Week[];
		later: string;
		undated_deals: { id: string; label: string; party: string | null; amount: string; chance: string; link: string }[];
		first_shortfall: string | null;
		habits: { late_days: number; term_days: number; from_history: boolean };
	};
	let weeks = $state(13);
	let f = $state<Forecast | null>(null);
	let open = $state<string | null>(null);
	const money = (v: string | number) => Number(v).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
	const KIND: Record<string, string> = { invoice: 'Invoice', crm_invoice: 'Invoice (not in the books)', deal: 'Deal', bill: 'Bill' };
	async function load() {
		try {
			f = await api.get<Forecast>(`/accounting/cash-forecast?weeks=${weeks}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	const lowest = $derived(f ? f.weeks.reduce((m, w) => Math.min(m, Number(w.balance)), Number(f.opening)) : 0);
</script>

<svelte:head><title>Cash forecast · NERP</title></svelte:head>

<div class="max-w-6xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Cash forecast</h1>
		<select class="input ml-auto w-36" aria-label="Weeks" bind:value={weeks} onchange={load}>
			{#each [4, 8, 13, 26, 52] as w (w)}<option value={w}>{w} weeks</option>{/each}
		</select>
	</div>
	<p class="text-sm text-slate-500">
		Open invoices are expected on their due date plus how late that client usually pays; open deals count by their chance of
		winning (their probability, or how often their pipeline wins) on the close date plus how long the client takes to pay;
		bills go out on their due date.
	</p>

	{#if f}
		<div class="grid gap-3 sm:grid-cols-4">
			<div class="card p-4"><p class="text-xs text-slate-500">In the bank today</p><p class="text-2xl font-semibold tabular-nums" data-opening>{money(f.opening)} <span class="text-sm font-normal text-slate-400">{f.currency}</span></p></div>
			<div class="card p-4"><p class="text-xs text-slate-500">After {weeks} weeks (likely)</p><p class="text-2xl font-semibold tabular-nums">{money(f.weeks.at(-1)?.balance ?? f.opening)}</p></div>
			<div class="card p-4"><p class="text-xs text-slate-500">Lowest point</p><p class="text-2xl font-semibold tabular-nums {lowest < 0 ? 'text-red-700' : ''}">{money(lowest)}</p></div>
			<div class="card p-4" data-shortfall>
				<p class="text-xs text-slate-500">Shortfall</p>
				{#if f.first_shortfall}<p class="text-lg font-semibold text-red-700">From the week of {formatDate(f.first_shortfall)}</p>
				{:else}<p class="text-lg font-semibold text-emerald-700">None expected</p>{/if}
			</div>
		</div>
		{#if !f.habits.from_history}<p class="text-xs text-amber-700">No paid invoices yet to learn from: clients are expected to pay on time, 30 days after invoicing.</p>{/if}

		<section class="card overflow-x-auto" aria-label="Weeks">
			<table class="min-w-full text-sm">
				<thead class="text-left text-xs text-slate-500">
					<tr class="border-b border-slate-100">
						<th class="px-4 py-2">Week of</th><th class="px-3 text-right">Invoices in</th><th class="px-3 text-right">Deals in</th><th class="px-3 text-right">Bills out</th><th class="px-3 text-right">Net</th>
						<th class="px-3 text-right">Balance</th><th class="px-3 text-right text-slate-400">At best</th><th class="px-4 text-right text-slate-400">Invoices only</th>
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-50">
					{#each f.weeks as w (w.start)}
						<tr class="cursor-pointer hover:bg-slate-50" onclick={() => (open = open === w.start ? null : w.start)} data-week={w.start}>
							<td class="px-4 py-2">{formatDate(w.start)} {#if w.items.length}<span class="text-xs text-slate-400">· {w.items.length}</span>{/if}</td>
							<td class="px-3 text-right tabular-nums">{Number(w.invoices_in) ? money(w.invoices_in) : ''}</td>
							<td class="px-3 text-right tabular-nums">{Number(w.deals_in) ? money(w.deals_in) : ''}</td>
							<td class="px-3 text-right tabular-nums text-slate-600">{Number(w.bills_out) ? '−' + money(w.bills_out) : ''}</td>
							<td class="px-3 text-right tabular-nums">{money(w.net)}</td>
							<td class="px-3 text-right font-semibold tabular-nums {Number(w.balance) < 0 ? 'text-red-700' : ''}" data-balance>{money(w.balance)}</td>
							<td class="px-3 text-right tabular-nums text-slate-400">{money(w.best)}</td>
							<td class="px-4 text-right tabular-nums text-slate-400">{money(w.worst)}</td>
						</tr>
						{#if open === w.start}
							<tr><td colspan="8" class="bg-slate-50 px-6 py-2">
								<ul class="space-y-1 text-xs" data-week-items>
									{#each w.items as it (it.kind + it.id)}
										<li class="flex gap-2">
											<span class="w-24 text-slate-500">{formatDate(it.date)}</span>
											<span class="flex-1"><a class="text-brand-700 hover:underline" href={it.link}>{it.label}</a>{#if it.party}<span class="text-slate-500"> · {it.party}</span>{/if} <span class="text-slate-400">({KIND[it.kind] ?? it.kind}{it.chance ? `, ${it.chance}% of ${money(it.full)}` : ''})</span></span>
											<span class="tabular-nums {it.dir === 'out' ? 'text-slate-600' : ''}">{it.dir === 'out' ? '−' : ''}{money(it.amount)}</span>
										</li>
									{:else}<li class="text-slate-400">Nothing expected.</li>{/each}
								</ul>
							</td></tr>
						{/if}
					{/each}
				</tbody>
			</table>
		</section>
		{#if Number(f.later)}<p class="text-sm text-slate-500">Later than that: {money(f.later)} more expected.</p>{/if}

		{#if f.undated_deals.length}
			<section class="card" aria-label="Deals without a close date" data-undated>
				<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">Deals without a close date</h2>
				<ul class="divide-y divide-slate-50 text-sm">
					{#each f.undated_deals as d (d.id)}
						<li class="flex gap-2 px-5 py-2"><a class="flex-1 text-brand-700 hover:underline" href={d.link}>{d.label}</a><span class="text-slate-500">{d.chance}% of</span><span class="tabular-nums">{money(d.amount)}</span></li>
					{/each}
				</ul>
				<p class="px-5 pb-3 text-xs text-slate-500">Give them a close date to see them in the forecast.</p>
			</section>
		{/if}
	{/if}
</div>
