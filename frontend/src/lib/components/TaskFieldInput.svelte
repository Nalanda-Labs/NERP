<script lang="ts">
	// One custom task field's input, by its kind.
	import type { TaskFieldDef } from '#lib/tasks.ts';
	import UserPicker from './UserPicker.svelte';

	let { field, value = $bindable(null), id }: { field: TaskFieldDef; value?: string | number | boolean | null; id: string } = $props();
	let userValue = $state<string | null>(null);
	let userLabel = $state<string | null>(null);
	$effect(() => {
		if (field.kind === 'user') userValue = (value as string | null) ?? null;
	});
</script>

{#if field.kind === 'bool'}
	<label class="flex items-center gap-2 text-sm"><input {id} type="checkbox" class="rounded border-slate-300 text-brand-600" checked={value === true} onchange={(e) => (value = e.currentTarget.checked)} /> Yes</label>
{:else if field.kind === 'select'}
	<select {id} class="input" required={field.required} value={value ?? ''} onchange={(e) => (value = e.currentTarget.value || null)}>
		<option value="">—</option>
		{#each field.options as o (o)}<option value={o}>{o}</option>{/each}
	</select>
{:else if field.kind === 'number'}
	<input {id} type="number" step="any" class="input" required={field.required} value={value ?? ''} oninput={(e) => (value = e.currentTarget.value === '' ? null : Number(e.currentTarget.value))} />
{:else if field.kind === 'date'}
	<input {id} type="date" class="input" required={field.required} value={value ?? ''} oninput={(e) => (value = e.currentTarget.value || null)} />
{:else if field.kind === 'user'}
	<UserPicker bind:value={userValue} bind:label={userLabel} onpick={(u) => (value = u.id)} />
{:else}
	<input {id} type={field.kind === 'url' ? 'url' : 'text'} class="input" required={field.required} value={value ?? ''} oninput={(e) => (value = e.currentTarget.value || null)} />
{/if}
