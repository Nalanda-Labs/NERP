<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { typeName } from '#lib/crm.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Deleted = { id: string; entity_type: number; number: number; title: string; deleted_at: string; deleted_by_name: string | null };
	let items = $state<Deleted[]>([]);
	let days = $state(30);
	async function load() {
		const r = await api.get<{ page: { items: Deleted[] }; retention_days: number }>('/crm/recycle-bin?limit=200');
		items = r.page.items;
		days = r.retention_days;
	}
	onMount(load);
	async function restore(d: Deleted) {
		const r = await api.post<{ type: string }>(`/crm/recycle-bin/${d.id}/restore`);
		toasts.success('Restored');
		goto(`/crm/${r.type}/${d.id}`);
	}
	async function purge(d: Deleted) {
		if (!confirm(`Permanently delete "${d.title}"? This cannot be undone.`)) return;
		await api.del(`/crm/recycle-bin/${d.id}`);
		load();
	}
</script>

<svelte:head><title>CRM recycle bin · NERP</title></svelte:head>

<div class="max-w-4xl space-y-4">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">Recycle bin</h1>
		<p class="text-sm text-slate-500">Deleted CRM records are kept for {days} days, then removed permanently.</p>
	</div>
	<ul class="card divide-y divide-slate-50">
		{#each items as d (d.id)}
			<li class="flex flex-wrap items-center gap-3 px-5 py-3 text-sm">
				<span class="badge bg-slate-100 text-slate-500">{typeName(d.entity_type)}</span>
				<span class="min-w-0 flex-1 truncate text-slate-800">#{d.number} {d.title}</span>
				<span class="text-xs text-slate-500">{formatDateTime(d.deleted_at)}{d.deleted_by_name ? ` · ${d.deleted_by_name}` : ''}</span>
				<button class="btn btn-secondary" onclick={() => restore(d)}>Restore</button>
				<button class="btn btn-ghost text-red-600" onclick={() => purge(d)}>Delete forever</button>
			</li>
		{:else}
			<li class="px-5 py-14 text-center text-slate-500">The recycle bin is empty.</li>
		{/each}
	</ul>
</div>
