<script lang="ts">
	// A stored video, played in the page: the copy every browser plays once the server has made
	// it (with its poster frame), the original until then.
	import { onDestroy, onMount } from 'svelte';
	import { api, streamUrl } from '#lib/api.ts';

	let { fileId, name = '', class: klass = '' }: { fileId: string; name?: string; class?: string } = $props();
	type Status = { status: string; web?: boolean; poster?: boolean };
	let src = $state('');
	let poster = $state<string | undefined>(undefined);
	let status = $state<Status | null>(null);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function load() {
		status = await api.get<Status>(`/files/${fileId}/video-status`).catch(() => null);
		const [s, p] = await Promise.all([streamUrl(`/api/files/${fileId}/video`), status?.poster ? streamUrl(`/api/files/${fileId}/poster`) : Promise.resolve(undefined)]);
		src = s;
		poster = p;
		// Still being made: look again in a while.
		if (status && (status.status === 'pending' || status.status === 'converting')) timer = setTimeout(load, 5000);
	}
	onMount(load);
	onDestroy(() => clearTimeout(timer));
</script>

<figure class="space-y-1 {klass}" data-video-player={fileId}>
	{#if src}
		<!-- svelte-ignore a11y_media_has_caption -->
		<video class="max-h-[420px] w-full rounded-xl bg-black" controls preload="metadata" playsinline {src} {poster} aria-label={name || 'Video'}></video>
	{:else}
		<div class="flex aspect-video w-full items-center justify-center rounded-xl bg-slate-900 text-sm text-slate-400">Loading…</div>
	{/if}
	{#if status && (status.status === 'pending' || status.status === 'converting')}
		<figcaption class="text-xs text-slate-500" data-video-converting>Making a version every browser plays…</figcaption>
	{/if}
</figure>
