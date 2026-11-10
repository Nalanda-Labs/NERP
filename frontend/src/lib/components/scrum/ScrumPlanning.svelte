<script lang="ts">
	import { api, query } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { EPIC_COLORS, fmtPoints, POINT_SCALE, STATUS, type Epic, type ScrumOverview, type Sprint, type TaskSummary } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { Page } from '#lib/types.ts';
	import Avatar from '../Avatar.svelte';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';

	let {
		projectId,
		overview,
		onchanged
	}: {
		projectId: string;
		overview: ScrumOverview;
		/** Something changed: reload the overview. */
		onchanged: () => void;
	} = $props();

	const OPEN = 'new,in_progress,review,deferred';
	const BACKLOG = 'backlog';

	let tasks = $state<TaskSummary[]>([]);
	let epicFilter = $state<string>('');
	let dragging = $state<string | null>(null);
	let over = $state<{ list: string; index: number } | null>(null);

	async function all(params: Record<string, string | number | boolean | undefined>) {
		const out: TaskSummary[] = [];
		for (let offset = 0; ; offset += 200) {
			const p = await api.get<Page<TaskSummary>>('/tasks' + query({ project_id: projectId, sort: 'backlog', limit: 200, offset, ...params }));
			out.push(...p.items);
			if (out.length >= p.total || !p.items.length) return out;
		}
	}

	export async function load() {
		const lists = await Promise.all([all({ backlog: true, status: OPEN }), ...overview.sprints.map((s) => all({ sprint_id: s.id }))]);
		tasks = lists.flat();
	}
	$effect(() => {
		overview.sprints.map((s) => s.id).join();
		load();
	});

	const listOf = (t: TaskSummary) => t.sprint_id ?? BACKLOG;
	function items(list: string) {
		return tasks
			.filter((t) => listOf(t) === list && (!epicFilter || t.epic_id === epicFilter || (epicFilter === 'none' && !t.epic_id)))
			.sort((a, b) => a.backlog_sort - b.backlog_sort || a.created_at.localeCompare(b.created_at));
	}
	const pointsOf = (list: string) => tasks.filter((t) => listOf(t) === list).reduce((n, t) => n + (t.story_points ?? 0), 0);

	async function place(t: TaskSummary, body: Record<string, unknown>) {
		try {
			const updated = await api.put<TaskSummary>(`/tasks/${t.id}/scrum`, body);
			tasks = tasks.map((x) => (x.id === t.id ? updated : x));
			onchanged();
		} catch (e) {
			toasts.error(e);
			load();
		}
	}

	// ----- drag and drop between the backlog and sprints -----
	function onDragOver(e: DragEvent, list: string, el: HTMLElement) {
		if (!dragging) return;
		e.preventDefault();
		const rows = [...el.querySelectorAll<HTMLElement>('[data-row]')].filter((r) => r.dataset.row !== dragging);
		let index = rows.length;
		for (let i = 0; i < rows.length; i++) {
			const r = rows[i]!.getBoundingClientRect();
			if (e.clientY < r.top + r.height / 2) {
				index = i;
				break;
			}
		}
		over = { list, index };
	}
	async function onDrop(e: DragEvent, list: string) {
		e.preventDefault();
		const t = tasks.find((x) => x.id === dragging);
		const target = over;
		dragging = null;
		over = null;
		if (!t || !target) return;
		const others = items(list).filter((x) => x.id !== t.id);
		const after = others[target.index - 1]?.id ?? null;
		const before = others[target.index]?.id ?? null;
		// Optimistic: show it in place straight away.
		const prev = others[target.index - 1]?.backlog_sort;
		const next = others[target.index]?.backlog_sort;
		const sort = prev != null && next != null ? (prev + next) / 2 : prev != null ? prev + 1 : next != null ? next - 1 : 0;
		tasks = tasks.map((x) => (x.id === t.id ? { ...x, sprint_id: list === BACKLOG ? null : list, backlog_sort: sort } : x));
		await place(t, { sprint_id: list === BACKLOG ? null : list, after_id: after, before_id: before });
	}

	// ----- quick add -----
	let adding = $state<Record<string, string>>({});
	async function quickAdd(e: SubmitEvent, list: string) {
		e.preventDefault();
		const title = adding[list]?.trim();
		if (!title) return;
		try {
			const t = await api.post<TaskSummary>('/tasks', {
				title,
				project_id: projectId,
				sprint_id: list === BACKLOG ? null : list,
				epic_id: epicFilter && epicFilter !== 'none' ? epicFilter : null
			});
			adding[list] = '';
			tasks = [...tasks, t];
			onchanged();
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- sprints -----
	async function newSprint() {
		try {
			await api.post(`/projects/${projectId}/sprints`, {});
			onchanged();
		} catch (e) {
			toasts.error(e);
		}
	}
	let startOpen = $state(false);
	let starting = $state<{ sprint: Sprint; ends: string; goal: string } | null>(null);
	function askStart(s: Sprint) {
		const end = new Date(Date.now() + overview.sprint_days * 86400e3);
		starting = { sprint: s, ends: end.toISOString().slice(0, 10), goal: s.goal ?? '' };
		startOpen = true;
	}
	async function start(e: SubmitEvent) {
		e.preventDefault();
		if (!starting) return;
		try {
			const ends = new Date(starting.ends + 'T23:59:00');
			await api.post(`/sprints/${starting.sprint.id}/start`, { ends_at: ends.toISOString(), goal: starting.goal || null });
			startOpen = false;
			toasts.success(`${starting.sprint.name} started`);
			onchanged();
		} catch (err) {
			toasts.error(err);
		}
	}
	let completeOpen = $state(false);
	let completing = $state<{ sprint: Sprint; move_to: string } | null>(null);
	function askComplete(s: Sprint) {
		completing = { sprint: s, move_to: '' };
		completeOpen = true;
	}
	async function complete(e: SubmitEvent) {
		e.preventDefault();
		if (!completing) return;
		try {
			await api.post(`/sprints/${completing.sprint.id}/complete`, { move_to: completing.move_to || null });
			completeOpen = false;
			toasts.success(`${completing.sprint.name} completed`);
			onchanged();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function renameSprint(s: Sprint) {
		const name = prompt('Sprint name', s.name);
		if (!name) return;
		const goal = prompt('Sprint goal (optional)', s.goal ?? '');
		try {
			await api.patch(`/sprints/${s.id}`, { name, goal: goal || null });
			onchanged();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function deleteSprint(s: Sprint) {
		if (!confirm(`Delete ${s.name}? Its tasks go back to the backlog.`)) return;
		try {
			await api.del(`/sprints/${s.id}`);
			onchanged();
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- epics -----
	let epicOpen = $state(false);
	let epicForm = $state<{ id: string | null; name: string; description: string; color: string }>({ id: null, name: '', description: '', color: 'violet' });
	function editEpic(e: Epic | null) {
		epicForm = e ? { id: e.id, name: e.name, description: e.description ?? '', color: e.color } : { id: null, name: '', description: '', color: 'violet' };
		epicOpen = true;
	}
	async function saveEpic(e: SubmitEvent) {
		e.preventDefault();
		const body = { name: epicForm.name, description: epicForm.description || null, color: epicForm.color };
		try {
			if (epicForm.id) await api.patch(`/epics/${epicForm.id}`, body);
			else await api.post(`/projects/${projectId}/epics`, body);
			epicOpen = false;
			onchanged();
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function deleteEpic() {
		if (!epicForm.id || !confirm('Delete this epic? Its tasks stay, without an epic.')) return;
		try {
			await api.del(`/epics/${epicForm.id}`);
			epicOpen = false;
			if (epicFilter === epicForm.id) epicFilter = '';
			onchanged();
			load();
		} catch (err) {
			toasts.error(err);
		}
	}

	const lists = $derived([
		...overview.sprints.map((s) => ({ key: s.id, sprint: s as Sprint | null })),
		{ key: BACKLOG, sprint: null as Sprint | null }
	]);
	const planned = $derived(overview.sprints.filter((s) => s.status === 'planned'));
</script>

<div class="grid gap-5 lg:grid-cols-[1fr_17rem]">
	<div class="min-w-0 space-y-4">
		<div class="flex flex-wrap items-center gap-2">
			{#if overview.can_plan}
				<button class="btn btn-secondary" onclick={newSprint}><Icon name="plus" class="size-4" /> Plan a sprint</button>
			{/if}
			<select class="input ml-auto w-48" aria-label="Show epic" bind:value={epicFilter}>
				<option value="">All epics</option>
				<option value="none">No epic</option>
				{#each overview.epics as e (e.id)}<option value={e.id}>{e.name}</option>{/each}
			</select>
		</div>

		{#each lists as l (l.key)}
			{@const s = l.sprint}
			{@const rows = items(l.key)}
			<section class="card" data-list={s ? s.name : 'Backlog'} aria-label={s ? s.name : 'Backlog'}>
				<header class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-slate-100 px-4 py-3">
					<h2 class="text-sm font-semibold text-slate-900">{s ? s.name : 'Backlog'}</h2>
					{#if s?.status === 'active'}
						<span class="badge bg-emerald-50 text-emerald-700">Active</span>
						<span class="text-xs text-slate-500">{formatDate(s.starts_at!)} – {formatDate(s.ends_at!)}</span>
					{:else if s}
						<span class="badge bg-slate-100 text-slate-600">Planned</span>
					{/if}
					<span class="text-xs text-slate-500" data-list-points>{rows.length} {rows.length === 1 ? 'task' : 'tasks'} · {fmtPoints(pointsOf(l.key))} points</span>
					{#if s?.goal}<span class="w-full text-xs text-slate-600">Goal: {s.goal}</span>{/if}
					{#if s && overview.can_plan}
						<div class="ml-auto flex gap-1">
							{#if s.status === 'planned'}
								<button class="btn btn-primary py-1 text-xs" disabled={!!overview.sprints.find((x) => x.status === 'active')} title={overview.sprints.find((x) => x.status === 'active') ? 'Finish the active sprint first' : ''} onclick={() => askStart(s)}>Start sprint</button>
								<button class="btn btn-ghost p-1.5" aria-label="Edit sprint" onclick={() => renameSprint(s)}><Icon name="edit" class="size-4" /></button>
								<button class="btn btn-ghost p-1.5 text-slate-400 hover:text-red-600" aria-label="Delete sprint" onclick={() => deleteSprint(s)}><Icon name="trash" class="size-4" /></button>
							{:else}
								<button class="btn btn-secondary py-1 text-xs" onclick={() => askComplete(s)}>Complete sprint</button>
								<button class="btn btn-ghost p-1.5" aria-label="Edit sprint" onclick={() => renameSprint(s)}><Icon name="edit" class="size-4" /></button>
							{/if}
						</div>
					{/if}
				</header>
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<ul
					class="min-h-12 divide-y divide-slate-100"
					ondragover={(e) => onDragOver(e, l.key, e.currentTarget)}
					ondragleave={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && (over = null)}
					ondrop={(e) => onDrop(e, l.key)}
				>
					{#each rows.filter((t) => t.id !== dragging) as t, i (t.id)}
						{#if over?.list === l.key && over.index === i}<li class="h-1 bg-brand-500"></li>{/if}
						<li
							class="flex items-center gap-3 px-4 py-2 text-sm {overview.can_contribute ? 'cursor-grab' : ''}"
							data-row={t.id}
							data-task={t.title}
							draggable={overview.can_contribute}
							ondragstart={(e) => {
								dragging = t.id;
								e.dataTransfer?.setData('text/plain', t.id);
							}}
							ondragend={() => {
								dragging = null;
								over = null;
							}}
						>
							<span class="w-14 shrink-0 text-xs text-slate-400">#{t.number}</span>
							<a href="/tasks/{t.id}" class="min-w-0 flex-1 truncate {t.status === 'done' ? 'text-slate-400 line-through' : 'text-slate-800'} hover:text-brand-700">{t.title}</a>
							{#if t.epic_name}<span class="badge hidden sm:inline-flex {EPIC_COLORS[t.epic_color ?? 'violet']}">{t.epic_name}</span>{/if}
							<span class="badge hidden md:inline-flex {STATUS[t.status].badge}">{STATUS[t.status].label}</span>
							<Avatar name={t.responsible_name} fileId={t.responsible_avatar_file_id} size="xs" />
							{#if overview.can_contribute}
								<select
									class="input w-16 px-1 py-0.5 text-xs"
									aria-label="Story points"
									value={t.story_points ?? ''}
									onchange={(e) => place(t, { story_points: e.currentTarget.value === '' ? null : Number(e.currentTarget.value) })}
								>
									<option value="">–</option>
									{#each POINT_SCALE as p (p)}<option value={p}>{fmtPoints(p)}</option>{/each}
									{#if t.story_points != null && !POINT_SCALE.includes(t.story_points)}<option value={t.story_points}>{fmtPoints(t.story_points)}</option>{/if}
								</select>
								<select
									class="input hidden w-28 px-1 py-0.5 text-xs sm:block"
									aria-label="Move to"
									value={t.sprint_id ?? BACKLOG}
									onchange={(e) => place(t, { sprint_id: e.currentTarget.value === BACKLOG ? null : e.currentTarget.value })}
								>
									<option value={BACKLOG}>Backlog</option>
									{#each overview.sprints as sp (sp.id)}<option value={sp.id}>{sp.name}</option>{/each}
								</select>
								<select class="input hidden w-28 px-1 py-0.5 text-xs lg:block" aria-label="Epic" value={t.epic_id ?? ''} onchange={(e) => place(t, { epic_id: e.currentTarget.value || null })}>
									<option value="">No epic</option>
									{#each overview.epics as ep (ep.id)}<option value={ep.id}>{ep.name}</option>{/each}
								</select>
							{:else}
								<span class="w-10 text-right text-xs font-medium text-slate-600">{fmtPoints(t.story_points)}</span>
							{/if}
						</li>
					{:else}
						<li class="px-4 py-4 text-center text-xs text-slate-400">{s ? 'Drag tasks here from the backlog.' : 'The backlog is empty.'}</li>
					{/each}
					{#if over?.list === l.key && over.index >= rows.filter((t) => t.id !== dragging).length && rows.length}<li class="h-1 bg-brand-500"></li>{/if}
				</ul>
				{#if overview.can_contribute}
					<form class="border-t border-slate-100 px-4 py-2" onsubmit={(e) => quickAdd(e, l.key)}>
						<input class="w-full border-0 bg-transparent p-1 text-sm placeholder:text-slate-400 focus:ring-0" placeholder={s ? `Add a task to ${s.name}…` : 'Add a task to the backlog…'} aria-label={s ? `Add a task to ${s.name}` : 'Add a task to the backlog'} bind:value={adding[l.key]} />
					</form>
				{/if}
			</section>
		{/each}
	</div>

	<aside class="space-y-4">
		<section class="card" aria-label="Epics">
			<header class="flex items-center border-b border-slate-100 px-4 py-3">
				<h2 class="text-sm font-semibold text-slate-900">Epics</h2>
				{#if overview.can_contribute}<button class="btn btn-ghost ml-auto p-1" aria-label="New epic" onclick={() => editEpic(null)}><Icon name="plus" class="size-4" /></button>{/if}
			</header>
			<ul class="divide-y divide-slate-100">
				{#each overview.epics as e (e.id)}
					<li class="px-4 py-2.5" data-epic={e.name}>
						<button class="flex w-full items-center gap-2 text-left" onclick={() => editEpic(e)} disabled={!overview.can_contribute}>
							<span class="badge {EPIC_COLORS[e.color]}">{e.name}</span>
							<span class="ml-auto text-xs text-slate-500">{fmtPoints(e.points_done)}/{fmtPoints(e.points_total)} pts</span>
						</button>
						<div class="mt-1.5 h-1.5 overflow-hidden rounded bg-slate-100">
							<div class="h-full bg-emerald-500" style="width: {e.points_total ? (100 * e.points_done) / e.points_total : e.task_count ? (100 * e.done_count) / e.task_count : 0}%"></div>
						</div>
					</li>
				{:else}
					<li class="px-4 py-4 text-center text-xs text-slate-400">No epics yet.</li>
				{/each}
			</ul>
		</section>
		<section class="card px-4 py-3 text-sm text-slate-600">
			<p><span class="font-medium text-slate-800">{overview.backlog.count}</span> in the backlog, {fmtPoints(overview.backlog.points)} points{#if overview.backlog.unestimated}, {overview.backlog.unestimated} not estimated{/if}.</p>
			<p class="mt-1">Scrum master: <span class="font-medium text-slate-800">{overview.scrum_master?.full_name ?? 'none'}</span></p>
		</section>
	</aside>
</div>

<Modal bind:open={startOpen} title="Start {starting?.sprint.name ?? 'sprint'}">
	{#if starting}
		<form id="sprint-start" class="space-y-4" onsubmit={start}>
			<p class="text-sm text-slate-600">{starting.sprint.task_count} tasks, {fmtPoints(starting.sprint.points_total)} points. The points not done yet become the sprint's commitment.</p>
			<div><label class="label" for="ss-end">Ends on</label><input id="ss-end" type="date" class="input" required bind:value={starting.ends} /></div>
			<div><label class="label" for="ss-goal">Sprint goal</label><textarea id="ss-goal" class="input" rows="2" bind:value={starting.goal}></textarea></div>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (startOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="sprint-start">Start</button>
	{/snippet}
</Modal>

<Modal bind:open={completeOpen} title="Complete {completing?.sprint.name ?? 'sprint'}">
	{#if completing}
		{@const left = completing.sprint.task_count - completing.sprint.done_count}
		<form id="sprint-complete" class="space-y-4" onsubmit={complete}>
			<p class="text-sm text-slate-600">{completing.sprint.done_count} of {completing.sprint.task_count} tasks done, {fmtPoints(completing.sprint.points_done)} of {fmtPoints(completing.sprint.points_total)} points.</p>
			{#if left > 0}
				<div>
					<label class="label" for="sc-move">Move the {left} unfinished {left === 1 ? 'task' : 'tasks'} to</label>
					<select id="sc-move" class="input" bind:value={completing.move_to}>
						<option value="">The backlog</option>
						{#each planned as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
					</select>
				</div>
			{/if}
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (completeOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="sprint-complete">Complete</button>
	{/snippet}
</Modal>

<Modal bind:open={epicOpen} title={epicForm.id ? 'Epic' : 'New epic'}>
	<form id="epic-form" class="space-y-4" onsubmit={saveEpic}>
		<div><label class="label" for="ep-name">Name</label><input id="ep-name" class="input" required bind:value={epicForm.name} /></div>
		<div><label class="label" for="ep-desc">Description</label><textarea id="ep-desc" class="input" rows="3" bind:value={epicForm.description}></textarea></div>
		<div>
			<span class="label">Colour</span>
			<div class="flex flex-wrap gap-2">
				{#each Object.entries(EPIC_COLORS) as [name, cls] (name)}
					<button type="button" aria-label={name} class="badge {cls} {epicForm.color === name ? 'ring-2 ring-slate-400 ring-offset-1' : ''}" onclick={() => (epicForm.color = name)}>{name}</button>
				{/each}
			</div>
		</div>
		{#if epicForm.id && overview.can_plan}
			<button type="button" class="text-sm text-red-600 hover:underline" onclick={deleteEpic}>Delete epic</button>
		{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (epicOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="epic-form">Save</button>
	{/snippet}
</Modal>
