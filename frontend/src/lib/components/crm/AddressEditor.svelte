<script lang="ts">
	import { untrack } from 'svelte';
	import { allCountries, type Address } from '#lib/crm.svelte.ts';

	let { value = $bindable<Address | null>(null) }: { value?: Address | null } = $props();
	const countries = allCountries();
	let a = $state<Address>(untrack(() => ({ ...(value ?? {}) })));
	$effect(() => {
		const clean = Object.fromEntries(Object.entries(a).filter(([, v]) => v && String(v).trim())) as Address;
		value = Object.keys(clean).length ? clean : null;
	});
</script>

<div class="grid gap-2 sm:grid-cols-2">
	<input class="input sm:col-span-2" placeholder="Street address" bind:value={a.line1} aria-label="Street address" />
	<input class="input sm:col-span-2" placeholder="Apartment, suite, building (optional)" bind:value={a.line2} aria-label="Address line 2" />
	<input class="input" placeholder="City" bind:value={a.city} aria-label="City" />
	<input class="input" placeholder="State / province / region" bind:value={a.region} aria-label="Region" />
	<input class="input" placeholder="Postal code" bind:value={a.postal_code} aria-label="Postal code" />
	<select class="input" bind:value={a.country} aria-label="Country">
		<option value={undefined}>Country…</option>
		{#each countries as c (c.code)}<option value={c.code}>{c.name}</option>{/each}
	</select>
</div>
