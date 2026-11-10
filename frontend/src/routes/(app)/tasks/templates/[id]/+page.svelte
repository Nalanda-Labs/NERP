<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import RecurrenceEditor from '#lib/components/RecurrenceEditor.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import type { Priority, ProjectSummary, Recurrence, TaskDetail, TaskTemplate } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { User } from '#lib/types.ts';

	const id = $derived(page.params.id!);
	const isNew = $derived(id === 'new');
	let existing = $state<TaskTemplate | null>(null);
	let projects = $state<ProjectSummary[]>([]);
	let saving = $state(false);
	let loaded = $state(false);

	let form = $state({
		title: '',
		description: '',
		responsible_id: null as string | null,
		responsible_name: null as string | null,
		assistants: [] as PickedUser[],
		watchers: [] as PickedUser[],
		priority: 'normal' as Priority,
		project_id: '',
		requires_review: false,
		estimate_hours: '',
		checklist: '',
		subtasks: '',
		shared: false,
		deadline_amount: '',
		deadline_unit: 'days' as 'hours' | 'days',
		recurrence: null as Recurrence | null,
		is_active: true
	});

	async function people(ids: string[]): Promise<PickedUser[]> {
		const users = await Promise.all(ids.map((u) => api.get<User>(`/users/${u}`).catch(() => null)));
		return users.filter((u): u is User => !!u).map((u) => ({ id: u.id, name: u.full_name, avatar_file_id: u.avatar_file_id }));
	}

	onMount(async () => {
		projects = (await api.get<ProjectSummary[]>('/projects?mine=true')).filter((p) => p.my_role !== 'viewer');
		if (isNew) {
			loaded = true;
			return;
		}
		const t = (existing = await api.get<TaskTemplate>(`/task-templates/${id}`));
		const off = t.deadline_offset_secs;
		const inDays = off !== null && off % 86400 === 0;
		form = {
			title: t.title,
			description: t.description ?? '',
			responsible_id: t.responsible_id,
			responsible_name: t.responsible_name,
			assistants: await people(t.assistant_ids),
			watchers: await people(t.watcher_ids),
			priority: t.priority,
			project_id: t.project_id ?? '',
			requires_review: t.requires_review,
			estimate_hours: t.time_estimate_secs ? String(+(t.time_estimate_secs / 3600).toFixed(2)) : '',
			checklist: t.checklist.join('\n'),
			subtasks: t.subtasks.map((x) => x.title).join('\n'),
			shared: t.shared,
			deadline_amount: off === null ? '' : String(inDays ? off / 86400 : +(off / 3600).toFixed(2)),
			deadline_unit: inDays || off === null ? 'days' : 'hours',
			recurrence: t.recurrence,
			is_active: t.is_active
		};
		loaded = true;
	});

	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		const amount = parseFloat(form.deadline_amount);
		const hours = parseFloat(form.estimate_hours);
		const body = {
			title: form.title,
			description: form.description || null,
			responsible_id: form.responsible_id,
			assistant_ids: form.assistants.map((u) => u.id),
			watcher_ids: form.watchers.map((u) => u.id),
			priority: form.priority,
			project_id: form.project_id || null,
			requires_review: form.requires_review,
			time_estimate_secs: Number.isFinite(hours) && hours > 0 ? Math.round(hours * 3600) : null,
			checklist: form.checklist.split('\n').map((s) => s.trim()).filter(Boolean),
			subtasks: form.subtasks.split('\n').map((s) => s.trim()).filter(Boolean).map((title) => ({ title })),
			shared: form.shared,
			deadline_offset_secs: Number.isFinite(amount) && amount >= 0 ? Math.round(amount * (form.deadline_unit === 'days' ? 86400 : 3600)) : null,
			recurrence: form.recurrence,
			is_active: form.is_active
		};
		try {
			const t = isNew ? await api.post<TaskTemplate>('/task-templates', body) : await api.patch<TaskTemplate>(`/task-templates/${id}`, body);
			toasts.success(t.recurrence ? `Saved. Next task: ${formatDateTime(t.next_run_at)}` : 'Template saved');
			goto('/tasks/templates');
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	async function remove() {
		if (!confirm('Delete this template? Tasks already created from it stay.')) return;
		await api.del(`/task-templates/${id}`);
		goto('/tasks/templates');
	}

	async function useNow() {
		const task = await api.post<TaskDetail>(`/task-templates/${id}/use`);
		goto(`/tasks/${task.id}`);
	}
</script>

<svelte:head><title>{isNew ? 'New template' : (existing?.title ?? 'Template')} · NERP</title></svelte:head>

<div class="max-w-3xl space-y-5">
	<a href="/tasks/templates" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Templates</a>
	<div class="flex flex-wrap items-center justify-between gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">{isNew ? 'New template' : 'Edit template'}</h1>
		{#if existing}
			<p class="text-sm text-slate-500">{existing.task_count} tasks created{existing.last_run_at ? ` · last ${formatDateTime(existing.last_run_at)}` : ''}</p>
		{/if}
	</div>

	{#if !loaded}
		<div class="card p-10 text-center text-slate-400">Loading…</div>
	{:else}
	<form class="space-y-5" onsubmit={save}>
		<section class="card space-y-4 p-6">
			<input class="input text-base font-medium" placeholder="Task title" required maxlength="500" bind:value={form.title} />
			<textarea class="input" rows="3" placeholder="Description (optional)" bind:value={form.description}></textarea>
			<div class="grid gap-4 sm:grid-cols-2">
				<div>
					<span class="label">Responsible</span>
					<UserPicker bind:value={form.responsible_id} bind:label={form.responsible_name} placeholder="Whoever uses the template" />
					{#if form.responsible_id}<button type="button" class="mt-1 text-xs text-slate-500 hover:underline" onclick={() => ((form.responsible_id = null), (form.responsible_name = null))}>Clear (use whoever creates it)</button>{/if}
				</div>
				<div>
					<span class="label">Deadline</span>
					<div class="flex items-center gap-2">
						<input type="number" min="0" step="0.5" class="input w-24" placeholder="—" bind:value={form.deadline_amount} aria-label="Deadline amount" />
						<select class="input w-28" bind:value={form.deadline_unit} aria-label="Deadline unit"><option value="hours">hours</option><option value="days">days</option></select>
						<span class="text-sm text-slate-500">after creation</span>
					</div>
				</div>
				<div>
					<label class="label" for="t-project">Project</label>
					<select id="t-project" class="input" bind:value={form.project_id}>
						<option value="">No project</option>
						{#each projects as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
					</select>
				</div>
				<div>
					<label class="label" for="t-priority">Priority</label>
					<select id="t-priority" class="input" bind:value={form.priority}><option value="high">High</option><option value="normal">Normal</option><option value="low">Low</option></select>
				</div>
				<div><span class="label">Assistants</span><MultiUserPicker bind:value={form.assistants} /></div>
				<div><span class="label">Watchers</span><MultiUserPicker bind:value={form.watchers} /></div>
				<label class="flex items-start gap-2 text-sm">
					<input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={form.requires_review} />
					<span><span class="font-medium text-slate-800">Review when done</span><br /><span class="text-slate-500">You accept the work before it closes.</span></span>
				</label>
				<div>
					<label class="label" for="t-est">Estimate (hours)</label>
					<input id="t-est" type="number" min="0" step="0.25" class="input w-32" bind:value={form.estimate_hours} />
				</div>
			</div>
			<div>
				<label class="label" for="t-checklist">Checklist <span class="font-normal text-slate-400">— one item per line</span></label>
				<textarea id="t-checklist" class="input font-mono text-xs" rows="3" bind:value={form.checklist}></textarea>
			</div>
			<div>
				<label class="label" for="t-subtasks">Subtasks <span class="font-normal text-slate-400">— one per line; made under the task each time</span></label>
				<textarea id="t-subtasks" class="input font-mono text-xs" rows="3" bind:value={form.subtasks}></textarea>
			</div>
			<label class="flex items-center gap-2 text-sm">
				<input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.shared} />
				Share with everyone <span class="text-slate-400">— others can use it; only you change it</span>
			</label>
		</section>

		<section class="card space-y-4 p-6">
			<h2 class="font-semibold text-slate-900">Schedule</h2>
			<RecurrenceEditor bind:value={form.recurrence} />
			{#if form.recurrence}
				<label class="flex items-center gap-2 text-sm">
					<input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.is_active} />
					Active <span class="text-slate-400">— untick to pause without losing the schedule</span>
				</label>
			{/if}
		</section>

		<div class="flex flex-wrap items-center gap-2">
			<button class="btn btn-primary" disabled={saving}>{saving ? 'Saving…' : 'Save template'}</button>
			{#if !isNew}
				<button type="button" class="btn btn-secondary" onclick={useNow}>Create a task now</button>
				<button type="button" class="btn btn-ghost ml-auto text-red-600 hover:bg-red-50" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>
			{/if}
		</div>
	</form>
	{/if}
</div>
