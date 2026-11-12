<script lang="ts">
	// Public: the confirmation link from the personal data request email.
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';

	let phase = $state<'busy' | 'ok' | 'bad'>('busy');
	let kind = $state('');
	onMount(async () => {
		try {
			const r = await api.post<{ kind: string }>('/public/privacy-request/verify', { token: page.url.searchParams.get('token') ?? '' });
			kind = r.kind;
			phase = 'ok';
		} catch {
			phase = 'bad';
		}
	});
</script>

<svelte:head><title>Confirm your request</title><meta name="robots" content="noindex" /></svelte:head>

<main class="flex min-h-screen items-center justify-center bg-slate-50 p-4">
	<div class="w-full max-w-md rounded-3xl bg-white p-8 text-center shadow-g1">
		{#if phase === 'busy'}
			<p class="text-sm text-slate-400">Confirming…</p>
		{:else if phase === 'ok'}
			<h1 class="text-xl font-semibold text-slate-900" data-confirmed>Request confirmed</h1>
			<p class="mt-2 text-sm text-slate-600">{kind === 'erasure' ? 'We will delete your personal data' : 'We will send you a copy of your personal data'} within 30 days, or tell you why we can't.</p>
		{:else}
			<h1 class="text-xl font-semibold text-slate-900">This link doesn't work</h1>
			<p class="mt-2 text-sm text-slate-600">It may have been used already, be incomplete or older than 7 days. You can send the request again.</p>
		{/if}
	</div>
</main>
