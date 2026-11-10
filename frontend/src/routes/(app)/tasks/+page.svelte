<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Gantt from '#lib/components/Gantt.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import TaskBoard from '#lib/components/TaskBoard.svelte';
	import TaskCalendar from '#lib/components/TaskCalendar.svelte';
	import TaskForm from '#lib/components/TaskForm.svelte';
	import TaskTable from '#lib/components/TaskTable.svelte';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { STAGE_COLORS, type TaskCounters, type TaskDetail, type TaskSummary } from '#lib/tasks.ts';
	import { PERM, type Page } from '#lib/types.ts';

	const VIEWS = [
		{ id: 'my', label: 'My tasks' },
		{ id: 'created', label: 'Created by me' },
		{ id: 'assisting', label: 'Assisting' },
		{ id: 'watching', label: 'Watching' },
		{ id: 'supervising', label: 'My team' }
	];
	const SORTS = [
		{ id: 'deadline', label: 'By deadline' },
		{ id: 'activity', label: 'Recent activity' },
		{ id: 'created', label: 'Newest' },
		{ id: 'priority', label: 'By priority' }
	];
	const STATES = [
		{ id: 'open', label: 'Open', statuses: 'new,in_progress,review' },
		{ id: 'done', label: 'Done', statuses: 'done' },
		{ id: 'deferred', label: 'Deferred', statuses: 'deferred' },
		{ id: 'all', label: 'All', statuses: '' }
	];

	const view = $derived(page.url.searchParams.get('view') ?? 'my');
	const stateFilter = $derived(page.url.searchParams.get('state') ?? 'open');
	const layout = $derived(page.url.searchParams.get('layout') ?? 'list');
	const overdueOnly = $derived(page.url.searchParams.get('overdue') === 'true');
	const unreadOnly = $derived(page.url.searchParams.get('unread') === 'true');
	const sort = $derived(page.url.searchParams.get('sort'));
	// My planner: my own columns.
	type Planned = TaskSummary & { planner_stage_id: string };
	let planner = $state<{ stages: { id: string; name: string; color: string }[]; tasks: Planned[] } | null>(null);
	const plannerColumns = $derived(planner?.stages.map((s) => ({ key: s.id, label: s.name, dot: STAGE_COLORS[s.color] ?? 'bg-slate-400' })) ?? []);
	async function planTask(t: TaskSummary, stage: string, after: string | null, before: string | null) {
		const was = (t as Planned).planner_stage_id;
		(t as Planned).planner_stage_id = stage;
		const d = await api.post<TaskDetail>(`/tasks/${t.id}/planner`, { stage_id: stage, after_id: after, before_id: before }).catch((e) => {
			(t as Planned).planner_stage_id = was;
			throw e;
		});
		// The card keeps the planner's own place, not the task's board position.
		return { ...d, position: t.position, stage_id: t.stage_id };
	}
	async function addPlannerColumn() {
		const name = prompt('Column name');
		if (!name) return;
		planner = await api.post<NonNullable<typeof planner>>('/tasks/planner/stages', { name });
	}

	let q = $state('');
	let data = $state<Page<TaskSummary> | null>(null);
	let counters = $state<TaskCounters | null>(null);
	let formOpen = $state(false);
	let loading = $state(false);
	let gantt = $state<{ tasks: TaskSummary[]; dependencies: { task_id: string; depends_on_id: string }[] } | null>(null);

	function setParam(changes: Record<string, string | null>) {
		const u = new URL(page.url.href);
		for (const [k, v] of Object.entries(changes)) {
			if (v === null) u.searchParams.delete(k);
			else u.searchParams.set(k, v);
		}
		goto(u.pathname + u.search, { replace: true, reset: false });
	}

	async function load() {
		loading = true;
		try {
			if (layout === 'calendar') return;
			if (layout === 'planner') {
				[planner, counters] = await Promise.all([api.get<NonNullable<typeof planner>>('/tasks/planner'), api.get<TaskCounters>('/tasks/counters')]);
				return;
			}
			if (layout === 'gantt') {
				[gantt, counters] = await Promise.all([
					api.get<typeof gantt>('/tasks/gantt' + query({ view: view === 'all' ? 'all' : view === 'created' ? 'created' : 'my' })),
					api.get<TaskCounters>('/tasks/counters')
				]);
				return;
			}
			// A board shows every column, so it ignores the open/done filter.
			const statuses = layout === 'board' ? 'new,in_progress,review,done' : STATES.find((s) => s.id === stateFilter)?.statuses;
			const [d, c] = await Promise.all([
				api.get<Page<TaskSummary>>(
					'/tasks' +
						query({
							view: view === 'all' ? undefined : view,
							status: statuses,
							q,
							overdue: overdueOnly || undefined,
							unread: unreadOnly || undefined,
							sort: layout === 'board' ? 'position' : (sort ?? (stateFilter === 'done' ? 'activity' : 'deadline')),
							limit: 200
						})
				),
				api.get<TaskCounters>('/tasks/counters')
			]);
			data = d;
			counters = c;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		view;
		stateFilter;
		layout;
		overdueOnly;
		unreadOnly;
		sort;
		load();
	});

	let timer: ReturnType<typeof setTimeout>;
	function onSearch() {
		clearTimeout(timer);
		timer = setTimeout(load, 250);
	}

	onMount(() => {
		let pending: ReturnType<typeof setTimeout>;
		// Refresh when any task we can see changes (debounced; many events can arrive together).
		return realtime.on('task.changed', () => {
			clearTimeout(pending);
			pending = setTimeout(load, 300);
		});
	});

	const views = $derived(session.can(PERM.TASKS_MANAGE_ALL) ? [...VIEWS, { id: 'all', label: 'All tasks' }] : VIEWS);
</script>

<svelte:head><title>Tasks · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Tasks</h1>
			{#if counters}
				<p class="text-sm text-slate-500">
					{counters.my_open} open for you
					{#if counters.my_overdue}· <button class="font-medium text-red-600 hover:underline" onclick={() => setParam({ view: 'my', overdue: 'true', state: 'open', layout: 'list' })}>{counters.my_overdue} overdue</button>{/if}
					{#if counters.awaiting_my_review}· <button class="font-medium text-amber-600 hover:underline" onclick={() => setParam({ view: 'created', state: 'open', overdue: null })}>{counters.awaiting_my_review} waiting for your review</button>{/if}
				</p>
			{/if}
		</div>
		<div class="flex gap-2">
			<a href="/flows" class="btn btn-ghost">Flows</a>
			<a href="/tasks/reports" class="btn btn-ghost">Reports</a>
			<a href="/tasks/trash" class="btn btn-ghost" title="Recycle bin"><Icon name="trash" class="size-4" /> Bin</a>
			{#if session.can(PERM.TASKS_MANAGE_ALL)}<a href="/tasks/fields" class="btn btn-ghost">Fields</a>{/if}
			<a href="/tasks/templates" class="btn btn-secondary"><Icon name="refresh" class="size-4" /> Templates</a>
			<button class="btn btn-primary" onclick={() => (formOpen = true)}><Icon name="plus" class="size-4" /> New task</button>
		</div>
	</div>

	<div class="flex flex-wrap items-center gap-3">
		<nav class="flex rounded-lg bg-slate-100 p-1 text-sm">
			{#each views as v (v.id)}
				<button
					class="rounded-md px-3 py-1.5 font-medium {view === v.id ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-600 hover:text-slate-900'}"
					onclick={() => setParam({ view: v.id, overdue: null })}
				>
					{v.label}
				</button>
			{/each}
		</nav>
		<div class="relative min-w-48 flex-1">
			<Icon name="search" class="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-slate-400" />
			<input class="input pl-9" placeholder="Search tasks or #number" bind:value={q} oninput={onSearch} />
		</div>
		{#if layout === 'list'}
			<select class="input w-36" value={stateFilter} onchange={(e) => setParam({ state: e.currentTarget.value })}>
				{#each STATES as s (s.id)}<option value={s.id}>{s.label}</option>{/each}
			</select>
		{/if}
		{#if layout === 'list'}
			<select class="input w-40" aria-label="Sort" value={sort ?? ''} onchange={(e) => setParam({ sort: e.currentTarget.value || null })}>
				<option value="">Default order</option>
				{#each SORTS as o (o.id)}<option value={o.id}>{o.label}</option>{/each}
			</select>
			<button class="badge py-1 {unreadOnly ? 'bg-brand-600 text-white' : 'bg-slate-100 text-slate-600'}" aria-pressed={unreadOnly} onclick={() => setParam({ unread: unreadOnly ? null : 'true' })}>New comments</button>
			<button class="badge py-1 {overdueOnly ? 'bg-red-600 text-white' : 'bg-slate-100 text-slate-600'}" aria-pressed={overdueOnly} onclick={() => setParam({ overdue: overdueOnly ? null : 'true', state: 'open' })}>Overdue</button>
		{/if}
		<div class="flex rounded-lg border border-slate-300 bg-white p-0.5">
			<button class="rounded-md p-1.5 {layout === 'list' ? 'bg-slate-100 text-slate-900' : 'text-slate-400'}" aria-label="List" onclick={() => setParam({ layout: 'list' })}><Icon name="list" class="size-4" /></button>
			<button class="rounded-md p-1.5 {layout === 'board' ? 'bg-slate-100 text-slate-900' : 'text-slate-400'}" aria-label="Board" onclick={() => setParam({ layout: 'board' })}><Icon name="kanban" class="size-4" /></button>
			<button class="rounded-md p-1.5 {layout === 'gantt' ? 'bg-slate-100 text-slate-900' : 'text-slate-400'}" aria-label="Gantt" onclick={() => setParam({ layout: 'gantt' })}><Icon name="gantt" class="size-4" /></button>
			<button class="rounded-md p-1.5 {layout === 'calendar' ? 'bg-slate-100 text-slate-900' : 'text-slate-400'}" aria-label="Calendar" onclick={() => setParam({ layout: 'calendar' })}><Icon name="calendar" class="size-4" /></button>
			<button class="rounded-md p-1.5 {layout === 'planner' ? 'bg-slate-100 text-slate-900' : 'text-slate-400'}" aria-label="Planner" title="My planner" onclick={() => setParam({ layout: 'planner' })}><Icon name="columns" class="size-4" /></button>
		</div>
	</div>

	<div class={loading && !data ? 'opacity-50' : ''}>
		{#if layout === 'calendar'}
			<TaskCalendar {view} />
		{:else if layout === 'planner' && planner}
			<div class="mb-2 flex items-center gap-3 text-sm text-slate-500">
				<span class="flex-1">My planner: your open tasks, in columns of your own. Moving a card here changes nothing for others.</span>
				<button class="btn btn-ghost text-sm" onclick={addPlannerColumn}><Icon name="plus" class="size-4" /> Column</button>
			</div>
			<TaskBoard bind:tasks={planner.tasks} columns={plannerColumns} columnOf={(t) => (t as Planned).planner_stage_id} move={planTask} showProject />
		{:else if layout === 'gantt' && gantt}
			<Gantt bind:tasks={gantt.tasks} dependencies={gantt.dependencies} onchanged={load} />
		{:else if layout === 'board' && data}
			<TaskBoard bind:tasks={data.items} showProject onchanged={load} />
		{:else if data}
			<TaskTable tasks={data.items} empty={q ? 'No tasks match your search.' : 'Nothing here. Enjoy the calm!'} />
			{#if data.total > data.items.length}<p class="mt-2 text-sm text-slate-500">Showing {data.items.length} of {data.total}. Narrow the search to see more.</p>{/if}
		{/if}
	</div>
</div>

<TaskForm bind:open={formOpen} onsaved={(t) => goto(`/tasks/${t.id}`)} />
