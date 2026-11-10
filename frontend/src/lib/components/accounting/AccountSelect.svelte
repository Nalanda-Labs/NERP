<script lang="ts">
	// Picks an account from the chart, optionally only some kinds.
	import { accounts, type Account } from '#lib/accounting.ts';

	let {
		value = $bindable(''),
		subtypes = null,
		types = null,
		id = undefined,
		label = undefined,
		required = false,
		placeholder = 'Choose an account…',
		onchange
	}: {
		value?: string | null;
		subtypes?: string[] | null;
		types?: Account['type'][] | null;
		id?: string;
		label?: string;
		required?: boolean;
		placeholder?: string;
		onchange?: (v: string) => void;
	} = $props();

	let all = $state<Account[]>([]);
	$effect(() => {
		accounts().then((a) => (all = a));
	});
	const shown = $derived(all.filter((a) => (a.is_active || a.id === value) && (!subtypes || subtypes.includes(a.subtype)) && (!types || types.includes(a.type))));
</script>

<select {id} class="input" aria-label={label} {required} bind:value onchange={() => onchange?.(value ?? '')}>
	<option value="">{placeholder}</option>
	{#each shown as a (a.id)}<option value={a.id}>{a.code} · {a.name}{a.currency ? ` (${a.currency})` : ''}</option>{/each}
</select>
