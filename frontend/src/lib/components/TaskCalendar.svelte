<script lang="ts">
	// Tasks on a month calendar by deadline.
	import { api, query } from '#lib/api.ts';
	import { STATUS, type TaskSummary } from '#lib/tasks.ts';
	import type { Page } from '#lib/types.ts';

	let { view, projectId = null }: { view?: string; projectId?: string | null } = $props();
	let month = $state(new Date(new Date().getFullYear(), new Date().getMonth(), 1));
	let tasks = $state<TaskSummary[]>([]);
	// Monday-first grid covering the month.
	const days = $derived.by(() => {
		const first = new Date(month);
		const start = new Date(first);
		start.setDate(1 - ((first.getDay() + 6) % 7));
		return Array.from({ length: 42 }, (_, i) => new Date(start.getFullYear(), start.getMonth(), start.getDate() + i));
	});
	export async function load() {
		const from = days[0]!, to = new Date(days[41]!.getFullYear(), days[41]!.getMonth(), days[41]!.getDate() + 1);
		const r = await api.get<Page<TaskSummary>>(
			'/tasks' + query({ view: view && view !== 'all' ? view : undefined, project_id: projectId ?? undefined, deadline_from: from.toISOString(), deadline_to: to.toISOString(), limit: 200 })
		);
		tasks = r.items;
	}
	$effect(() => {
		month;
		view;
		load();
	});
	const key = (d: Date) => `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
	const byDay = $derived.by(() => {
		const m = new Map<string, TaskSummary[]>();
		for (const t of tasks) {
			const k = key(new Date(t.deadline!));
			m.set(k, [...(m.get(k) ?? []), t]);
		}
		return m;
	});
	const today = key(new Date());
	const shift = (n: number) => (month = new Date(month.getFullYear(), month.getMonth() + n, 1));
</script>

<div class="card" aria-label="Task calendar">
	<div class="flex items-center gap-2 border-b border-slate-100 px-4 py-3">
		<button class="btn btn-ghost px-2" aria-label="Previous month" onclick={() => shift(-1)}>‹</button>
		<h2 class="min-w-40 text-center font-semibold text-slate-900" data-month>{month.toLocaleDateString(undefined, { month: 'long', year: 'numeric' })}</h2>
		<button class="btn btn-ghost px-2" aria-label="Next month" onclick={() => shift(1)}>›</button>
		<button class="btn btn-ghost ml-2 text-sm" onclick={() => (month = new Date(new Date().getFullYear(), new Date().getMonth(), 1))}>Today</button>
	</div>
	<div class="grid grid-cols-7 text-center text-xs text-slate-500">
		{#each ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as d (d)}<div class="py-2">{d}</div>{/each}
	</div>
	<div class="grid grid-cols-7 border-t border-slate-100">
		{#each days as d (key(d))}
			{@const list = byDay.get(key(d)) ?? []}
			<div class="min-h-24 border-r border-b border-slate-100 p-1 {d.getMonth() !== month.getMonth() ? 'bg-slate-50/60' : ''}" data-day={d.getDate()}>
				<p class="mb-1 text-right text-xs {key(d) === today ? 'font-bold text-brand-600' : 'text-slate-400'}">{d.getDate()}</p>
				{#each list.slice(0, 4) as t (t.id)}
					<a href="/tasks/{t.id}" class="mb-0.5 block truncate rounded px-1.5 py-0.5 text-xs {t.status === 'done' ? 'bg-slate-100 text-slate-400 line-through' : t.is_overdue ? 'bg-red-50 text-red-700' : STATUS[t.status].badge}" title={t.title} data-cal-task={t.title}>{t.title}</a>
				{/each}
				{#if list.length > 4}<p class="px-1 text-[11px] text-slate-500">+{list.length - 4} more</p>{/if}
			</div>
		{/each}
	</div>
</div>
