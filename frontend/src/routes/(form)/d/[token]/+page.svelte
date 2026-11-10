<script lang="ts">
	// Public: a file shared with a link from the drive.
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import { formatBytes } from '#lib/format.ts';

	let info = $state<{ name: string; size_bytes: number } | null>(null);
	let missing = $state(false);
	$effect(() => {
		api.get<{ name: string; size_bytes: number }>(`/public/drive/${encodeURIComponent(page.params.token ?? '')}`).then((i) => (info = i)).catch(() => (missing = true));
	});
</script>

<svelte:head><title>{info?.name ?? 'Shared file'}</title><meta name="robots" content="noindex" /></svelte:head>

<main class="flex min-h-screen items-center justify-center bg-slate-50 p-4">
	<div class="w-full max-w-md rounded-3xl bg-white p-8 text-center shadow-g1">
		{#if missing}
			<h1 class="text-xl font-semibold text-slate-900">This link doesn't work</h1>
			<p class="mt-2 text-sm text-slate-600">It may have expired or been removed.</p>
		{:else if info}
			<h1 class="text-xl font-semibold break-words text-slate-900" data-file-name>{info.name}</h1>
			<p class="mt-1 text-sm text-slate-500">{formatBytes(info.size_bytes)}</p>
			<a class="btn btn-primary mt-6" href="/api/public/drive/{encodeURIComponent(page.params.token ?? '')}/download">Download</a>
		{:else}
			<p class="text-sm text-slate-400">Loading…</p>
		{/if}
	</div>
</main>
