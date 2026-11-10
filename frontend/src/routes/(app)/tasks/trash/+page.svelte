<script lang="ts">
	// Deleted tasks, kept for a while so they can be put back.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDate, formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Binned = { id: string; number: number; title: string; project_name: string | null; deleted_by_name: string | null; deleted_at: string; purge_at: string };
	let items = $state<Binned[]>([]);
	let days = $state(30);
	let q = $state('');
	async function load() {
		const r = await api.get<{ items: Binned[]; days: number }>('/tasks/trash' + query({ q: q.trim() || undefined }));
		items = r.items;
		days = r.days;
	}
	onMount(load);
	async function restore(t: Binned) {
		try {
			await api.post(`/tasks/trash/${t.id}/restore`);
			toasts.success(`#${t.number} is back`);
			goto(`/tasks/${t.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function purge(t: Binned) {
		if (!confirm(`Delete “${t.title}” for good?`)) return;
		await api.del(`/tasks/trash/${t.id}`);
		items = items.filter((x) => x.id !== t.id);
	}
	let timer: ReturnType<typeof setTimeout>;
</script>

<svelte:head><title>Deleted tasks · NERP</title></svelte:head>

<div class="mx-auto max-w-4xl space-y-4">
	<a href="/tasks" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Tasks</a>
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Recycle bin</h1>
		<input class="input w-64" type="search" placeholder="Search…" aria-label="Search deleted tasks" bind:value={q} oninput={() => (clearTimeout(timer), (timer = setTimeout(load, 250)))} />
	</div>
	<p class="text-sm text-slate-500">Tasks you made or deleted stay here for {days} days, with their checklist, comments, files and history, and can be put back.</p>
	<div class="card">
		<ul class="divide-y divide-slate-100">
			{#each items as t (t.id)}
				<li class="flex items-center gap-3 px-4 py-3" data-binned={t.title}>
					<div class="min-w-0 flex-1">
						<p class="truncate text-sm font-medium text-slate-900"><span class="text-slate-400">#{t.number}</span> {t.title}</p>
						<p class="truncate text-xs text-slate-500">{t.project_name ? `${t.project_name} · ` : ''}deleted {t.deleted_by_name ? `by ${t.deleted_by_name} ` : ''}{formatDateTime(t.deleted_at)} · gone for good on {formatDate(t.purge_at)}</p>
					</div>
					<button class="btn btn-secondary px-3 py-1 text-sm" onclick={() => restore(t)}>Restore</button>
					<button class="btn btn-ghost p-1.5 text-slate-400 hover:text-red-600" aria-label="Delete for good" onclick={() => purge(t)}><Icon name="trash" class="size-4" /></button>
				</li>
			{:else}
				<li class="px-4 py-10 text-center text-sm text-slate-500">The bin is empty.</li>
			{/each}
		</ul>
	</div>
</div>
