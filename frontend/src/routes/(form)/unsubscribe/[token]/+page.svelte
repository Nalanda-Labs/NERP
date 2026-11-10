<script lang="ts">
	// Public: the unsubscribe link in campaign emails. Asks once, then confirms.
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';

	let info = $state<{ email: string; company: string | null; unsubscribed: boolean } | null>(null);
	let missing = $state(false);
	let busy = $state(false);
	onMount(async () => {
		try {
			info = await api.get(`/public/m/u/${page.params.token}`);
		} catch {
			missing = true;
		}
	});
	async function confirm() {
		busy = true;
		try {
			await api.post(`/public/m/u/${page.params.token}`, {});
			if (info) info.unsubscribed = true;
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Unsubscribe</title><meta name="robots" content="noindex" /></svelte:head>

<main class="flex min-h-screen items-center justify-center bg-slate-50 p-4">
	<div class="w-full max-w-md rounded-3xl bg-white p-8 text-center shadow-g1">
		{#if missing}
			<h1 class="text-xl font-semibold text-slate-900">This link doesn't work</h1>
			<p class="mt-2 text-sm text-slate-600">It may be incomplete. Use the unsubscribe link from the email again.</p>
		{:else if !info}
			<p class="text-sm text-slate-400">Loading…</p>
		{:else if info.unsubscribed}
			<h1 class="text-xl font-semibold text-slate-900">You're unsubscribed</h1>
			<p class="mt-2 text-sm text-slate-600">{info.email} won't get marketing emails from {info.company ?? 'us'} any more.</p>
		{:else}
			<h1 class="text-xl font-semibold text-slate-900">Unsubscribe?</h1>
			<p class="mt-2 text-sm text-slate-600">{info.email} will stop getting marketing emails from {info.company ?? 'us'}.</p>
			<button class="btn btn-primary mt-6" disabled={busy} onclick={confirm}>{busy ? 'One moment…' : 'Unsubscribe'}</button>
		{/if}
	</div>
</main>
