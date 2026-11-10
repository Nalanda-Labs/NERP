<script lang="ts">
	import { goto } from '$app/navigation';
	import { formatDeadline, PRIORITY, PROJECT_COLORS, STATUS, type TaskSummary } from '#lib/tasks.ts';
	import Avatar from './Avatar.svelte';
	import Icon from './Icon.svelte';

	let { tasks, showProject = true, empty = 'No tasks here.' }: { tasks: TaskSummary[]; showProject?: boolean; empty?: string } = $props();
</script>

<div class="card overflow-hidden">
	<table class="min-w-full text-sm">
		<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
			<tr>
				<th class="px-4 py-3">Task</th>
				<th class="hidden px-4 py-3 sm:table-cell">Status</th>
				<th class="px-4 py-3">Deadline</th>
				<th class="hidden px-4 py-3 md:table-cell">Responsible</th>
				{#if showProject}<th class="hidden px-4 py-3 lg:table-cell">Project</th>{/if}
			</tr>
		</thead>
		<tbody class="divide-y divide-slate-100">
			{#each tasks as t (t.id)}
				<tr class="cursor-pointer hover:bg-slate-50" onclick={() => goto(`/tasks/${t.id}`)}>
					<td class="px-4 py-3">
						<a href="/tasks/{t.id}" class="flex items-start gap-2">
							{#if t.priority === 'high'}<span class="mt-0.5 {PRIORITY.high.class}" title="High priority">▲</span>{/if}
							<span class="min-w-0">
								<span class="block font-medium {t.status === 'done' ? 'text-slate-400 line-through' : 'text-slate-900'}">{t.title}</span>
								<span class="mt-0.5 flex flex-wrap items-center gap-x-3 text-xs text-slate-400">
									<span>#{t.number}</span>
									{#if t.checklist_total}<span>☑ {t.checklist_done}/{t.checklist_total}</span>{/if}
									{#if t.comment_count}<span>💬 {t.comment_count}</span>{/if}
									{#if t.new_comments}<span class="rounded-full bg-brand-600 px-1.5 font-semibold text-white" title="New comments" data-new-comments>{t.new_comments} new</span>{/if}
									{#if t.rating}<span title="Rated">{t.rating === 'good' ? '👍' : '👎'}</span>{/if}
									{#if t.subtask_count}<span>↳ {t.subtask_count}</span>{/if}
									{#if t.file_count}<span>📎 {t.file_count}</span>{/if}
								</span>
							</span>
						</a>
					</td>
					<td class="hidden px-4 py-3 sm:table-cell"><span class="badge {STATUS[t.status].badge}">{STATUS[t.status].label}</span></td>
					<td class="px-4 py-3 whitespace-nowrap {t.is_overdue ? 'font-medium text-red-600' : 'text-slate-600'}">
						{#if t.deadline}{formatDeadline(t.deadline)}{:else}<span class="text-slate-300">—</span>{/if}
					</td>
					<td class="hidden px-4 py-3 md:table-cell">
						<span class="flex items-center gap-2 text-slate-700">
							<Avatar name={t.responsible_name} fileId={t.responsible_avatar_file_id} size="xs" />
							{t.responsible_name}
						</span>
					</td>
					{#if showProject}
						<td class="hidden px-4 py-3 lg:table-cell">
							{#if t.project_name}
								<span class="flex items-center gap-2 text-slate-600">
									<span class="size-2 rounded-full {PROJECT_COLORS[t.project_color ?? 'slate']?.bg}"></span>{t.project_name}
								</span>
							{/if}
						</td>
					{/if}
				</tr>
			{:else}
				<tr>
					<td colspan="5" class="px-4 py-14 text-center text-slate-500">
						<Icon name="check" class="mx-auto mb-2 size-8 text-slate-300" />
						{empty}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
