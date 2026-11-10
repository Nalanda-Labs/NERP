<script lang="ts">
	// A project's own task fields (in its settings, for its owners).
	import { api } from '#lib/api.ts';
	import { FIELD_KINDS, type TaskFieldDef, type TaskFieldKind } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from './Icon.svelte';

	let { projectId }: { projectId: string } = $props();
	let fields = $state<TaskFieldDef[]>([]);
	let form = $state<{ name: string; kind: TaskFieldKind; options: string; required: boolean }>({ name: '', kind: 'text', options: '', required: false });
	const mine = $derived(fields.filter((f) => f.project_id === projectId));
	$effect(() => {
		api.get<{ items: TaskFieldDef[] }>(`/tasks/fields?project_id=${projectId}`).then((r) => (fields = r.items));
	});
	async function add() {
		if (!form.name.trim()) return;
		try {
			fields = (await api.post<{ items: TaskFieldDef[] }>('/tasks/fields', { ...form, options: form.options.split(','), project_id: projectId })).items;
			form = { name: '', kind: 'text', options: '', required: false };
		} catch (e) {
			toasts.error(e);
		}
	}
	async function remove(f: TaskFieldDef) {
		if (!confirm(`Delete the field “${f.name}” and its values?`)) return;
		fields = (await api.del<{ items: TaskFieldDef[] }>(`/tasks/fields/${f.id}`)).items;
	}
</script>

<div class="space-y-2" aria-label="Task fields">
	<span class="label">Task fields of this project</span>
	<ul class="divide-y divide-slate-100 rounded-lg border border-slate-200">
		{#each mine as f (f.id)}
			<li class="flex items-center gap-2 px-3 py-1.5 text-sm" data-project-field={f.name}>
				<span class="flex-1">{f.name}{f.required ? ' *' : ''}</span>
				<span class="text-xs text-slate-500">{FIELD_KINDS[f.kind]}</span>
				<button type="button" class="text-slate-400 hover:text-red-600" aria-label="Delete {f.name}" onclick={() => remove(f)}><Icon name="x" class="size-4" /></button>
			</li>
		{:else}
			<li class="px-3 py-2 text-xs text-slate-400">None yet.</li>
		{/each}
	</ul>
	<div class="flex flex-wrap gap-2">
		<input class="input min-w-32 flex-1" placeholder="Field name" aria-label="New field name" bind:value={form.name} />
		<select class="input w-32" aria-label="New field kind" bind:value={form.kind}>{#each Object.entries(FIELD_KINDS) as [k, l] (k)}<option value={k}>{l}</option>{/each}</select>
		{#if form.kind === 'select'}<input class="input w-48" placeholder="Choices, comma separated" aria-label="Choices" bind:value={form.options} />{/if}
		<label class="flex items-center gap-1 text-xs"><input type="checkbox" class="rounded border-slate-300" bind:checked={form.required} /> Required</label>
		<button type="button" class="btn btn-secondary" onclick={add}>Add field</button>
	</div>
</div>
