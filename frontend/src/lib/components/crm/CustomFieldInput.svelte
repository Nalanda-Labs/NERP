<script lang="ts">
	// An input for one custom field, whatever its type.
	import { untrack } from 'svelte';
	import { crm, type FieldDef } from '#lib/crm.svelte.ts';
	import AddressEditor from './AddressEditor.svelte';
	import UserPicker from '../UserPicker.svelte';

	let { def, value = $bindable<unknown>(null) }: { def: FieldDef; value?: unknown } = $props();
	const id = $derived(`uf-${def.code}`);
	let userLabel = $state<string | null>(null);
	let money = $state<{ amount: string; currency: string }>(
		untrack(() => (value as { amount: string; currency: string }) ?? { amount: '', currency: crm.config?.base_currency ?? 'USD' })
	);
	$effect(() => {
		if (def.kind === 'money') value = money.amount ? { ...money } : null;
	});
</script>

{#if def.kind === 'text'}
	<textarea {id} class="input" rows="3" bind:value={value as string}></textarea>
{:else if def.kind === 'number'}
	<input {id} class="input" type="number" step="any" bind:value={value as number} />
{:else if def.kind === 'date'}
	<input {id} class="input" type="date" bind:value={value as string} />
{:else if def.kind === 'datetime'}
	<input {id} class="input" type="datetime-local" value={value ? String(value).slice(0, 16) : ''} onchange={(e) => (value = e.currentTarget.value ? new Date(e.currentTarget.value).toISOString() : null)} />
{:else if def.kind === 'boolean'}
	<label class="flex h-10 items-center gap-2 text-sm"><input {id} type="checkbox" class="rounded border-slate-300 text-brand-600" checked={!!value} onchange={(e) => (value = e.currentTarget.checked)} /> Yes</label>
{:else if def.kind === 'list'}
	<select {id} class="input" bind:value>
		<option value={null}>—</option>
		{#each def.settings.options ?? [] as o (o.id)}<option value={o.id}>{o.label}</option>{/each}
	</select>
{:else if def.kind === 'money'}
	<div class="flex gap-2">
		<input {id} class="input" type="number" step="0.01" min="0" bind:value={money.amount} />
		<select class="input w-28" bind:value={money.currency} aria-label="Currency">
			{#each crm.config?.currencies ?? [] as c (c.code)}<option>{c.code}</option>{/each}
		</select>
	</div>
{:else if def.kind === 'user'}
	<UserPicker bind:value={value as string | null} bind:label={userLabel} placeholder="Choose a person…" />
{:else if def.kind === 'url'}
	<input {id} class="input" type="url" placeholder="https://" bind:value={value as string} />
{:else if def.kind === 'address'}
	<AddressEditor bind:value={value as never} />
{:else}
	<input {id} class="input" bind:value={value as string} />
{/if}
