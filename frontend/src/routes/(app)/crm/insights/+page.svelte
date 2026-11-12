<script lang="ts">
	// Insights from the company's own records: open deals by health (least healthy first, with
	// the reasons) and clients who stopped ordering.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { crm } from '#lib/crm.svelte.ts';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Deal = { id: string; title: string; stage: string | null; assigned_name: string | null; amount: string; score: number; grade: 'healthy' | 'at_risk' | 'poor'; reasons: { kind: string; penalty: number; text: string }[] };
	type Silent = { id: string; name: string; link: string; responsible_name: string | null; orders: number; usual_gap_days: number; last_order_at: string; days_since: number; expected_by: string; task_id: string | null };
	let tab = $state<'health' | 'silent'>('health');
	let deals = $state<Deal[]>([]);
	let counts = $state({ healthy: 0, at_risk: 0, poor: 0 });
	let grade = $state('');
	let category = $state('');
	let mine = $state(false);
	let silent = $state<Silent[]>([]);
	let tasksOn = $state(false);
	const isAdmin = $derived(session.can(PERM.CRM_ADMIN));
	const pipelines = $derived(crm.type('deal')?.categories ?? []);
	const LABEL = { healthy: 'Healthy', at_risk: 'At risk', poor: 'Poor' };
	const COLOR = { healthy: 'bg-emerald-50 text-emerald-800', at_risk: 'bg-amber-50 text-amber-800', poor: 'bg-red-50 text-red-800' };

	async function loadDeals() {
		const q = new URLSearchParams();
		if (grade) q.set('grade', grade);
		if (category) q.set('category_id', category);
		if (mine) q.set('mine', 'true');
		try {
			const r = await api.get<{ items: Deal[]; counts: typeof counts }>(`/crm/deal/health?${q}`);
			deals = r.items;
			counts = r.counts;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function loadSilent() {
		try {
			silent = (await api.get<{ items: Silent[] }>('/crm/silent-clients')).items;
			tasksOn = (await api.get<{ silent_tasks: boolean }>('/crm/insights/settings')).silent_tasks;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(async () => {
		await crm.load();
		await loadDeals();
	});
	async function toggleTasks() {
		try {
			tasksOn = (await api.put<{ silent_tasks: boolean }>('/crm/insights/settings', { silent_tasks: !tasksOn })).silent_tasks;
			toasts.success(tasksOn ? 'Check-in tasks are on' : 'Check-in tasks are off');
		} catch (e) {
			toasts.error(e);
		}
	}
	const money = (v: string) => Number(v).toLocaleString(undefined, { maximumFractionDigits: 0 });
</script>

<svelte:head><title>Insights · NERP</title></svelte:head>

<div class="max-w-6xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Insights</h1>
		<div class="flex gap-1" role="tablist">
			{#each [['health', 'Deal health'], ['silent', 'Silent clients']] as [k, l] (k)}
				<button role="tab" aria-selected={tab === k} class="rounded-full px-3 py-1.5 text-sm font-medium {tab === k ? 'bg-slate-900 text-white' : 'bg-white text-slate-700 ring-1 ring-slate-200 hover:bg-slate-100'}" onclick={() => ((tab = k as typeof tab), k === 'silent' ? loadSilent() : loadDeals())} data-tab={k}>{l}</button>
			{/each}
		</div>
	</div>

	{#if tab === 'health'}
		<p class="text-sm text-slate-500">Each open deal from 100 down, against how the pipeline's won deals went: contact, pace through the stage, people involved, unanswered emails, dates and next steps, and how the client pays.</p>
		<div class="flex flex-wrap items-center gap-2">
			{#each [['', `All (${counts.healthy + counts.at_risk + counts.poor})`], ['poor', `Poor (${counts.poor})`], ['at_risk', `At risk (${counts.at_risk})`], ['healthy', `Healthy (${counts.healthy})`]] as [g, l] (g)}
				<button class="rounded-full px-3 py-1 text-sm {grade === g ? 'bg-slate-900 text-white' : 'bg-white ring-1 ring-slate-200'}" onclick={() => ((grade = g), loadDeals())} data-grade={g || 'all'}>{l}</button>
			{/each}
			{#if pipelines.length > 1}<select class="input w-48" aria-label="Pipeline" bind:value={category} onchange={loadDeals}><option value="">All pipelines</option>{#each pipelines as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select>{/if}
			<label class="flex items-center gap-1 text-sm"><input type="checkbox" bind:checked={mine} onchange={loadDeals} /> Mine</label>
		</div>
		<section class="card divide-y divide-slate-50" aria-label="Deals">
			{#each deals as d (d.id)}
				<div class="flex gap-3 px-5 py-3 text-sm" data-deal={d.title}>
					<span class="w-10 shrink-0 text-right text-lg font-semibold tabular-nums">{d.score}</span>
					<div class="min-w-0 flex-1">
						<p><a class="font-medium text-brand-700 hover:underline" href="/crm/deal/{d.id}">{d.title}</a> <span class="rounded-full px-2 py-0.5 text-xs {COLOR[d.grade]}">{LABEL[d.grade]}</span></p>
						<p class="text-xs text-slate-500">{d.stage ?? ''}{d.assigned_name ? ` · ${d.assigned_name}` : ''}{Number(d.amount) ? ` · ${money(d.amount)}` : ''}</p>
						{#if d.reasons.length}<p class="mt-0.5 text-xs text-slate-600">{d.reasons.map((r) => r.text).join(' · ')}</p>{/if}
					</div>
				</div>
			{:else}<p class="px-5 py-4 text-sm text-slate-500">No open deals here.</p>{/each}
		</section>
	{:else}
		<p class="text-sm text-slate-500">Clients with at least three orders (won deals or completed shop orders) who have gone well past their usual gap between orders.</p>
		{#if isAdmin}
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" checked={tasksOn} onchange={toggleTasks} data-silent-tasks /> Make a task for the client's responsible person to check in (once each time a client goes quiet)</label>
		{/if}
		<section class="card overflow-x-auto" aria-label="Silent clients">
			<table class="min-w-full text-sm">
				<thead class="text-left text-xs text-slate-500"><tr class="border-b border-slate-100"><th class="px-5 py-2">Client</th><th>Usually orders every</th><th>Last order</th><th>Expected by</th><th>Responsible</th><th class="px-5"></th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each silent as c (c.id)}
						<tr data-silent={c.name}>
							<td class="px-5 py-2"><a class="text-brand-700 hover:underline" href={c.link}>{c.name}</a> <span class="text-xs text-slate-400">· {c.orders} orders</span></td>
							<td>{c.usual_gap_days} days</td>
							<td>{formatDate(c.last_order_at)} <span class="text-xs text-red-700">({c.days_since} days ago)</span></td>
							<td>{formatDate(c.expected_by)}</td>
							<td>{c.responsible_name ?? '—'}</td>
							<td class="px-5 text-right">{#if c.task_id}<a class="text-xs text-brand-700 hover:underline" href="/tasks/{c.task_id}">Check-in task</a>{/if}</td>
						</tr>
					{:else}<tr><td colspan="6" class="px-5 py-4 text-slate-500">No client has gone quiet.</td></tr>{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>
