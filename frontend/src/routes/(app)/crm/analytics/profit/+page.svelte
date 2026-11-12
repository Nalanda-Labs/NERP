<script lang="ts">
	// True profit: what clients, deals, products and salespeople really earned — revenue less
	// what the goods cost, the time support and tasks took (at each person's hourly cost) and
	// the cost of late payment — with the hourly costs and the financing rate behind it.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Modal from '#lib/components/Modal.svelte';

	type Fig = { revenue: string; discounts: string; goods: string; support: string; time: string; late: string; profit: string; margin: string | null; invoices: number };
	type Row = Fig & { id: string | null; name: string; link: string | null };
	type Report = { by: string; from: string; to: string; currency: string; total: Fig; items: Row[]; settings: { default_hourly: string; financing_rate: string } };
	type Settings = { default_hourly: string; financing_rate: string; people: { id: string; name: string; hourly: string | null }[] };

	const BY: [string, string][] = [['client', 'Clients'], ['deal', 'Deals'], ['product', 'Products'], ['person', 'Salespeople']];
	let by = $state('client');
	let to = $state(new Date().toISOString().slice(0, 10));
	let from = $state(new Date(Date.now() - 90 * 86400e3).toISOString().slice(0, 10));
	let report = $state<Report | null>(null);
	let loading = $state(false);
	let settingsOpen = $state(false);
	let settings = $state<Settings | null>(null);
	let saving = $state(false);
	const canSet = $derived(session.can(PERM.USERS_MANAGE) || session.can(PERM.CRM_ADMIN));

	const money = (v: string | number) => Number(v).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
	async function load() {
		loading = true;
		try {
			report = await api.get<Report>(`/crm/profit?by=${by}&from=${from}&to=${to}`);
		} catch (e) {
			toasts.error(e);
		} finally {
			loading = false;
		}
	}
	onMount(load);
	async function openSettings() {
		settings = await api.get<Settings>('/crm/profit/settings');
		settingsOpen = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		if (!settings) return;
		saving = true;
		try {
			await api.put('/crm/profit/settings', { default_hourly: settings.default_hourly, financing_rate: settings.financing_rate });
			settings = await api.put<Settings>('/crm/profit/costs', settings.people.map((p) => ({ user_id: p.id, hourly: p.hourly === '' ? null : p.hourly })));
			settingsOpen = false;
			toasts.success('Saved');
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
	const COLS: [keyof Fig, string][] = [['revenue', 'Revenue'], ['goods', 'Goods'], ['support', 'Support'], ['time', 'Task time'], ['late', 'Late payment'], ['profit', 'Profit']];
</script>

<svelte:head><title>True profit · NERP</title></svelte:head>

<div class="max-w-6xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<a class="btn btn-ghost px-2" href="/crm/analytics" aria-label="Sales analytics">‹</a>
		<h1 class="text-2xl font-semibold text-slate-900">True profit</h1>
		{#if loading}<span class="text-xs text-slate-400">Updating…</span>{/if}
		<button class="btn btn-secondary ml-auto" onclick={openSettings} data-profit-settings>Costs and rates</button>
	</div>
	<p class="text-sm text-slate-500">
		Invoices in the period (not lost), without tax and after discounts, less what the goods cost, the time support and tasks
		spent on each client (at each person's hourly cost) and what waiting for late payments costs.
	</p>

	<div class="card flex flex-wrap items-end gap-3 p-4">
		<div class="flex gap-1" role="tablist" aria-label="By">
			{#each BY as [k, label] (k)}
				<button role="tab" class="rounded-full px-3 py-1.5 text-sm font-medium {by === k ? 'bg-slate-900 text-white' : 'bg-white text-slate-700 ring-1 ring-slate-200 hover:bg-slate-100'}" aria-selected={by === k} onclick={() => ((by = k), load())} data-by={k}>{label}</button>
			{/each}
		</div>
		<div><label class="label" for="pf-from">From</label><input id="pf-from" class="input" type="date" bind:value={from} onchange={load} /></div>
		<div><label class="label" for="pf-to">To</label><input id="pf-to" class="input" type="date" bind:value={to} onchange={load} /></div>
	</div>

	{#if report}
		<div class="grid gap-3 sm:grid-cols-4">
			{#each [['Revenue', report.total.revenue], ['Costs', Number(report.total.goods) + Number(report.total.support) + Number(report.total.time) + Number(report.total.late)], ['Profit', report.total.profit]] as [label, v] (label)}
				<div class="card p-4" data-total={label}><p class="text-xs text-slate-500">{label}</p><p class="text-2xl font-semibold tabular-nums">{money(v)} <span class="text-sm font-normal text-slate-400">{report.currency}</span></p></div>
			{/each}
			<div class="card p-4"><p class="text-xs text-slate-500">Margin</p><p class="text-2xl font-semibold tabular-nums">{report.total.margin ?? '—'}{report.total.margin ? '%' : ''}</p><p class="text-xs text-slate-500">{report.total.invoices} invoices</p></div>
		</div>

		<section class="card overflow-x-auto" aria-label="Profit">
			<table class="min-w-full text-sm">
				<thead class="text-left text-xs text-slate-500">
					<tr class="border-b border-slate-100">
						<th class="px-4 py-2">{BY.find((b) => b[0] === by)?.[1]}</th>
						{#each COLS as [, label] (label)}<th class="px-3 text-right">{label}</th>{/each}
						<th class="px-4 text-right">Margin</th>
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-50">
					{#each report.items as r (r.id ?? r.name)}
						<tr data-row={r.name}>
							<td class="px-4 py-2">{#if r.link}<a class="text-brand-700 hover:underline" href={r.link}>{r.name}</a>{:else}{r.name}{/if}{#if Number(r.discounts) > 0}<span class="block text-xs text-slate-400">{money(r.discounts)} in discounts</span>{/if}</td>
							{#each COLS as [k] (k)}
								<td class="px-3 text-right tabular-nums {k === 'profit' ? (Number(r.profit) < 0 ? 'font-semibold text-red-700' : 'font-semibold') : k === 'revenue' ? '' : 'text-slate-600'}" data-col={k}>{k === 'revenue' || k === 'profit' ? '' : '−'}{money(r[k] ?? 0)}</td>
							{/each}
							<td class="px-4 text-right tabular-nums" data-col="margin">{r.margin ?? '—'}{r.margin ? '%' : ''}</td>
						</tr>
					{:else}<tr><td colspan="8" class="px-4 py-4 text-slate-500">No invoices in the period.</td></tr>{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<Modal bind:open={settingsOpen} title="Costs and rates">
	{#if settings}
		<form id="pf-settings" class="space-y-3 text-sm" onsubmit={save}>
			<div class="grid grid-cols-2 gap-3">
				<div><label class="label" for="pf-def">Default hourly cost</label><input id="pf-def" class="input" inputmode="decimal" bind:value={settings.default_hourly} disabled={!canSet} /></div>
				<div><label class="label" for="pf-rate">Financing rate (% a year)</label><input id="pf-rate" class="input" inputmode="decimal" bind:value={settings.financing_rate} disabled={!canSet} /></div>
			</div>
			<p class="text-xs text-slate-500">What an hour of someone's time costs the company (wages and overheads), in {report?.currency ?? 'the base currency'}. Leave a person empty to use the default.</p>
			<div class="max-h-72 divide-y divide-slate-50 overflow-y-auto">
				{#each settings.people as p (p.id)}
					<label class="flex items-center gap-2 py-1"><span class="flex-1">{p.name}</span><input class="input w-28 py-1" inputmode="decimal" placeholder="Default" bind:value={p.hourly} disabled={!canSet} aria-label="Hourly cost of {p.name}" /></label>
				{/each}
			</div>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (settingsOpen = false)}>Close</button>
		{#if canSet}<button class="btn btn-primary" form="pf-settings" disabled={saving}>{saving ? 'Saving…' : 'Save'}</button>{/if}
	{/snippet}
</Modal>
