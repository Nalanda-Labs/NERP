<script lang="ts">
	// Automation → Log: every robot and schedule run of a pipeline, with totals per status
	// and per robot, a search, and a retry for failed runs.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';

	let { categoryId }: { categoryId: string; typeCode?: string } = $props();

	type RunRow = {
		id: string; robot_id: string | null; schedule_id: string | null; robot_name: string; action: string | null; status: string; run_at: string;
		finished_at: string | null; wait_until: string | null; result: string | null; attempts: number; created_at: string;
		item_id: string; item_title: string; item_number: number; type_code: string;
	};
	type Source = { robot_id: string | null; schedule_id: string | null; name: string; total: number; done: number; failed: number; skipped: number; open: number };
	type Report = { counts: Record<string, number>; by_source: Source[]; items: RunRow[]; total: number };

	const STATUS: Record<string, [string, string]> = {
		done: ['Done', 'bg-emerald-50 text-emerald-800'],
		failed: ['Failed', 'bg-red-50 text-red-700'],
		skipped: ['Skipped', 'bg-slate-100 text-slate-600'],
		waiting: ['Waiting', 'bg-amber-50 text-amber-800'],
		scheduled: ['Scheduled', 'bg-sky-50 text-sky-800'],
		pending: ['After previous', 'bg-sky-50 text-sky-800'],
		running: ['Running', 'bg-sky-50 text-sky-800']
	};
	const PERIODS: [number, string][] = [[1, 'Last 24 hours'], [7, 'Last 7 days'], [30, 'Last 30 days'], [90, 'Last 90 days']];

	let days = $state(7);
	let status = $state('');
	let source = $state('');
	let q = $state('');
	let report = $state<Report | null>(null);
	let items = $state<RunRow[]>([]);
	let loading = $state(false);

	function query(offset: number) {
		const p = new URLSearchParams({ category_id: categoryId, from: new Date(Date.now() - days * 86400000).toISOString(), limit: '50', offset: String(offset) });
		if (status) p.set('status', status);
		if (source) p.set('source', source);
		if (q.trim()) p.set('q', q.trim());
		return `/crm/automation/report?${p}`;
	}
	async function load() {
		loading = true;
		try {
			report = await api.get<Report>(query(0));
			items = report.items;
		} catch (e) {
			toasts.error(e);
		} finally {
			loading = false;
		}
	}
	async function more() {
		const r = await api.get<Report>(query(items.length));
		items = [...items, ...r.items];
	}
	let timer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => {
		categoryId; days; status; source; q;
		clearTimeout(timer);
		timer = setTimeout(() => untrack(load), 250);
		return () => clearTimeout(timer);
	});

	async function retry(r: RunRow) {
		try {
			await api.post(`/crm/robot-runs/${r.id}/retry`, {});
			toasts.success(`“${r.robot_name}” runs again`);
			setTimeout(load, 1500);
		} catch (e) {
			toasts.error(e);
		}
	}

	const chips = $derived([
		['', 'All', Object.values(report?.counts ?? {}).reduce((a, b) => a + b, 0)],
		['failed', 'Failed', report?.counts.failed ?? 0],
		['open', 'Waiting', (report?.counts.pending ?? 0) + (report?.counts.scheduled ?? 0) + (report?.counts.waiting ?? 0) + (report?.counts.running ?? 0)],
		['done', 'Done', report?.counts.done ?? 0],
		['skipped', 'Skipped', report?.counts.skipped ?? 0]
	] as [string, string, number][]);
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2">
		<select class="input w-40" bind:value={days} aria-label="Period">{#each PERIODS as [d, l] (d)}<option value={d}>{l}</option>{/each}</select>
		<select class="input w-56" bind:value={source} aria-label="Robot or schedule">
			<option value="">All robots and schedules</option>
			{#each report?.by_source ?? [] as s ((s.robot_id ?? s.schedule_id) + s.name)}<option value={s.robot_id ?? s.schedule_id}>{s.name}{s.schedule_id ? ' (schedule)' : ''}</option>{/each}
		</select>
		<div class="relative w-64">
			<Icon name="search" class="pointer-events-none absolute top-2.5 left-3 size-4 text-slate-400" />
			<input class="input pl-9" type="search" placeholder="Record, robot or result…" bind:value={q} aria-label="Search the log" />
		</div>
	</div>

	<div class="flex flex-wrap gap-1.5" role="group" aria-label="Status">
		{#each chips as [k, l, n] (k)}
			<button aria-pressed={status === k} class="rounded-full px-3 py-1 text-sm {status === k ? 'bg-brand-100 font-medium text-brand-900' : 'bg-slate-100 text-slate-600 hover:bg-slate-200'}" onclick={() => (status = k)}>
				{l} <span class="ml-0.5 tabular-nums {k === 'failed' && n ? 'text-red-700' : ''}">{n}</span>
			</button>
		{/each}
	</div>

	{#if report?.by_source.length}
		<details class="card">
			<summary class="cursor-pointer px-5 py-3 text-sm font-medium text-slate-800">By robot and schedule</summary>
			<div class="overflow-x-auto border-t border-slate-100">
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500">
						<tr><th class="px-5 py-2 font-medium">Name</th><th class="px-3 py-2 text-right font-medium">Runs</th><th class="px-3 py-2 text-right font-medium">Done</th><th class="px-3 py-2 text-right font-medium">Failed</th><th class="px-3 py-2 text-right font-medium">Skipped</th><th class="px-5 py-2 text-right font-medium">Waiting</th></tr>
					</thead>
					<tbody class="divide-y divide-slate-50">
						{#each report.by_source as s ((s.robot_id ?? s.schedule_id) + s.name)}
							<tr class="hover:bg-slate-50">
								<td class="px-5 py-2"><button class="text-left text-slate-800 hover:underline" onclick={() => (source = s.robot_id ?? s.schedule_id ?? '')}>{s.name}</button>{#if s.schedule_id}<span class="ml-1.5 text-xs text-slate-400">schedule</span>{/if}</td>
								<td class="px-3 py-2 text-right tabular-nums">{s.total}</td>
								<td class="px-3 py-2 text-right tabular-nums">{s.done}</td>
								<td class="px-3 py-2 text-right tabular-nums {s.failed ? 'font-medium text-red-700' : ''}">{s.failed}</td>
								<td class="px-3 py-2 text-right tabular-nums">{s.skipped}</td>
								<td class="px-5 py-2 text-right tabular-nums">{s.open}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</details>
	{/if}

	<div class="card overflow-x-auto {loading ? 'opacity-70' : ''}">
		<table class="w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs text-slate-500">
				<tr><th class="px-5 py-2.5 font-medium">When</th><th class="px-3 py-2.5 font-medium">Record</th><th class="px-3 py-2.5 font-medium">Robot</th><th class="px-3 py-2.5 font-medium">Status</th><th class="px-3 py-2.5 font-medium">Result</th><th class="px-5 py-2.5"></th></tr>
			</thead>
			<tbody class="divide-y divide-slate-50">
				{#each items as r (r.id)}
					{@const st = STATUS[r.status] ?? [r.status, 'bg-slate-100 text-slate-600']}
					<tr class="align-top hover:bg-slate-50" data-run={r.id}>
						<td class="px-5 py-2.5 whitespace-nowrap text-slate-600">{formatDateTime(r.finished_at ?? r.created_at)}</td>
						<td class="max-w-56 px-3 py-2.5"><a class="block truncate text-brand-700 hover:underline" href="/crm/{r.type_code}/{r.item_id}">#{r.item_number} {r.item_title}</a></td>
						<td class="px-3 py-2.5 text-slate-800">{r.robot_name}{#if r.schedule_id}<span class="ml-1.5 text-xs text-slate-400">schedule</span>{/if}</td>
						<td class="px-3 py-2.5 whitespace-nowrap">
							<span class="rounded-full px-2 py-0.5 text-xs font-medium {st[1]}">{st[0]}</span>
							{#if r.attempts}<span class="ml-1 text-xs text-slate-400">{r.attempts} retr{r.attempts === 1 ? 'y' : 'ies'}</span>{/if}
						</td>
						<td class="max-w-md px-3 py-2.5 text-slate-600">
							<p class="line-clamp-2 break-words" title={r.result ?? ''}>
								{#if r.status === 'scheduled'}at {formatDateTime(r.run_at)}{#if r.result} — {r.result}{/if}{:else if r.status === 'waiting'}until {formatDateTime(r.wait_until)} at most{:else}{r.result ?? ''}{/if}
							</p>
						</td>
						<td class="px-5 py-2.5 text-right">{#if r.status === 'failed'}<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => retry(r)}><Icon name="refresh" class="size-3.5" /> Retry</button>{/if}</td>
					</tr>
				{:else}
					<tr><td colspan="6" class="px-5 py-8 text-center text-slate-500">{loading ? 'Loading…' : 'Nothing ran in this period.'}</td></tr>
				{/each}
			</tbody>
		</table>
		{#if report && items.length < report.total}
			<div class="border-t border-slate-100 p-2 text-center"><button class="btn btn-ghost text-sm" onclick={more}>Show more ({report.total - items.length} left)</button></div>
		{/if}
	</div>
</div>
