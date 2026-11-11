<script lang="ts">
	// Records a short video (camera and microphone) in the browser, for a video announcement:
	// look at it, record again or use it. At most five minutes.
	import { onDestroy } from 'svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	let { onrecorded, oncancel }: { onrecorded: (file: File) => void; oncancel: () => void } = $props();
	const MAX_SECS = 300;
	let stream: MediaStream | null = null;
	let recorder: MediaRecorder | null = null;
	let chunks: Blob[] = [];
	let live: HTMLVideoElement | undefined = $state();
	let recording = $state(false);
	let seconds = $state(0);
	let result = $state<{ url: string; file: File } | null>(null);
	let tick: ReturnType<typeof setInterval> | undefined;

	async function start() {
		try {
			stream = await navigator.mediaDevices.getUserMedia({ video: { width: { ideal: 1280 }, height: { ideal: 720 } }, audio: true });
		} catch {
			toasts.error('The camera or microphone could not be used; check the browser\'s permissions');
			return;
		}
		if (live) live.srcObject = stream;
		const type = ['video/webm;codecs=vp9,opus', 'video/webm;codecs=vp8,opus', 'video/webm', 'video/mp4'].find((t) => MediaRecorder.isTypeSupported(t)) ?? '';
		recorder = new MediaRecorder(stream, type ? { mimeType: type } : undefined);
		chunks = [];
		recorder.ondataavailable = (e) => e.data.size && chunks.push(e.data);
		recorder.onstop = () => {
			const mime = recorder?.mimeType || 'video/webm';
			const blob = new Blob(chunks, { type: mime });
			const file = new File([blob], `Video announcement.${mime.includes('mp4') ? 'mp4' : 'webm'}`, { type: mime.split(';')[0] });
			result = { url: URL.createObjectURL(blob), file };
			release();
		};
		recorder.start(1000);
		recording = true;
		seconds = 0;
		tick = setInterval(() => {
			seconds += 1;
			if (seconds >= MAX_SECS) stop();
		}, 1000);
	}
	function stop() {
		clearInterval(tick);
		recording = false;
		if (recorder?.state === 'recording') recorder.stop();
	}
	function release() {
		stream?.getTracks().forEach((t) => t.stop());
		stream = null;
	}
	function again() {
		if (result) URL.revokeObjectURL(result.url);
		result = null;
		start();
	}
	function use() {
		if (result) onrecorded(result.file);
	}
	function cancel() {
		stop();
		release();
		oncancel();
	}
	onDestroy(() => {
		clearInterval(tick);
		release();
		if (result) URL.revokeObjectURL(result.url);
	});
	const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
</script>

<div class="space-y-2 rounded-xl bg-slate-50 p-3" data-video-recorder>
	{#if result}
		<!-- svelte-ignore a11y_media_has_caption -->
		<video class="max-h-80 w-full rounded-lg bg-black" controls src={result.url}></video>
		<div class="flex gap-2">
			<button type="button" class="btn btn-primary" onclick={use} data-use-video>Use this video</button>
			<button type="button" class="btn btn-secondary" onclick={again}>Record again</button>
			<button type="button" class="btn btn-ghost" onclick={cancel}>Cancel</button>
		</div>
	{:else}
		<video bind:this={live} class="max-h-80 w-full -scale-x-100 rounded-lg bg-black" autoplay muted playsinline></video>
		<div class="flex items-center gap-2">
			{#if recording}
				<button type="button" class="btn btn-primary" onclick={stop} data-stop-recording>■ Stop</button>
				<span class="text-sm text-red-600 tabular-nums" aria-live="polite">● {clock(seconds)} / {clock(MAX_SECS)}</span>
			{:else}
				<button type="button" class="btn btn-primary" onclick={start} data-start-recording>● Record</button>
			{/if}
			<button type="button" class="btn btn-ghost" onclick={cancel}>Cancel</button>
		</div>
	{/if}
</div>
