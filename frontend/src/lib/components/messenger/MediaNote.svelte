<script lang="ts">
	// A voice message or video note, played in place (through a short-lived link).
	import { streamUrl } from '#lib/api.ts';
	import type { MsgFile } from '#lib/messenger.ts';

	let { file, kind }: { file: MsgFile; kind: 'voice' | 'video' } = $props();
	let url = $state<string | null>(null);
	let failed = $state(false);
	$effect(() => {
		file.id;
		url = null;
		streamUrl(`/api/files/${file.id}/content`).then((u) => (url = u)).catch(() => (failed = true));
	});
</script>

{#if failed}
	<span class="text-xs opacity-70">The recording can't be played.</span>
{:else if !url}
	<span class="text-xs opacity-70">Loading…</span>
{:else if kind === 'voice'}
	<audio class="h-9 w-64 max-w-full" controls preload="metadata" src={url} data-voice></audio>
{:else}
	<!-- svelte-ignore a11y_media_has_caption -->
	<video class="size-48 rounded-full bg-black object-cover" controls preload="metadata" src={url} data-video-note></video>
{/if}
