<script lang="ts">
	// Workload and efficiency: per person, over a period.
	import { api, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Row = {
		user_id: string;
		full_name: string;
		avatar_file_id: string | null;
		done: number;
		on_time: number;
		late: number;
		open: number;
		overdue: number;
		in_progress: number;
		open_estimate_hours: number;
		logged_hours: number;
		avg_completion_hours: number | null;
		rated_good: number;
		rated_bad: number;
		efficiency: number | null;
	};
	type Report = { items: Row[]; totals: { done: number; on_time: number; open: number; overdue: number; efficiency: number | null } };
	const day = (d: Date) => d.toISOString().slice(0, 10);
	let scope = $state('team');
	let from = $state(day(new Date(Date.now() - 30 * 86400e3)));
	let to = $state(day(new Date()));
	let report = $state<Report | null>(null);
	$effect(() => {
		const q = query({ scope, from: new Date(from + 'T00:00:00').toISOString(), to: new Date(to + 'T23:59:59').toISOString() });
		api.get<Report>('/tasks/reports/efficiency' + q).then((r) => (report = r));
	});
	const hours = (h: number | null) => (h == null ? '—' : h < 48 ? `${h} h` : `${(h / 24).toFixed(1)} d`);
	const tone = (e: number | null) => (e == null ? 'text-slate-400' : e >= 80 ? 'text-emerald-700' : e >= 50 ? 'text-amber-700' : 'text-red-700');
</script>

<svelte:head><title>Task reports · NERP</title></svelte:head>

<div class="space-y-4">
	<a href="/tasks" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Tasks</a>
	<div class="flex flex-wrap items-end gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Workload and efficiency</h1>
		<select class="input w-44" aria-label="Whose" bind:value={scope}>
			<option value="me">Mine</option>
			<option value="team">My team</option>
			{#if session.can(PERM.TASKS_MANAGE_ALL)}<option value="all">Everyone</option>{/if}
		</select>
		<label class="text-sm text-slate-600">From <input type="date" class="input w-40" bind:value={from} /></label>
		<label class="text-sm text-slate-600">To <input type="date" class="input w-40" bind:value={to} /></label>
	</div>
	<p class="text-sm text-slate-500">Efficiency is the share of tasks handled on time: finished by their deadline, out of everything finished in the period plus what is overdue now.</p>
	{#if report}
		<div class="grid gap-3 sm:grid-cols-4" aria-label="Totals">
			{#each [['Finished', report.totals.done], ['On time', report.totals.on_time], ['Open', report.totals.open], ['Overdue now', report.totals.overdue]] as [l, v] (l)}
				<div class="card px-4 py-3"><p class="text-xs text-slate-500">{l}</p><p class="text-2xl font-semibold text-slate-900">{v}</p></div>
			{/each}
		</div>
		<div class="card overflow-x-auto">
			<table class="w-full text-sm">
				<thead class="text-left text-xs text-slate-500">
					<tr>
						<th class="px-4 py-2 font-medium">Person</th>
						<th class="px-3 py-2 text-right font-medium">Efficiency</th>
						<th class="px-3 py-2 text-right font-medium">Finished</th>
						<th class="px-3 py-2 text-right font-medium">Late</th>
						<th class="px-3 py-2 text-right font-medium">Open</th>
						<th class="px-3 py-2 text-right font-medium">Overdue</th>
						<th class="px-3 py-2 text-right font-medium">Estimate left</th>
						<th class="px-3 py-2 text-right font-medium">Logged</th>
						<th class="px-3 py-2 text-right font-medium">Avg. to finish</th>
						<th class="px-3 py-2 text-right font-medium">Rated</th>
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-100">
					{#each report.items as r (r.user_id)}
						<tr data-person={r.full_name}>
							<td class="px-4 py-2"><span class="flex items-center gap-2"><Avatar name={r.full_name} fileId={r.avatar_file_id} size="xs" />{r.full_name}</span></td>
							<td class="px-3 py-2 text-right font-semibold {tone(r.efficiency)}" data-efficiency={r.efficiency ?? ''}>
								{#if r.efficiency != null}
									<span class="inline-flex items-center gap-2"><span class="h-1.5 w-16 overflow-hidden rounded bg-slate-100"><span class="block h-full bg-current" style="width: {r.efficiency}%"></span></span>{r.efficiency}%</span>
								{:else}—{/if}
							</td>
							<td class="px-3 py-2 text-right">{r.done}</td>
							<td class="px-3 py-2 text-right {r.late ? 'text-amber-700' : ''}">{r.late}</td>
							<td class="px-3 py-2 text-right">{r.open}</td>
							<td class="px-3 py-2 text-right {r.overdue ? 'font-medium text-red-600' : ''}">{r.overdue}</td>
							<td class="px-3 py-2 text-right">{r.open_estimate_hours} h</td>
							<td class="px-3 py-2 text-right">{r.logged_hours} h</td>
							<td class="px-3 py-2 text-right">{hours(r.avg_completion_hours)}</td>
							<td class="px-3 py-2 text-right">{r.rated_good ? `👍 ${r.rated_good}` : ''} {r.rated_bad ? `👎 ${r.rated_bad}` : ''}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>
