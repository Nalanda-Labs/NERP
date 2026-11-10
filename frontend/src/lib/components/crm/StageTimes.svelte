<script lang="ts">
	// Automation → Time in stages: per open stage, how long visits last (median, average,
	// longest), how many records are there now, and how often the time limit was exceeded.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { durationText } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { categoryId }: { categoryId: string } = $props();

	type Row = {
		stage_id: string; name: string; color: string; semantics: string; sla_minutes: number | null;
		entries: number; avg_hours: number | null; median_hours: number | null; max_hours: number | null; in_stage: number; over_limit: number; breaches: number;
	};
	const PERIODS: [number, string][] = [[30, 'Last 30 days'], [90, 'Last 90 days'], [365, 'Last 12 months']];
	let days = $state(90);
	let rows = $state<Row[]>([]);
	let loading = $state(false);
	let hover = $state<string | null>(null);

	$effect(() => {
		categoryId; days;
		untrack(async () => {
			loading = true;
			try {
				const from = new Date(Date.now() - days * 86400000).toISOString();
				rows = (await api.get<{ stages: Row[] }>(`/crm/automation/stage-times?category_id=${categoryId}&from=${encodeURIComponent(from)}`)).stages;
			} catch (e) {
				toasts.error(e);
			} finally {
				loading = false;
			}
		});
	});

	// Only open stages: time "in" Won or Lost is just time since closing.
	const open = $derived(rows.filter((r) => r.semantics === 'process'));
	const longest = $derived(Math.max(1e-9, ...open.map((r) => r.median_hours ?? 0)));
	const h = (v: number | null) => (v === null ? '—' : v * 60 < 1 ? '< 1 min' : durationText(v * 60));
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<select class="input w-44" bind:value={days} aria-label="Period">{#each PERIODS as [d, l] (d)}<option value={d}>{l}</option>{/each}</select>
		<p class="text-sm text-slate-500">For records that entered each stage in the period; visits still going count until now.</p>
	</div>

	<div class="card overflow-x-auto {loading ? 'opacity-70' : ''}">
		<table class="w-full text-sm">
			<caption class="sr-only">Time records spend in each open stage</caption>
			<thead class="border-b border-slate-100 text-left text-xs text-slate-500">
				<tr>
					<th class="px-5 py-2.5 font-medium">Stage</th>
					<th class="w-[28%] px-3 py-2.5 font-medium">Median time</th>
					<th class="px-3 py-2.5 text-right font-medium">Average</th>
					<th class="px-3 py-2.5 text-right font-medium">Longest</th>
					<th class="px-3 py-2.5 text-right font-medium">Entered</th>
					<th class="px-3 py-2.5 text-right font-medium">There now</th>
					<th class="px-5 py-2.5 text-right font-medium">Over the limit</th>
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-50">
				{#each open as r (r.stage_id)}
					<tr class="hover:bg-slate-50" onmouseenter={() => (hover = r.stage_id)} onmouseleave={() => (hover = null)}>
						<td class="px-5 py-2.5">
							<span class="flex items-center gap-2 text-slate-800"><span class="size-2.5 shrink-0 rounded-full" style="background:{r.color}"></span>{r.name}</span>
							{#if r.sla_minutes}<span class="ml-4.5 text-xs text-slate-500">limit {durationText(r.sla_minutes)}</span>{/if}
						</td>
						<td class="px-3 py-2.5">
							<div class="flex items-center gap-2">
								<div class="h-3 flex-1 rounded-full bg-slate-100" aria-hidden="true">
									{#if r.median_hours}<div class="h-3 rounded-full" style="width:{Math.max(2, (r.median_hours / longest) * 100)}%;background:var(--series-1);opacity:{hover === null || hover === r.stage_id ? 1 : 0.5}"></div>{/if}
								</div>
								<span class="w-20 text-right whitespace-nowrap text-slate-800 tabular-nums">{h(r.median_hours)}</span>
							</div>
						</td>
						<td class="px-3 py-2.5 text-right whitespace-nowrap text-slate-600 tabular-nums">{h(r.avg_hours)}</td>
						<td class="px-3 py-2.5 text-right whitespace-nowrap text-slate-600 tabular-nums">{h(r.max_hours)}</td>
						<td class="px-3 py-2.5 text-right tabular-nums">{r.entries}</td>
						<td class="px-3 py-2.5 text-right tabular-nums">{r.in_stage}</td>
						<td class="px-5 py-2.5 text-right whitespace-nowrap tabular-nums">
							{#if r.sla_minutes}
								<span class={r.over_limit ? 'font-medium text-red-700' : ''}>{r.over_limit} now</span>
								<span class="text-slate-400">· {r.breaches} in period</span>
							{:else}<span class="text-slate-400">no limit</span>{/if}
						</td>
					</tr>
				{:else}
					<tr><td colspan="7" class="px-5 py-8 text-center text-slate-500">{loading ? 'Loading…' : 'No open stages.'}</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
	<p class="text-xs text-slate-500">Medians are less thrown off by a few records that sat for a long time; compare them with the average to spot those.</p>
</div>
