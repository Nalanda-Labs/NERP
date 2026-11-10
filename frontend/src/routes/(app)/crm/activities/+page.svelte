<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { crm, itemLink, type Activity } from '#lib/crm.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { realtime } from '#lib/realtime.svelte.ts';

	let tab = $state<'open' | 'overdue' | 'today' | 'completed'>('open');
	let items = $state<Activity[]>([]);
	let total = $state(0);
	const ICON: Record<string, string> = { todo: 'checkSquare', call: 'phone', meeting: 'calendar', email: 'mail' };

	async function load() {
		const p = await api.get<{ items: Activity[]; total: number }>(`/crm/activities?mine=true&state=${tab}&limit=200`);
		items = p.items;
		total = p.total;
		crm.loadCounters();
	}
	onMount(() => {
		load();
		let t: ReturnType<typeof setTimeout>;
		return realtime.on('crm.changed', () => (clearTimeout(t), (t = setTimeout(load, 500))));
	});
	async function toggle(a: Activity) {
		await api.post(`/crm/activities/${a.id}/complete`, { reopen: a.is_completed });
		load();
	}
</script>

<svelte:head><title>My activities · NERP</title></svelte:head>

<div class="max-w-4xl space-y-4">
	<h1 class="text-2xl font-semibold text-slate-900">My activities</h1>
	<nav class="flex gap-1">
		{#each [['open', 'Planned'], ['overdue', `Overdue${crm.counters?.activities_overdue ? ` (${crm.counters.activities_overdue})` : ''}`], ['today', 'Today'], ['completed', 'Done']] as [k, label] (k)}
			<button class="btn {tab === k ? 'bg-brand-100 text-brand-900' : 'btn-ghost'}" onclick={() => ((tab = k as typeof tab), load())}>{label}</button>
		{/each}
	</nav>
	<ul class="card divide-y divide-slate-50">
		{#each items as a (a.id)}
			<li class="flex items-start gap-3 px-5 py-3 text-sm">
				<input type="checkbox" class="mt-0.5 rounded border-slate-300 text-emerald-600" checked={a.is_completed} aria-label="Done" onchange={() => toggle(a)} />
				<Icon name={ICON[a.kind] as 'phone'} class="mt-0.5 size-4 text-slate-400" />
				<div class="min-w-0 flex-1">
					<p class="{a.is_completed ? 'text-slate-400 line-through' : 'text-slate-900'}">{a.subject}</p>
					<a class="text-xs text-brand-600 hover:underline" href={itemLink(a.item_type, a.item_id)}>{a.item_title}</a>
				</div>
				<span class="text-xs whitespace-nowrap {a.is_overdue ? 'font-medium text-red-600' : 'text-slate-500'}">{a.deadline ? formatDateTime(a.deadline) : 'No deadline'}</span>
			</li>
		{:else}
			<li class="px-5 py-14 text-center text-slate-500">Nothing here.</li>
		{/each}
	</ul>
	{#if total > items.length}<p class="text-sm text-slate-500">Showing {items.length} of {total}.</p>{/if}
</div>
