<script lang="ts">
	// Marketing → Unsubscribes: addresses no campaign sends to (people who unsubscribed, and
	// ones added by hand).
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Optout = { email: string; source: 'unsubscribed' | 'added'; campaign: string | null; created_at: string };
	let items = $state<Optout[]>([]);
	let total = $state(0);
	let q = $state('');
	let add = $state('');
	async function load() {
		try {
			const r = await api.get<{ items: Optout[]; total: number }>(`/marketing/optouts${q.trim() ? `?q=${encodeURIComponent(q.trim())}` : ''}`);
			items = r.items;
			total = r.total;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	let timer: ReturnType<typeof setTimeout> | undefined;
	function search() {
		clearTimeout(timer);
		timer = setTimeout(load, 250);
	}
	async function addOne(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.post('/marketing/optouts', { email: add });
			add = '';
			toasts.success('Added');
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove(o: Optout) {
		if (!confirm(`Send marketing emails to ${o.email} again? Only do this if they asked for it.`)) return;
		try {
			await api.del(`/marketing/optouts/${encodeURIComponent(o.email)}`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Unsubscribes · NERP</title></svelte:head>

<div class="space-y-5">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">Unsubscribes</h1>
		<p class="mt-1 text-sm text-slate-500">{total} address{total === 1 ? '' : 'es'} that no campaign sends to. Emails about their own deals, tasks or documents are not affected.</p>
	</div>
	<div class="flex flex-wrap gap-3">
		<input class="input w-72" type="search" placeholder="Find an address…" bind:value={q} oninput={search} aria-label="Find an address" />
		<form class="ml-auto flex gap-2" onsubmit={addOne}>
			<input class="input w-64" type="email" required placeholder="Add an address" bind:value={add} aria-label="Add an address" />
			<button class="btn btn-secondary">Add</button>
		</form>
	</div>
	<div class="card overflow-x-auto">
		<table class="w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs text-slate-500"><tr><th class="px-5 py-2.5 font-medium">Address</th><th class="px-3 py-2.5 font-medium">How</th><th class="px-3 py-2.5 font-medium">When</th><th class="px-5 py-2.5"></th></tr></thead>
			<tbody class="divide-y divide-slate-50">
				{#each items as o (o.email)}
					<tr data-optout={o.email}>
						<td class="px-5 py-2.5">{o.email}</td>
						<td class="px-3 py-2.5 text-slate-600">{o.source === 'unsubscribed' ? `Unsubscribed${o.campaign ? ` from “${o.campaign}”` : ''}` : 'Added by an employee'}</td>
						<td class="px-3 py-2.5 text-slate-600">{formatDateTime(o.created_at)}</td>
						<td class="px-5 py-2.5 text-right"><button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => remove(o)}>Remove</button></td>
					</tr>
				{:else}
					<tr><td colspan="4" class="px-5 py-8 text-center text-slate-500">Nobody has unsubscribed.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
