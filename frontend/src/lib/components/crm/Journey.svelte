<script lang="ts">
	// The client's journey: their visits to the company's websites (from the tracking script),
	// each with where it came from and the pages seen, then what became of the record.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';

	type Visit = { id: string; started_at: string; channel: string; source: string; utm: Record<string, string>; referrer: string | null; landing: string; pages: number; ad: string | null; page_list: { at: string; url: string; title: string | null }[] };
	type Ev = { kind: 'created' | 'stage' | 'won' | 'lost'; at: string; stage?: string };
	let { itemId }: { itemId: string } = $props();
	type Data = { first_touch: { channel: string; source: string } | null; visits: Visit[]; events: Ev[] };
	let data = $state<Data | null>(null);
	let open = $state<string | null>(null);
	const CHANNEL: Record<string, string> = { paid: 'Paid ad', search: 'Search', social: 'Social', email: 'Email', referral: 'Website', campaign: 'Campaign', offline: 'Offline ad', direct: 'Direct' };

	onMount(async () => {
		data = await api.get<Data>(`/crm/items/${itemId}/journey`).catch(() => null);
	});
	const steps = $derived.by(() => {
		if (!data) return [];
		const all: ({ at: string } & ({ visit: Visit } | { event: Ev }))[] = [...data.visits.map((v) => ({ at: v.started_at, visit: v })), ...data.events.map((e) => ({ at: e.at, event: e }))];
		return all.sort((a, b) => a.at.localeCompare(b.at));
	});
	const path = (u: string) => {
		try {
			const x = new URL(u);
			return x.host + x.pathname;
		} catch {
			return u;
		}
	};
</script>

{#if data && data.visits.length}
	<section class="card p-5" data-journey aria-labelledby="journey-h">
		<h2 id="journey-h" class="font-medium text-slate-900">Journey</h2>
		{#if data.first_touch}<p class="mt-0.5 text-xs text-slate-500">First came through {CHANNEL[data.first_touch.channel] ?? data.first_touch.channel} · {data.first_touch.source}</p>{/if}
		<ol class="mt-3 space-y-2 border-l border-slate-200 pl-4 text-sm">
			{#each steps as s, i (i)}
				<li class="relative">
					<span class="absolute top-1.5 -left-[21px] size-2.5 rounded-full {'visit' in s ? 'bg-brand-500' : s.event.kind === 'won' ? 'bg-emerald-500' : s.event.kind === 'lost' ? 'bg-red-500' : 'bg-slate-400'}" aria-hidden="true"></span>
					{#if 'visit' in s}
						{@const v = s.visit}
						<button class="text-left" onclick={() => (open = open === v.id ? null : v.id)} aria-expanded={open === v.id}>
							<span class="font-medium text-slate-800">{CHANNEL[v.channel] ?? v.channel}</span>
							<span class="text-slate-600">· {v.ad ?? v.source}{v.utm.campaign ? ` · ${v.utm.campaign}` : ''}</span>
							<span class="block text-xs text-slate-500">{formatDateTime(v.started_at)} · {v.pages} page{v.pages === 1 ? '' : 's'} · {path(v.landing)}</span>
						</button>
						{#if open === v.id}
							<ul class="mt-1 space-y-0.5 text-xs text-slate-600">
								{#each v.page_list as p, j (j)}<li class="truncate" title={p.url}>{p.title || path(p.url)}</li>{/each}
							</ul>
						{/if}
					{:else}
						<span class="text-slate-700">{s.event.kind === 'created' ? 'Became a CRM record' : s.event.kind === 'won' ? `Won (${s.event.stage})` : s.event.kind === 'lost' ? `Lost (${s.event.stage})` : `Moved to ${s.event.stage}`}</span>
						<span class="block text-xs text-slate-500">{formatDateTime(s.event.at)}</span>
					{/if}
				</li>
			{/each}
		</ol>
	</section>
{/if}
