<script lang="ts">
	// A workgroup's calendar: what is coming up in the next weeks.
	import { api, query } from '#lib/api.ts';
	import EventEditor from '#lib/components/EventEditor.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDate } from '#lib/format.ts';

	let { projectId, canAdd }: { projectId: string; canAdd: boolean } = $props();

	type Item = { id: string; title: string; start: string; end: string; all_day: boolean; location: string | null; occurrence: string | null; busy: boolean };
	let items = $state<Item[] | null>(null);
	let weeks = $state(8);
	let editorOpen = $state(false);
	let eventId = $state<string | null>(null);
	let occurrence = $state<string | null>(null);

	async function load() {
		const from = new Date();
		from.setHours(0, 0, 0, 0);
		const to = new Date(from.getTime() + weeks * 7 * 86400000);
		items = (await api.get<{ items: Item[] }>('/calendar' + query({ from: from.toISOString(), to: to.toISOString(), project_id: projectId }))).items;
	}
	$effect(() => {
		weeks;
		load();
	});
	const time = (i: Item) =>
		i.all_day ? 'All day' : `${new Date(i.start).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })} – ${new Date(i.end).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
	const days = $derived.by(() => {
		const out: { day: string; items: Item[] }[] = [];
		for (const i of items ?? []) {
			const d = formatDate(i.start);
			const last = out.at(-1);
			if (last?.day === d) last.items.push(i);
			else out.push({ day: d, items: [i] });
		}
		return out;
	});
</script>

<div class="card max-w-3xl">
	<div class="flex items-center gap-2 border-b border-slate-100 p-4">
		<h2 class="flex-1 font-semibold text-slate-900">Coming up</h2>
		{#if canAdd}<button class="btn btn-primary" onclick={() => ((eventId = null), (occurrence = null), (editorOpen = true))}><Icon name="plus" class="size-4" /> New event</button>{/if}
	</div>
	{#if items}
		{#each days as d (d.day)}
			<section class="border-b border-slate-100 px-4 py-3 last:border-0">
				<h3 class="mb-1 text-xs font-medium tracking-wide text-slate-500 uppercase">{d.day}</h3>
				<ul>
					{#each d.items as i (i.id + (i.occurrence ?? ''))}
						<li>
							<button class="flex w-full items-baseline gap-3 rounded-lg px-2 py-1.5 text-left hover:bg-slate-50" data-group-event={i.title} onclick={() => ((eventId = i.id), (occurrence = i.occurrence), (editorOpen = true))}>
								<span class="w-44 shrink-0 text-sm whitespace-nowrap text-slate-500">{time(i)}</span>
								<span class="min-w-0 flex-1 truncate text-sm font-medium text-slate-800">{i.title}</span>
								{#if i.location}<span class="truncate text-xs text-slate-400">{i.location}</span>{/if}
							</button>
						</li>
					{/each}
				</ul>
			</section>
		{:else}
			<p class="p-10 text-center text-sm text-slate-500">Nothing planned in the next {weeks} weeks.</p>
		{/each}
		<div class="p-3 text-center"><button class="text-sm text-brand-600 hover:underline" onclick={() => (weeks += 8)}>Show further ahead</button></div>
	{/if}
</div>

<EventEditor bind:open={editorOpen} {eventId} {occurrence} {projectId} onchanged={load} />
