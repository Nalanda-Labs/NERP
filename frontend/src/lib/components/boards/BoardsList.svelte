<script lang="ts">
	// Whiteboards made inside a task or a workgroup, and a button to make one.
	import { goto } from '$app/navigation';
	import { api, query } from '#lib/api.ts';
	import { formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { taskId = null, projectId = null, canAdd = true, card = true }: { taskId?: string | null; projectId?: string | null; canAdd?: boolean; card?: boolean } = $props();
	let items = $state<{ id: string; title: string; updated_at: string }[]>([]);
	$effect(() => {
		api.get<{ items: typeof items }>('/boards' + query(taskId ? { scope: 'task', task_id: taskId } : { scope: 'project', project_id: projectId ?? undefined })).then((r) => (items = r.items)).catch(() => (items = []));
	});
	async function add() {
		const title = prompt('Name of the board');
		if (!title?.trim()) return;
		try {
			const b = await api.post<{ id: string }>('/boards', { title, task_id: taskId, project_id: projectId });
			goto(`/boards/${b.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<section class={card ? 'card' : ''} data-boards>
	<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
		<h2 class="font-semibold text-slate-900">Boards</h2>
		{#if canAdd}<button class="text-sm font-medium text-brand-600 hover:underline" onclick={add}>New board</button>{/if}
	</div>
	{#if items.length}
		<ul class="divide-y divide-slate-100 text-sm">
			{#each items as b (b.id)}<li class="flex items-center gap-2 px-5 py-2.5"><a class="flex-1 text-slate-900 hover:underline" href="/boards/{b.id}">{b.title}</a><span class="text-xs text-slate-500">{formatRelative(b.updated_at)}</span></li>{/each}
		</ul>
	{:else}
		<p class="px-5 py-3 text-sm text-slate-400">No boards yet.</p>
	{/if}
</section>
