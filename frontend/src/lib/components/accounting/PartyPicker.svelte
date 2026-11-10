<script lang="ts">
	// Picks a customer or supplier: a CRM company or contact.
	import { api, query } from '#lib/api.ts';
	import type { Party } from '#lib/accounting.ts';
	import Icon from '#lib/components/Icon.svelte';

	let {
		value = $bindable(null),
		label = 'Customer or supplier',
		placeholder = 'Search companies and contacts…',
		onpick
	}: { value?: Party | null; label?: string; placeholder?: string; onpick?: (p: Party | null) => void } = $props();

	let q = $state('');
	let hits = $state<Party[]>([]);
	let open = $state(false);
	let t: ReturnType<typeof setTimeout> | undefined;
	function search() {
		clearTimeout(t);
		t = setTimeout(async () => {
			hits = (await api.get<{ items: Party[] }>('/accounting/parties' + query({ q }))).items;
			open = true;
		}, 200);
	}
	function pick(p: Party | null) {
		value = p;
		open = false;
		q = '';
		onpick?.(p);
	}
</script>

{#if value}
	<div class="flex items-center gap-2 rounded-lg border border-slate-200 px-3 py-2 text-sm" data-party={value.name}>
		<Icon name={value.kind === 'company' ? 'building' : 'user'} class="size-4 text-slate-400" />
		<span class="flex-1 truncate">{value.name}</span>
		<button type="button" class="text-slate-400 hover:text-slate-700" aria-label="Change {label.toLowerCase()}" onclick={() => pick(null)}><Icon name="x" class="size-4" /></button>
	</div>
{:else}
	<div class="relative">
		<input class="input" aria-label={label} {placeholder} bind:value={q} oninput={search} onfocus={search} onblur={() => setTimeout(() => (open = false), 200)} />
		{#if open && hits.length}
			<ul class="absolute z-20 mt-1 max-h-64 w-full overflow-auto rounded-lg border border-slate-200 bg-white py-1 shadow-lg">
				{#each hits as h (h.id)}
					<li><button type="button" class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm hover:bg-slate-50" onmousedown={() => pick(h)}><Icon name={h.kind === 'company' ? 'building' : 'user'} class="size-4 text-slate-400" />{h.name}</button></li>
				{/each}
			</ul>
		{/if}
	</div>
{/if}
