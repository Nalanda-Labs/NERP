<script lang="ts">
	import { api, query } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import { fromLocalInput, toLocalInput, type Priority, type ProjectSummary, type TaskDetail, type TaskFieldDef } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Modal from './Modal.svelte';
	import MultiUserPicker, { type PickedUser } from './MultiUserPicker.svelte';
	import TaskFieldInput from './TaskFieldInput.svelte';
	import UserPicker from './UserPicker.svelte';

	let {
		open = $bindable(false),
		task = null,
		projectId = null,
		parentId = null,
		onsaved
	}: {
		open?: boolean;
		/** Edit this task; omit to create a new one. */
		task?: TaskDetail | null;
		projectId?: string | null;
		parentId?: string | null;
		onsaved?: (t: TaskDetail) => void;
	} = $props();

	let projects = $state<ProjectSummary[]>([]);
	let saving = $state(false);
	let more = $state(false);

	type Form = {
		title: string;
		description: string;
		responsible_id: string | null;
		responsible_name: string | null;
		assistants: PickedUser[];
		watchers: PickedUser[];
		deadline: string;
		start_date: string;
		priority: Priority;
		project_id: string;
		requires_review: boolean;
		estimate_hours: string;
		checklist: string;
		requires_result: boolean;
		skip_nonworking: boolean;
		fields: Record<string, string | number | boolean | null>;
	};
	// Custom fields that apply to the chosen project.
	let defs = $state<TaskFieldDef[]>([]);
	$effect(() => {
		if (!open) return;
		const p = form.project_id;
		api.get<{ items: TaskFieldDef[] }>('/tasks/fields' + query({ project_id: p || undefined }))
			.then((r) => {
				// Every field gets a value to bind to (empty to start with).
				for (const d of r.items) if (!(d.id in form.fields)) form.fields[d.id] = null;
				defs = r.items;
			})
			.catch(() => (defs = []));
	});
	let form = $state<Form>(blank());

	function blank(): Form {
		const me = session.user!;
		return {
			title: '',
			description: '',
			responsible_id: me.id,
			responsible_name: me.full_name,
			assistants: [],
			watchers: [],
			deadline: '',
			start_date: '',
			priority: 'normal',
			project_id: projectId ?? '',
			requires_review: false,
			estimate_hours: '',
			checklist: '',
			requires_result: false,
			skip_nonworking: false,
			fields: {}
		};
	}

	function fromTask(t: TaskDetail): Form {
		const pick = (role: string) =>
			t.people.filter((p) => p.role === role).map((p) => ({ id: p.user_id, name: p.full_name, avatar_file_id: p.avatar_file_id }));
		return {
			title: t.title,
			description: t.description ?? '',
			responsible_id: t.responsible_id,
			responsible_name: t.responsible_name,
			assistants: pick('assistant'),
			watchers: pick('watcher'),
			deadline: toLocalInput(t.deadline),
			start_date: toLocalInput(t.start_date),
			priority: t.priority,
			project_id: t.project_id ?? '',
			requires_review: t.requires_review,
			estimate_hours: t.time_estimate_secs ? String(+(t.time_estimate_secs / 3600).toFixed(2)) : '',
			checklist: '',
			requires_result: t.requires_result,
			skip_nonworking: t.skip_nonworking,
			fields: Object.fromEntries(t.fields.map((f) => [f.id, f.value]))
		};
	}

	// Reset the form every time the modal opens.
	let wasOpen = false;
	$effect(() => {
		if (open && !wasOpen) {
			form = task ? fromTask(task) : blank();
			more = !!task && (form.assistants.length > 0 || form.watchers.length > 0 || form.requires_review || form.requires_result || form.skip_nonworking || !!form.estimate_hours || !!form.start_date);
			api.get<ProjectSummary[]>('/projects?mine=true').then((p) => (projects = p.filter((x) => x.my_role !== 'viewer')));
		}
		wasOpen = open;
	});

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!form.responsible_id) {
			toasts.error(new Error('Choose who is responsible'));
			return;
		}
		saving = true;
		const hours = parseFloat(form.estimate_hours);
		const body = {
			title: form.title,
			description: form.description || null,
			responsible_id: form.responsible_id,
			assistant_ids: form.assistants.map((u) => u.id),
			watcher_ids: form.watchers.map((u) => u.id),
			deadline: fromLocalInput(form.deadline),
			start_date: fromLocalInput(form.start_date),
			priority: form.priority,
			project_id: form.project_id || null,
			requires_review: form.requires_review,
			time_estimate_secs: Number.isFinite(hours) && hours > 0 ? Math.round(hours * 3600) : null,
			requires_result: form.requires_result,
			skip_nonworking: form.skip_nonworking,
			// Only the fields of the chosen project (and company-wide ones) are sent.
			...(defs.length ? { fields: Object.fromEntries(defs.map((d) => [d.id, form.fields[d.id] ?? null])) } : {})
		};
		try {
			const saved = task
				? await api.patch<TaskDetail>(`/tasks/${task.id}`, body)
				: await api.post<TaskDetail>('/tasks', {
						...body,
						parent_id: parentId,
						checklist: form.checklist.split('\n').map((s) => s.trim()).filter(Boolean)
					});
			toasts.success(task ? 'Task saved' : `Task #${saved.number} created`);
			open = false;
			onsaved?.(saved);
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	function quickDeadline(days: number) {
		const d = new Date();
		d.setDate(d.getDate() + days);
		d.setHours(18, 0, 0, 0);
		form.deadline = toLocalInput(d.toISOString());
	}
</script>

<Modal bind:open title={task ? `Edit task #${task.number}` : parentId ? 'New subtask' : 'New task'} size="lg">
	<form id="task-form" class="space-y-4" onsubmit={submit}>
		<!-- svelte-ignore a11y_autofocus -->
		<input class="input text-base font-medium" placeholder="What needs to be done?" required maxlength="500" autofocus bind:value={form.title} />
		<textarea class="input" rows="4" placeholder="Add details (optional)" bind:value={form.description}></textarea>

		<div class="grid gap-4 sm:grid-cols-2">
			<div>
				<span class="label">Responsible</span>
				<UserPicker bind:value={form.responsible_id} bind:label={form.responsible_name} />
			</div>
			<div>
				<label class="label" for="tf-deadline">Deadline</label>
				<input id="tf-deadline" type="datetime-local" class="input" bind:value={form.deadline} />
				<div class="mt-1 flex gap-2 text-xs">
					<button type="button" class="text-brand-600 hover:underline" onclick={() => quickDeadline(0)}>Today</button>
					<button type="button" class="text-brand-600 hover:underline" onclick={() => quickDeadline(1)}>Tomorrow</button>
					<button type="button" class="text-brand-600 hover:underline" onclick={() => quickDeadline(7)}>In a week</button>
					{#if form.deadline}<button type="button" class="text-slate-500 hover:underline" onclick={() => (form.deadline = '')}>Clear</button>{/if}
				</div>
			</div>
			<div>
				<label class="label" for="tf-project">Project</label>
				<select id="tf-project" class="input" bind:value={form.project_id}>
					<option value="">No project</option>
					{#each projects as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
					{#if task?.project_id && !projects.some((p) => p.id === task.project_id)}<option value={task.project_id}>{task.project_name}</option>{/if}
				</select>
			</div>
			<div>
				<label class="label" for="tf-priority">Priority</label>
				<select id="tf-priority" class="input" bind:value={form.priority}>
					<option value="high">High</option>
					<option value="normal">Normal</option>
					<option value="low">Low</option>
				</select>
			</div>
		</div>

		{#if defs.length}
			<div class="grid gap-4 sm:grid-cols-2" aria-label="Fields">
				{#each defs as d (d.id)}
					<div>
						<label class="label" for="tf-field-{d.id}">{d.name}{d.required ? ' *' : ''}</label>
						<TaskFieldInput field={d} id="tf-field-{d.id}" bind:value={form.fields[d.id]} />
					</div>
				{/each}
			</div>
		{/if}

		{#if !task}
			<div>
				<label class="label" for="tf-checklist">Checklist <span class="font-normal text-slate-400">— one item per line</span></label>
				<textarea id="tf-checklist" class="input font-mono text-xs" rows="2" bind:value={form.checklist}></textarea>
			</div>
		{/if}

		{#if !more}
			<button type="button" class="text-sm font-medium text-brand-600 hover:underline" onclick={() => (more = true)}>More options: start date, assistants, watchers, review, result, working days, estimate</button>
		{:else}
			<div class="grid gap-4 border-t border-slate-100 pt-4 sm:grid-cols-2">
				<div class="sm:col-span-2">
					<label class="label" for="tf-start">Start date <span class="font-normal text-slate-400">— used by the Gantt chart</span></label>
					<input id="tf-start" type="datetime-local" class="input sm:w-1/2" bind:value={form.start_date} />
				</div>
				<div>
					<span class="label">Assistants</span>
					<MultiUserPicker bind:value={form.assistants} exclude={form.responsible_id ? [form.responsible_id] : []} placeholder="Add assistants…" />
				</div>
				<div>
					<span class="label">Watchers</span>
					<MultiUserPicker bind:value={form.watchers} placeholder="Add watchers…" />
				</div>
				<label class="flex items-start gap-2 text-sm sm:col-span-1">
					<input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={form.requires_review} />
					<span><span class="font-medium text-slate-800">Review when done</span><br /><span class="text-slate-500">The creator accepts the work before the task closes.</span></span>
				</label>
				<label class="flex items-start gap-2 text-sm">
					<input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={form.requires_result} />
					<span><span class="font-medium text-slate-800">Needs a result</span><br /><span class="text-slate-500">A comment must be marked as the result before it's finished.</span></span>
				</label>
				<label class="flex items-start gap-2 text-sm">
					<input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={form.skip_nonworking} />
					<span><span class="font-medium text-slate-800">Skip weekends and holidays</span><br /><span class="text-slate-500">Dates that fall on them move to the next working day.</span></span>
				</label>
				<div>
					<label class="label" for="tf-est">Estimate (hours)</label>
					<input id="tf-est" type="number" min="0" step="0.25" class="input w-32" bind:value={form.estimate_hours} />
				</div>
			</div>
		{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="task-form" disabled={saving}>{saving ? 'Saving…' : task ? 'Save' : 'Create task'}</button>
	{/snippet}
</Modal>
