<script lang="ts">
	// My approvals: what business processes are waiting for me to decide.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type A = { id: string; title: string; mode: string; due_at: string | null; created_at: string; item_id: string; item_title: string; number: number; list_id: string | null; list_name: string; author: string | null; link?: string };
	let items = $state<A[]>([]);
	let comments = $state<Record<string, string>>({});
	const load = async () => (items = (await api.get<{ items: A[] }>('/approvals')).items);
	onMount(() => {
		load();
		return realtime.on('approvals.changed', load);
	});
	async function decide(a: A, approve: boolean) {
		try {
			items = (await api.patch<{ items: A[] }>(`/approvals/${a.id}`, { approve, comment: comments[a.id] || null })).items;
			toasts.success(approve ? 'Approved' : 'Rejected');
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Approvals · NERP</title></svelte:head>

<div class="max-w-3xl space-y-4">
	<h1 class="text-2xl font-semibold text-slate-900">My approvals</h1>
	{#each items as a (a.id)}
		<div class="card space-y-3 p-5" data-approval={a.title}>
			<div>
				<p class="font-medium text-slate-900">{a.title}</p>
				<p class="text-sm text-slate-500"><a class="text-brand-600 hover:underline" href={a.link ?? `/lists/${a.list_id}?item=${a.item_id}`}>{a.list_name} #{a.number}: {a.item_title}</a> · from {a.author ?? 'someone'} · {formatRelative(a.created_at)}{a.mode === 'all' ? ' · everyone asked must approve' : ''}</p>
				{#if a.due_at}<p class="text-xs {new Date(a.due_at) < new Date() ? 'text-red-700' : 'text-slate-500'}">Due {formatDateTime(a.due_at)}</p>{/if}
			</div>
			<input class="input" placeholder="Comment (optional)" aria-label="Comment" bind:value={comments[a.id]} />
			<div class="flex gap-2"><button class="btn btn-primary" onclick={() => decide(a, true)}>Approve</button><button class="btn btn-secondary" onclick={() => decide(a, false)}>Reject</button></div>
		</div>
	{:else}
		<div class="card p-10 text-center text-slate-500">Nothing waiting for you.</div>
	{/each}
</div>
