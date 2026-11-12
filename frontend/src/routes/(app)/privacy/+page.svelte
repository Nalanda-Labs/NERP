<script lang="ts">
	// Requests from people about their personal data: what we hold on them, or delete it.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Req = { id: string; kind: 'access' | 'erasure'; email: string; name: string | null; source: string; status: string; due_at: string | null; created_at: string };
	let items = $state<Req[]>([]);
	let overdue = $state(0);
	let status = $state('');
	let form = $state({ kind: 'access', email: '', phone: '', name: '', message: '' });
	let adding = $state(false);

	async function load() {
		try {
			const r = await api.get<{ items: Req[]; overdue: number }>(`/privacy/requests${status ? `?status=${status}` : ''}`);
			items = r.items;
			overdue = r.overdue;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	async function add(e: SubmitEvent) {
		e.preventDefault();
		try {
			const r = await api.post<Req>('/privacy/requests', form);
			goto(`/privacy/${r.id}`);
		} catch (err) {
			toasts.error(err);
		}
	}
	const STATUS: Record<string, string> = { unverified: 'Waiting for confirmation', open: 'Open', done: 'Done', rejected: 'Rejected' };
	const late = (r: Req) => r.status === 'open' && r.due_at && new Date(r.due_at) < new Date();
</script>

<svelte:head><title>Data requests · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Personal data requests</h1>
			<p class="text-sm text-slate-500">People asking what we hold on them, or to delete it — answered within 30 days. They can ask themselves at <a class="text-brand-700 hover:underline" href="/privacy-request" target="_blank">/privacy-request</a>.</p>
		</div>
		<button class="btn btn-primary ml-auto" onclick={() => (adding = !adding)} data-new-request>New request</button>
	</div>

	{#if adding}
		<form class="card grid gap-3 p-5 text-sm sm:grid-cols-2" onsubmit={add} aria-label="New request">
			<label class="block">Request<select class="input mt-1" bind:value={form.kind} name="kind"><option value="access">A copy of their data</option><option value="erasure">Delete their data</option></select></label>
			<label class="block">Email<input class="input mt-1" type="email" required bind:value={form.email} name="email" /></label>
			<label class="block">Phone (optional, to find more)<input class="input mt-1" bind:value={form.phone} name="phone" /></label>
			<label class="block">Name<input class="input mt-1" bind:value={form.name} name="name" /></label>
			<label class="block sm:col-span-2">How it came in, notes<textarea class="input mt-1" rows="2" bind:value={form.message}></textarea></label>
			<div class="sm:col-span-2"><button class="btn btn-primary">Add and look them up</button></div>
		</form>
	{/if}

	<div class="flex items-center gap-2 text-sm">
		<select class="input w-56" bind:value={status} onchange={load} aria-label="Status"><option value="">All</option><option value="open">Open</option><option value="unverified">Waiting for confirmation</option><option value="done">Done</option><option value="rejected">Rejected</option></select>
		{#if overdue}<span class="rounded-full bg-red-50 px-2 py-0.5 text-red-800" data-overdue>{overdue} overdue</span>{/if}
	</div>

	<section class="card divide-y divide-slate-100" aria-label="Requests">
		{#each items as r (r.id)}
			<a class="flex items-center gap-3 px-5 py-3 text-sm hover:bg-slate-50" href="/privacy/{r.id}" data-request={r.email}>
				<span class="w-24 shrink-0 font-medium {r.kind === 'erasure' ? 'text-red-700' : 'text-slate-700'}">{r.kind === 'erasure' ? 'Erasure' : 'Access'}</span>
				<span class="min-w-0 flex-1 truncate">{r.name ? `${r.name} · ` : ''}{r.email}</span>
				<span class="text-xs text-slate-500">{STATUS[r.status]}{r.source === 'public' ? ' · from the form' : ''}</span>
				<span class="w-28 text-right text-xs {late(r) ? 'font-medium text-red-700' : 'text-slate-500'}">{r.status === 'open' && r.due_at ? `due ${formatDate(r.due_at)}` : formatDate(r.created_at)}</span>
			</a>
		{:else}<p class="px-5 py-4 text-sm text-slate-500">No requests.</p>{/each}
	</section>
</div>
