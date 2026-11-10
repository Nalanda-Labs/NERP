<script lang="ts">
	import { session } from '#lib/session.svelte.ts';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import type { TaskDetail, TaskTemplate } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let templates = $state<TaskTemplate[] | null>(null);
	onMount(async () => (templates = await api.get<TaskTemplate[]>('/task-templates')));

	async function use(t: TaskTemplate) {
		try {
			const task = await api.post<TaskDetail>(`/task-templates/${t.id}/use`);
			toasts.success(`Task #${task.number} created`);
			goto(`/tasks/${task.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Task templates · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<a href="/tasks" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Tasks</a>
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Templates & recurring tasks</h1>
			<p class="text-sm text-slate-500">Reuse tasks you create often, or have NERP create them on a schedule.</p>
		</div>
		<a href="/tasks/templates/new" class="btn btn-primary"><Icon name="plus" class="size-4" /> New template</a>
	</div>

	{#if templates && templates.length}
		<ul class="card divide-y divide-slate-100">
			{#each templates as t (t.id)}
				<li class="flex flex-wrap items-center gap-4 px-5 py-4">
					<span class="flex size-10 shrink-0 items-center justify-center rounded-lg {t.recurrence ? 'bg-violet-50 text-violet-600' : 'bg-slate-100 text-slate-500'}">
						<Icon name={t.recurrence ? 'refresh' : 'file'} />
					</span>
					<a href="/tasks/templates/{t.id}" class="min-w-0 flex-1">
						<span class="block truncate font-medium text-slate-900 hover:text-brand-700">{t.title}</span>
						<span class="block text-sm text-slate-500">
							{#if t.recurrence}
								{t.schedule} ({t.recurrence.timezone})
								{#if t.is_active && t.next_run_at}· next {formatDateTime(t.next_run_at)}{:else}· <span class="text-amber-600">paused</span>{/if}
							{:else}
								Template{t.project_name ? ` · ${t.project_name}` : ''}
							{/if}
							· {t.task_count} {t.task_count === 1 ? 'task' : 'tasks'} created
							{#if t.subtasks.length}· {t.subtasks.length} {t.subtasks.length === 1 ? 'subtask' : 'subtasks'}{/if}
							{#if t.shared}· <span class="text-brand-600" data-shared>shared{t.created_by !== session.user?.id && t.created_by_name ? ` by ${t.created_by_name}` : ''}</span>{/if}
						</span>
					</a>
					<button class="btn btn-secondary" onclick={() => use(t)}><Icon name="plus" class="size-4" /> Create task now</button>
				</li>
			{/each}
		</ul>
	{:else if templates}
		<div class="card flex flex-col items-center gap-2 p-14 text-center">
			<Icon name="refresh" class="size-8 text-slate-300" />
			<p class="font-medium text-slate-700">No templates yet</p>
			<p class="max-w-md text-sm text-slate-500">Create one here, or open any task and choose <em>Save as template</em>.</p>
		</div>
	{/if}
</div>
