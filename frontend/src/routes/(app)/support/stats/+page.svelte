<script lang="ts">
	// Help-desk statistics for a period: volumes, response and resolution times, late
	// replies, ratings; tickets by category, status, criticality and person.
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import { duration } from '#lib/support.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Group = { id: string | null; name: string; open: number; created: number };
	type Stats = {
		from: string; to: string; created: number; closed: number; auto_closed: number; open: number; overdue: number; unassigned: number; on_hold: number;
		avg_first_response_minutes: number | null; avg_resolution_minutes: number | null; replies: number; late_replies: number; minutes_spent: number;
		by_category: Group[]; by_status: Group[]; by_criticality: Group[];
		by_responsible: { id: string | null; name: string; open: number; overdue: number; closed: number; minutes_spent: number; late_replies: number }[];
		marks: { id: string; name: string; count: number }[];
		daily: { day: string; created: number; closed: number }[];
	};
	const ymd = (d: Date) => d.toISOString().slice(0, 10);
	let to = $state(ymd(new Date()));
	let from = $state(ymd(new Date(Date.now() - 29 * 86400000)));
	let s = $state<Stats | null>(null);
	let showTable = $state(false);
	let hover = $state<number | null>(null);
	async function load() {
		try {
			s = await api.get<Stats>(`/support/stats${query({ from, to })}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	const peak = $derived(Math.max(1, ...(s?.daily ?? []).map((d) => d.created)));
	const rated = $derived((s?.marks ?? []).reduce((n, m) => n + m.count, 0));
	const dayLabel = (d: string) => new Date(d + 'T00:00:00').toLocaleDateString([], { day: 'numeric', month: 'short' });
</script>

<svelte:head><title>Statistics · Help desk · NERP</title></svelte:head>

<div class="max-w-6xl space-y-4">
	<div><a href="/support" class="text-sm text-slate-500 hover:underline">‹ Help desk</a><h1 class="mt-1 text-2xl font-semibold text-slate-900">Help-desk statistics</h1></div>
	<form class="flex flex-wrap items-end gap-2" onsubmit={(e) => (e.preventDefault(), load())}>
		<div><label class="label" for="st-from">From</label><input id="st-from" class="input" type="date" bind:value={from} /></div>
		<div><label class="label" for="st-to">To</label><input id="st-to" class="input" type="date" bind:value={to} /></div>
		<button class="btn btn-secondary" type="submit">Show</button>
	</form>

	{#if s}
		<div class="grid grid-cols-2 gap-3 md:grid-cols-4">
			{#each [
				['Created', String(s.created), 'in the period'],
				['Closed', String(s.closed), s.auto_closed ? `${s.auto_closed} automatically` : 'in the period'],
				['Open now', String(s.open), `${s.unassigned} unassigned · ${s.on_hold} on hold`],
				['Overdue now', String(s.overdue), 'waiting past the SLA'],
				['First reply', duration(s.avg_first_response_minutes), 'average'],
				['Resolution', duration(s.avg_resolution_minutes), 'average, created to closed'],
				['Late replies', `${s.late_replies} of ${s.replies}`, 'after the deadline'],
				['Time spent', duration(s.minutes_spent), 'logged by support']
			] as [label, value, note] (label)}
				<div class="card p-4" data-stat={label}>
					<p class="text-sm text-slate-500">{label}</p>
					<p class="mt-1 text-2xl font-semibold text-slate-900">{value}</p>
					<p class="text-xs text-slate-500">{note}</p>
				</div>
			{/each}
		</div>

		<section class="card p-4">
			<div class="mb-3 flex items-center justify-between">
				<h2 class="font-medium text-slate-900">Tickets created per day</h2>
				<button class="text-sm text-brand-700 hover:underline" onclick={() => (showTable = !showTable)}>{showTable ? 'Show chart' : 'Show as table'}</button>
			</div>
			{#if showTable}
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500 uppercase"><tr><th class="py-1">Day</th><th class="py-1 text-right">Created</th><th class="py-1 text-right">Closed</th></tr></thead>
					<tbody class="divide-y divide-slate-100">{#each s.daily as d (d.day)}<tr><td class="py-1">{dayLabel(d.day)}</td><td class="py-1 text-right tabular-nums">{d.created}</td><td class="py-1 text-right tabular-nums">{d.closed}</td></tr>{/each}</tbody>
				</table>
			{:else}
				<div class="relative">
					<div class="flex h-40 items-end gap-0.5 border-b border-slate-300" role="img" aria-label="Tickets created per day, {from} to {to}">
						{#each s.daily as d, i (d.day)}
							<div class="flex h-full flex-1 items-end justify-center" role="presentation" onmouseenter={() => (hover = i)} onmouseleave={() => (hover = null)}>
								<div class="w-full max-w-3 rounded-t bg-brand-600 {hover === i ? 'opacity-80' : ''}" style="height: {(d.created / peak) * 100}%; min-height: {d.created ? 2 : 0}px"></div>
							</div>
						{/each}
					</div>
					{#if hover !== null}
						{@const d = s.daily[hover]}
						<div class="pointer-events-none absolute -top-2 rounded-lg bg-slate-900 px-2.5 py-1.5 text-xs text-white shadow" style="left: clamp(0px, calc({((hover + 0.5) / s.daily.length) * 100}% - 4rem), calc(100% - 8rem))">
							<p class="font-medium">{dayLabel(d.day)}</p><p>{d.created} created · {d.closed} closed</p>
						</div>
					{/if}
					<div class="mt-1 flex justify-between text-xs text-slate-500"><span>{dayLabel(s.daily[0]?.day ?? from)}</span><span>peak {peak}</span><span>{dayLabel(s.daily.at(-1)?.day ?? to)}</span></div>
				</div>
			{/if}
		</section>

		<div class="grid gap-4 md:grid-cols-3">
			{#each [['By category', s.by_category], ['By status', s.by_status], ['By criticality', s.by_criticality]] as [title, rows] (title)}
				<section class="card p-4">
					<h2 class="mb-2 font-medium text-slate-900">{title}</h2>
					<table class="w-full text-sm">
						<thead class="text-left text-xs text-slate-500 uppercase"><tr><th class="py-1"></th><th class="py-1 text-right">Open</th><th class="py-1 text-right">New</th></tr></thead>
						<tbody class="divide-y divide-slate-100">
							{#each rows as Group[] as g (g.id ?? 'none')}<tr><td class="py-1">{g.name}</td><td class="py-1 text-right tabular-nums">{g.open}</td><td class="py-1 text-right tabular-nums">{g.created}</td></tr>{/each}
						</tbody>
					</table>
				</section>
			{/each}
		</div>

		<div class="grid gap-4 md:grid-cols-[2fr_1fr]">
			<section class="card overflow-x-auto p-4">
				<h2 class="mb-2 font-medium text-slate-900">By person</h2>
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500 uppercase"><tr><th class="py-1"></th><th class="py-1 text-right">Open</th><th class="py-1 text-right">Overdue</th><th class="py-1 text-right">Closed</th><th class="py-1 text-right">Late replies</th><th class="py-1 text-right">Time spent</th></tr></thead>
					<tbody class="divide-y divide-slate-100">
						{#each s.by_responsible as p (p.id ?? 'none')}
							<tr data-person={p.name}><td class="py-1">{p.name}</td><td class="py-1 text-right tabular-nums">{p.open}</td><td class="py-1 text-right tabular-nums {p.overdue ? 'font-medium text-red-600' : ''}">{p.overdue}</td><td class="py-1 text-right tabular-nums">{p.closed}</td><td class="py-1 text-right tabular-nums">{p.late_replies}</td><td class="py-1 text-right">{duration(p.minutes_spent)}</td></tr>
						{/each}
					</tbody>
				</table>
			</section>
			<section class="card p-4">
				<h2 class="mb-2 font-medium text-slate-900">Ratings</h2>
				{#if rated}
					<ul class="space-y-2 text-sm">
						{#each s.marks as m (m.id)}
							<li>
								<div class="flex justify-between"><span>{m.name}</span><span class="tabular-nums text-slate-600">{m.count}</span></div>
								<div class="mt-1 h-1.5 rounded-full bg-slate-100"><div class="h-1.5 rounded-full bg-brand-600" style="width: {(m.count / rated) * 100}%"></div></div>
							</li>
						{/each}
					</ul>
				{:else}<p class="text-sm text-slate-500">No ratings in this period.</p>{/if}
			</section>
		</div>
	{/if}
</div>
