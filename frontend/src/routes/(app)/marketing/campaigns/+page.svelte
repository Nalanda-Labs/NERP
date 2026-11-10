<script lang="ts">
	// Marketing → Campaigns: every email campaign with its state and how it did.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { CAMPAIGN_STATUS, rate, type Campaign } from './campaign.ts';

	let items = $state<Campaign[]>([]);
	let status = $state('');
	let loaded = $state(false);
	async function load() {
		try {
			items = (await api.get<{ items: Campaign[] }>(`/marketing/campaigns${status ? `?status=${status}` : ''}`)).items;
			loaded = true;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	async function create() {
		const name = prompt('Name the campaign', 'New campaign');
		if (!name?.trim()) return;
		try {
			const c = await api.post<Campaign>('/marketing/campaigns', { name });
			goto(`/marketing/campaigns/${c.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Campaigns · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-end gap-3">
		<div class="flex-1">
			<h1 class="text-2xl font-semibold text-slate-900">Campaigns</h1>
			<p class="mt-1 text-sm text-slate-500">Email campaigns to your segments, with opens, clicks and the leads and deals they bring in.</p>
		</div>
		<select class="input w-40" bind:value={status} onchange={load} aria-label="Status">
			<option value="">All</option>
			{#each Object.entries(CAMPAIGN_STATUS) as [k, [l]] (k)}<option value={k}>{l}</option>{/each}
		</select>
		<button class="btn btn-primary" onclick={create}><Icon name="plus" class="size-4" /> New campaign</button>
	</div>

	<div class="card overflow-x-auto">
		<table class="w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs text-slate-500">
				<tr>
					<th class="px-5 py-2.5 font-medium">Campaign</th>
					<th class="px-3 py-2.5 font-medium">Status</th>
					<th class="px-3 py-2.5 text-right font-medium">Sent</th>
					<th class="px-3 py-2.5 text-right font-medium">Opened</th>
					<th class="px-3 py-2.5 text-right font-medium">Clicked</th>
					<th class="px-5 py-2.5 text-right font-medium">Unsubscribed</th>
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-50">
				{#each items as c (c.id)}
					{@const st = CAMPAIGN_STATUS[c.status]}
					<tr class="cursor-pointer hover:bg-slate-50" onclick={() => goto(`/marketing/campaigns/${c.id}`)} data-campaign={c.id}>
						<td class="px-5 py-2.5">
							<a class="font-medium text-slate-900 hover:underline" href="/marketing/campaigns/{c.id}">{c.name}</a>
							<p class="text-xs text-slate-500">{c.subject || 'No subject yet'}</p>
						</td>
						<td class="px-3 py-2.5 whitespace-nowrap">
							<span class="rounded-full px-2 py-0.5 text-xs font-medium {st?.[1] ?? 'bg-slate-100 text-slate-600'}">{st?.[0] ?? c.status}</span>
							<span class="block text-xs text-slate-500">{c.status === 'scheduled' ? formatDateTime(c.send_at) : c.finished_at ? formatDateTime(c.finished_at) : c.started_at ? formatDateTime(c.started_at) : ''}</span>
						</td>
						<td class="px-3 py-2.5 text-right tabular-nums">{c.counts.sent || '—'}</td>
						<td class="px-3 py-2.5 text-right tabular-nums">{c.counts.sent ? rate(c.counts.opened, c.counts.sent) : '—'}</td>
						<td class="px-3 py-2.5 text-right tabular-nums">{c.counts.sent ? rate(c.counts.clicked, c.counts.sent) : '—'}</td>
						<td class="px-5 py-2.5 text-right tabular-nums">{c.counts.sent ? c.counts.unsubscribed : '—'}</td>
					</tr>
				{:else}
					<tr><td colspan="6" class="px-5 py-8 text-center text-slate-500">{loaded ? 'No campaigns yet.' : 'Loading…'}</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
