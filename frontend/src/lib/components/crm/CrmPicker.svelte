<script lang="ts">
	// Finds CRM records of the given types by name, number, phone or email.
	import { api, query } from '#lib/api.ts';

	type Hit = { id: string; type: string; number: number; title: string; subtitle: string | null };
	let {
		types,
		placeholder = 'Search…',
		exclude = [],
		onpick
	}: { types: string[]; placeholder?: string; exclude?: string[]; onpick: (h: Hit) => void } = $props();

	let term = $state('');
	let results = $state<Hit[]>([]);
	let open = $state(false);
	let timer: ReturnType<typeof setTimeout>;

	function search() {
		clearTimeout(timer);
		if (term.trim().length < 3) {
			results = [];
			return;
		}
		timer = setTimeout(async () => {
			results = (await api.get<Hit[]>('/crm/lookup' + query({ types: types.join(','), q: term }))).filter((h) => !exclude.includes(h.id));
			open = true;
		}, 200);
	}
</script>

<div class="relative">
	<input class="input" {placeholder} bind:value={term} oninput={search} onfocus={search} onblur={() => setTimeout(() => (open = false), 150)} />
	{#if open && results.length}
		<ul class="absolute z-20 mt-1 max-h-64 w-full overflow-auto rounded-xl bg-white py-1 shadow-g2">
			{#each results as h (h.id)}
				<li>
					<button
						type="button"
						class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-slate-50"
						onmousedown={() => {
							onpick(h);
							term = '';
							open = false;
						}}
					>
						<span class="badge bg-slate-100 text-slate-500 capitalize">{h.type}</span>
						<span class="truncate text-slate-800">{h.title}</span>
						{#if h.subtitle}<span class="truncate text-xs text-slate-400">{h.subtitle}</span>{/if}
					</button>
				</li>
			{/each}
		</ul>
	{:else if open && term.trim().length >= 3}
		<p class="absolute z-20 mt-1 w-full rounded-xl bg-white px-3 py-2 text-sm text-slate-500 shadow-g2">Nothing found</p>
	{/if}
</div>
