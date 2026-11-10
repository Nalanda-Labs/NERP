<script lang="ts">
	// Who reads and who edits a knowledge base space: people and departments.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';

	type Who = { users: { id: string; name: string; avatar_file_id: string | null }[]; departments: string[] };
	let {
		toAll = $bindable(true),
		readers = $bindable(),
		editors = $bindable(),
		hint = 'Whoever made the space and people who manage the knowledge base can always edit it.'
	}: { toAll?: boolean; readers: Who; editors: Who; hint?: string } = $props();
	let departments = $state<{ id: string; name: string }[]>([]);
	onMount(() => {
		api.get<{ id: string; name: string }[]>('/departments').then((d) => (departments = d)).catch(() => {});
	});
	function toggle(w: Who, id: string) {
		w.departments = w.departments.includes(id) ? w.departments.filter((x) => x !== id) : [...w.departments, id];
	}
</script>

<div class="space-y-4">
	<div>
		<span class="label">Who reads it</span>
		<select class="input" aria-label="Who reads it" bind:value={toAll}><option value={true}>Everyone</option><option value={false}>Chosen people and departments</option></select>
		{#if !toAll}
			<div class="mt-2 space-y-2 rounded-xl bg-slate-50 p-3">
				<MultiUserPicker bind:value={readers.users} placeholder="People…" />
				<div class="flex flex-wrap gap-1.5">{#each departments as d (d.id)}<button type="button" class="rounded-full border px-3 py-1 text-sm {readers.departments.includes(d.id) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600'}" aria-pressed={readers.departments.includes(d.id)} onclick={() => toggle(readers, d.id)}>{d.name}</button>{/each}</div>
			</div>
		{/if}
	</div>
	<div>
		<span class="label">Who edits it</span>
		<div class="space-y-2 rounded-xl bg-slate-50 p-3">
			<MultiUserPicker bind:value={editors.users} placeholder="People…" />
			<div class="flex flex-wrap gap-1.5">{#each departments as d (d.id)}<button type="button" class="rounded-full border px-3 py-1 text-sm {editors.departments.includes(d.id) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600'}" aria-pressed={editors.departments.includes(d.id)} onclick={() => toggle(editors, d.id)}>{d.name}</button>{/each}</div>
			<p class="text-xs text-slate-500">{hint}</p>
		</div>
	</div>
</div>
