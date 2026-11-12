<script lang="ts">
	// Public: the link from the portal email signs in and goes on to the portal.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	let failed = $state(false);
	onMount(async () => {
		const r = await fetch('/api/public/portal/sign-in', {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ token: page.url.searchParams.get('token') ?? '' })
		});
		if (r.ok) goto('/portal', { replaceState: true });
		else failed = true;
	});
</script>

<svelte:head><title>Client portal</title><meta name="robots" content="noindex" /></svelte:head>

<main class="flex min-h-screen items-center justify-center bg-slate-50 p-4">
	<div class="w-full max-w-md rounded-3xl bg-white p-8 text-center shadow-g1">
		{#if failed}
			<h1 class="text-xl font-semibold text-slate-900" data-link-bad>This link doesn't work</h1>
			<p class="mt-2 text-sm text-slate-600">It may have been used already or be older than 7 days. <a class="text-brand-700 hover:underline" href="/portal">Ask for a new one.</a></p>
		{:else}
			<p class="text-sm text-slate-400">Signing in…</p>
		{/if}
	</div>
</main>
