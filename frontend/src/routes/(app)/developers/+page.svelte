<script lang="ts">
	// API & webhooks: personal API keys, outgoing event webhooks, and the API reference.
	import { page } from '$app/state';
	import ApiKeys from '#lib/components/api/ApiKeys.svelte';
	import ApiReference from '#lib/components/api/ApiReference.svelte';
	import Webhooks from '#lib/components/api/Webhooks.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';

	const tabs = $derived(
		[
			['keys', 'API keys', true],
			['webhooks', 'Webhooks', session.can(PERM.WEBHOOKS_MANAGE)],
			['reference', 'Reference', true]
		].filter((t) => t[2]) as [string, string, boolean][]
	);
	const tab = $derived(tabs.find(([k]) => k === page.url.searchParams.get('tab'))?.[0] ?? 'keys');
</script>

<svelte:head><title>API & webhooks · NERP</title></svelte:head>

<div class="space-y-5">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">API & webhooks</h1>
		<p class="mt-1 max-w-3xl text-sm text-slate-500">Connect other systems: API keys let them call NERP's REST API on your behalf, and webhooks send them events as they happen.</p>
	</div>
	<nav class="flex flex-wrap gap-1">
		{#each tabs as [k, l] (k)}
			<a href="?tab={k}" class="btn {tab === k ? 'bg-brand-100 text-brand-900' : 'btn-ghost'}" aria-current={tab === k ? 'page' : undefined} data-sveltekit-replacestate data-sveltekit-noscroll>{l}</a>
		{/each}
	</nav>
	{#if tab === 'keys'}
		<ApiKeys />
	{:else if tab === 'webhooks'}
		<Webhooks />
	{:else}
		<ApiReference />
	{/if}
</div>
