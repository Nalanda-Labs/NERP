<script lang="ts">
	// CRM settings → Record types → Traffic sources: the company's own sources beside the
	// built-in ones (call, website, web form…). A record that arrives with one of a source's
	// utm_source values, and no source of its own, gets that source.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { crm } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';

	type Source = { id: string; code: string; name: string; utm: string[] };
	let items = $state<Source[]>([]);
	let name = $state('');
	let utm = $state('');
	const set = (r: { items: Source[] }) => {
		items = r.items;
		crm.load(true);
	};
	onMount(async () => (items = (await api.get<{ items: Source[] }>('/crm/traffic-sources')).items));
	const list = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);
	async function add(e: SubmitEvent) {
		e.preventDefault();
		try {
			set(await api.post('/crm/traffic-sources', { name, utm: list(utm) }));
			name = '';
			utm = '';
		} catch (err) {
			toasts.error(err);
		}
	}
	async function change(s: Source, patch: Partial<{ name: string; utm: string[] }>) {
		try {
			set(await api.patch(`/crm/traffic-sources/${s.id}`, patch));
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove(s: Source) {
		if (!confirm(`Remove the source “${s.name}”? Records keep it, shown by its code.`)) return;
		set(await api.del(`/crm/traffic-sources/${s.id}`));
	}
</script>

<section class="card" aria-labelledby="ts-h" data-traffic-sources>
	<div class="border-b border-slate-100 px-5 py-3">
		<h2 id="ts-h" class="font-semibold text-slate-900">Traffic sources</h2>
		<p class="text-xs text-slate-500">Your own sources beside the built-in ones. A record that arrives with one of the utm_source values, and no source of its own, gets the source.</p>
	</div>
	<ul class="divide-y divide-slate-50 text-sm">
		{#each items as s (s.id)}
			<li class="flex flex-wrap items-center gap-2 px-5 py-2" data-source={s.name}>
				<input class="input w-48" value={s.name} aria-label="Source name" onchange={(e) => change(s, { name: e.currentTarget.value })} />
				<input class="input min-w-0 flex-1 font-mono text-xs" value={s.utm.join(', ')} aria-label="utm_source values" placeholder="utm_source values, comma-separated" onchange={(e) => change(s, { utm: list(e.currentTarget.value) })} />
				<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Remove {s.name}" onclick={() => remove(s)}><Icon name="trash" class="size-4" /></button>
			</li>
		{/each}
	</ul>
	<form class="flex flex-wrap gap-2 border-t border-slate-100 px-5 py-3" onsubmit={add}>
		<input class="input w-48" required maxlength="60" placeholder="e.g. Google Ads" aria-label="New source name" bind:value={name} />
		<input class="input min-w-0 flex-1 font-mono text-xs" placeholder="utm_source values, e.g. google, adwords" aria-label="New source utm_source values" bind:value={utm} />
		<button class="btn btn-primary">Add</button>
	</form>
</section>
