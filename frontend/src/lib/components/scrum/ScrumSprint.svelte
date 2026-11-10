<script lang="ts">
	import { api, query } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { fmtPoints, STAGE_COLORS, type ScrumOverview, type Stage, type TaskDetail, type TaskSummary } from '#lib/tasks.ts';
	import type { Page } from '#lib/types.ts';
	import Modal from '../Modal.svelte';
	import TaskBoard from '../TaskBoard.svelte';

	let {
		overview,
		stages,
		onchanged
	}: {
		overview: ScrumOverview;
		stages: Stage[];
		onchanged: () => void;
	} = $props();

	const sprint = $derived(overview.sprints.find((s) => s.status === 'active') ?? null);
	let tasks = $state<TaskSummary[]>([]);
	const columns = $derived(stages.map((s) => ({ key: s.id, label: s.name, dot: STAGE_COLORS[s.color] ?? 'bg-slate-400' })));

	export async function load() {
		if (!sprint) {
			tasks = [];
			return;
		}
		const p = await api.get<Page<TaskSummary>>('/tasks' + query({ sprint_id: sprint.id, sort: 'position', limit: 200 }));
		tasks = p.items;
	}
	$effect(() => {
		sprint?.id;
		load();
	});

	// The definition of done is checked before a card goes to a "done" column.
	let dodOpen = $state(false);
	let dodTask = $state<TaskSummary | null>(null);
	let ticked = $state<string[]>([]);
	let settle: ((ok: boolean) => void) | null = null;
	function askDod(t: TaskSummary): Promise<boolean> {
		dodTask = t;
		ticked = [];
		dodOpen = true;
		return new Promise((r) => (settle = r));
	}
	async function confirmDod(e: SubmitEvent) {
		e.preventDefault();
		await api.post(`/tasks/${dodTask!.id}/dod`, { checked: ticked });
		dodOpen = false;
		settle?.(true);
		settle = null;
	}
	function cancelDod() {
		settle?.(false);
		settle = null;
	}

	async function move(t: TaskSummary, stage: string, after: string | null, before: string | null) {
		const target = stages.find((s) => s.id === stage);
		if (target?.status === 'done' && t.status !== 'done' && overview.dod.length) {
			if (!(await askDod(t))) throw new Error('Not moved: the definition of done was not confirmed');
		}
		const saved = await api.post<TaskDetail>(`/tasks/${t.id}/stage`, { stage_id: stage, after_id: after, before_id: before });
		onchanged();
		return saved;
	}

	const daysLeft = $derived(sprint?.ends_at ? Math.max(0, Math.ceil((Date.parse(sprint.ends_at) - Date.now()) / 86400e3)) : 0);
	const pointsDone = $derived(tasks.filter((t) => t.status === 'done').reduce((n, t) => n + (t.story_points ?? 0), 0));
	const pointsAll = $derived(tasks.reduce((n, t) => n + (t.story_points ?? 0), 0));
</script>

{#if !sprint}
	<div class="card p-10 text-center text-sm text-slate-500">No sprint is running. Plan one on the Planning tab and start it.</div>
{:else}
	<div class="mb-4 flex flex-wrap items-center gap-x-4 gap-y-2" data-active-sprint={sprint.name}>
		<h2 class="text-lg font-semibold text-slate-900">{sprint.name}</h2>
		<span class="text-sm text-slate-500">{formatDate(sprint.starts_at!)} – {formatDate(sprint.ends_at!)} · {daysLeft} {daysLeft === 1 ? 'day' : 'days'} left</span>
		<div class="flex min-w-48 flex-1 items-center gap-2">
			<div class="h-2 flex-1 overflow-hidden rounded bg-slate-100"><div class="h-full bg-emerald-500" style="width: {pointsAll ? (100 * pointsDone) / pointsAll : 0}%"></div></div>
			<span class="text-xs text-slate-600" data-sprint-progress>{fmtPoints(pointsDone)} / {fmtPoints(pointsAll)} points</span>
		</div>
		{#if sprint.goal}<p class="w-full text-sm text-slate-600">Goal: {sprint.goal}</p>{/if}
	</div>
	<TaskBoard bind:tasks {columns} columnOf={(t) => t.stage_id} {move} onchanged={load} />
{/if}

<Modal bind:open={dodOpen} title="Definition of done" onclose={cancelDod}>
	<form id="dod-form" class="space-y-3" onsubmit={confirmDod}>
		<p class="text-sm text-slate-600">Before <span class="font-medium">{dodTask?.title}</span> is done, confirm each point:</p>
		{#each overview.dod as d (d.id)}
			<label class="flex items-center gap-2 text-sm text-slate-800">
				<input type="checkbox" class="rounded border-slate-300 text-brand-600" value={d.id} bind:group={ticked} />
				{d.title}
			</label>
		{/each}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => ((dodOpen = false), cancelDod())}>Cancel</button>
		<button class="btn btn-primary" form="dod-form" disabled={ticked.length < overview.dod.length}>Done</button>
	{/snippet}
</Modal>
