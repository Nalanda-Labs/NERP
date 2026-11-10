<script lang="ts">
	// A task's custom fields in its sidebar: shown, and changed in place by those who edit it.
	import { api } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import type { TaskDetail, TaskField } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import TaskFieldInput from './TaskFieldInput.svelte';

	let { task = $bindable() }: { task: TaskDetail } = $props();
	let editing = $state(false);
	let values = $state<Record<string, string | number | boolean | null>>({});
	function start() {
		values = Object.fromEntries(task.fields.map((f) => [f.id, f.value]));
		editing = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			task = await api.put<TaskDetail>(`/tasks/${task.id}/fields`, { values });
			editing = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	const show = (f: TaskField) =>
		f.value === null || f.value === '' ? '—' : f.kind === 'bool' ? (f.value ? 'Yes' : 'No') : f.kind === 'date' ? formatDate(String(f.value)) : f.kind === 'user' ? 'Employee' : String(f.value);
</script>

{#if task.fields.length}
	<section class="card text-sm" aria-label="Fields">
		<div class="flex items-center border-b border-slate-100 px-5 py-2.5">
			<h2 class="flex-1 text-xs font-semibold tracking-wide text-slate-500 uppercase">Fields</h2>
			{#if task.access.edit && !editing}<button class="text-xs text-brand-600 hover:underline" onclick={start}>Edit</button>{/if}
		</div>
		{#if editing}
			<form class="space-y-3 px-5 py-3" onsubmit={save}>
				{#each task.fields as f (f.id)}
					<div>
						<label class="label" for="tfc-{f.id}">{f.name}{f.required ? ' *' : ''}</label>
						<TaskFieldInput field={f} id="tfc-{f.id}" bind:value={values[f.id]} />
					</div>
				{/each}
				<div class="flex gap-2"><button class="btn btn-primary">Save</button><button type="button" class="btn btn-ghost" onclick={() => (editing = false)}>Cancel</button></div>
			</form>
		{:else}
			<dl class="divide-y divide-slate-100">
				{#each task.fields as f (f.id)}
					<div class="flex gap-3 px-5 py-2" data-field={f.name}>
						<dt class="w-28 shrink-0 text-xs text-slate-500">{f.name}</dt>
						<dd class="min-w-0 flex-1 break-words text-slate-800">
							{#if f.kind === 'url' && f.value}<a class="text-brand-600 hover:underline" href={String(f.value)} target="_blank" rel="noopener noreferrer">{f.value}</a>
							{:else if f.kind === 'user' && f.value}<a class="text-brand-600 hover:underline" href="/employees/{f.value}">Profile</a>
							{:else}{show(f)}{/if}
						</dd>
					</div>
				{/each}
			</dl>
		{/if}
	</section>
{/if}
