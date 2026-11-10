<script lang="ts">
	// Custom task fields for the whole company (administrators), with each project's listed too.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { FIELD_KINDS, type TaskFieldDef, type TaskFieldKind } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let fields = $state<TaskFieldDef[]>([]);
	const load = async () => (fields = (await api.get<{ items: TaskFieldDef[] }>('/tasks/fields?all=true')).items);
	onMount(load);
	let open = $state(false);
	let form = $state<{ id: string | null; name: string; kind: TaskFieldKind; options: string; required: boolean; show_in_list: boolean; project_id: string | null }>({ id: null, name: '', kind: 'text', options: '', required: false, show_in_list: false, project_id: null });
	function edit(f: TaskFieldDef | null) {
		form = f ? { id: f.id, name: f.name, kind: f.kind, options: f.options.join('\n'), required: f.required, show_in_list: f.show_in_list, project_id: f.project_id } : { id: null, name: '', kind: 'text', options: '', required: false, show_in_list: false, project_id: null };
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		const body = { name: form.name, kind: form.kind, options: form.options.split('\n'), required: form.required, show_in_list: form.show_in_list };
		try {
			if (form.id) await api.patch(`/tasks/fields/${form.id}`, body);
			else await api.post('/tasks/fields', body);
			open = false;
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove() {
		if (!form.id || !confirm('Delete this field and every value in it?')) return;
		await api.del(`/tasks/fields/${form.id}`);
		open = false;
		load();
	}
</script>

<svelte:head><title>Task fields · NERP</title></svelte:head>

<div class="mx-auto max-w-3xl space-y-4">
	<a href="/tasks" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Tasks</a>
	<div class="flex items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Task fields</h1>
		<button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New field</button>
	</div>
	<p class="text-sm text-slate-500">Fields here are on every task. A project's owners add fields for its tasks in the project's settings.</p>
	<div class="card">
		<ul class="divide-y divide-slate-100">
			{#each fields as f (f.id)}
				<li>
					<button class="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-slate-50 disabled:cursor-default" disabled={!!f.project_id} onclick={() => edit(f)} data-def={f.name}>
						<span class="flex-1 font-medium text-slate-900">{f.name}{f.required ? ' *' : ''}</span>
						<span class="text-xs text-slate-500">{FIELD_KINDS[f.kind]}{f.kind === 'select' ? `: ${f.options.join(', ')}` : ''}</span>
						<span class="badge bg-slate-100 text-slate-600">{f.project_name ?? 'Every task'}</span>
					</button>
				</li>
			{:else}
				<li class="px-4 py-10 text-center text-sm text-slate-500">No custom fields yet.</li>
			{/each}
		</ul>
	</div>
</div>

<Modal bind:open title={form.id ? 'Field' : 'New field'}>
	<form id="field-form" class="space-y-3" onsubmit={save}>
		<div><label class="label" for="fd-n">Name</label><input id="fd-n" class="input" required maxlength="100" bind:value={form.name} /></div>
		<div>
			<label class="label" for="fd-k">Kind</label>
			<select id="fd-k" class="input" bind:value={form.kind} disabled={!!form.id}>
				{#each Object.entries(FIELD_KINDS) as [k, l] (k)}<option value={k}>{l}</option>{/each}
			</select>
		</div>
		{#if form.kind === 'select'}
			<div><label class="label" for="fd-o">Choices <span class="font-normal text-slate-400">— one per line</span></label><textarea id="fd-o" class="input" rows="4" bind:value={form.options}></textarea></div>
		{/if}
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.required} /> Required when a task is made or edited</label>
		{#if form.id}<button type="button" class="text-sm text-red-600 hover:underline" onclick={remove}>Delete field</button>{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="field-form">Save</button>
	{/snippet}
</Modal>
