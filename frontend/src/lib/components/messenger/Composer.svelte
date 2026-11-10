<script lang="ts">
	// Writing in a chat (or under a post): text and files, voice messages and video notes
	// recorded in the browser, stickers and polls.
	import { api } from '#lib/api.ts';
	import { STICKERS, type Msg } from '#lib/messenger.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';

	let {
		chatId,
		parentId = null,
		placeholder = 'Write a message…',
		onsent,
		onfocus
	}: {
		chatId: string;
		parentId?: string | null;
		placeholder?: string;
		onsent: (m: Msg) => void;
		onfocus?: () => void;
	} = $props();

	let draft = $state('');
	let files = $state<{ id: string; name: string }[]>([]);
	let busy = $state(false);

	async function post(body: Record<string, unknown>) {
		busy = true;
		try {
			onsent(await api.post<Msg>(`/im/chats/${chatId}/messages`, { parent_id: parentId, ...body }));
			return true;
		} catch (err) {
			toasts.error(err);
			return false;
		} finally {
			busy = false;
		}
	}
	async function uploadOne(f: Blob, name: string) {
		const fd = new FormData();
		fd.append('file', f, name);
		return (await api.post<{ id: string }>('/files', fd)).id;
	}
	async function upload(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		for (const f of Array.from(input.files ?? [])) {
			try {
				files = [...files, { id: await uploadOne(f, f.name), name: f.name }];
			} catch (err) {
				toasts.error(err);
			}
		}
		input.value = '';
	}
	async function send(e: SubmitEvent) {
		e.preventDefault();
		if (!draft.trim() && !files.length) return;
		if (await post({ body: draft, file_ids: files.map((f) => f.id) })) {
			draft = '';
			files = [];
		}
	}
	function keydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			(e.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
		}
	}

	// ----- voice messages and video notes -----
	let recording = $state<'voice' | 'video' | null>(null);
	let seconds = $state(0);
	let preview: HTMLVideoElement | undefined = $state();
	let recorder: MediaRecorder | null = null;
	let stream: MediaStream | null = null;
	let chunks: Blob[] = [];
	let timer: ReturnType<typeof setInterval> | undefined;
	let cancelled = false;
	const LIMIT = { voice: 300, video: 60 };

	async function record(kind: 'voice' | 'video') {
		try {
			stream = await navigator.mediaDevices.getUserMedia(kind === 'voice' ? { audio: true } : { audio: true, video: { width: 480, height: 480, facingMode: 'user' } });
		} catch {
			toasts.error(kind === 'voice' ? 'The microphone is not available' : 'The camera is not available');
			return;
		}
		chunks = [];
		cancelled = false;
		recorder = new MediaRecorder(stream);
		recorder.ondataavailable = (e) => e.data.size && chunks.push(e.data);
		recorder.onstop = finish;
		recorder.start(250);
		recording = kind;
		seconds = 0;
		timer = setInterval(() => {
			seconds++;
			if (seconds >= LIMIT[kind]) stop();
		}, 1000);
		if (kind === 'video') {
			await Promise.resolve();
			if (preview) preview.srcObject = stream;
		}
	}
	function stop(cancel = false) {
		cancelled = cancel;
		clearInterval(timer);
		recorder?.state === 'recording' && recorder.stop();
	}
	async function finish() {
		const kind = recording!;
		stream?.getTracks().forEach((t) => t.stop());
		stream = null;
		recording = null;
		if (cancelled || !chunks.length) return;
		const type = (recorder?.mimeType || (kind === 'voice' ? 'audio/webm' : 'video/webm')).split(';')[0]!;
		const blob = new Blob(chunks, { type });
		const ext = type.includes('mp4') ? 'mp4' : type.includes('ogg') ? 'ogg' : 'webm';
		try {
			const id = await uploadOne(blob, `${kind === 'voice' ? 'Voice message' : 'Video note'} ${new Date().toISOString().slice(0, 16).replace('T', ' ')}.${ext}`);
			await post({ kind, file_ids: [id] });
		} catch (err) {
			toasts.error(err);
		}
	}
	const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;

	// ----- stickers -----
	let stickersOpen = $state(false);
	async function sticker(code: string) {
		stickersOpen = false;
		await post({ kind: 'sticker', body: code });
	}

	// ----- polls -----
	let pollOpen = $state(false);
	let poll = $state({ question: '', options: ['', ''], multiple: false, anonymous: false });
	function openPoll() {
		poll = { question: '', options: ['', ''], multiple: false, anonymous: false };
		pollOpen = true;
	}
	async function sendPoll(e: SubmitEvent) {
		e.preventDefault();
		try {
			onsent(await api.post<Msg>(`/im/chats/${chatId}/polls`, { ...poll, parent_id: parentId }));
			pollOpen = false;
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<form class="border-t border-slate-100 p-3" onsubmit={send}>
	{#if files.length}
		<div class="mb-2 flex flex-wrap gap-2">{#each files as f, i (f.id)}<span class="flex items-center gap-1 rounded-lg bg-slate-100 px-2 py-1 text-xs">{f.name}<button type="button" aria-label="Remove {f.name}" onclick={() => files.splice(i, 1)}><Icon name="x" class="size-3" /></button></span>{/each}</div>
	{/if}
	{#if recording}
		<div class="flex items-center gap-3" data-recording={recording}>
			{#if recording === 'video'}
				<!-- svelte-ignore a11y_media_has_caption -->
				<video bind:this={preview} class="size-24 rounded-full bg-black object-cover" autoplay muted playsinline></video>
			{/if}
			<span class="flex items-center gap-2 text-sm text-red-600"><span class="size-2.5 animate-pulse rounded-full bg-red-600"></span>{recording === 'voice' ? 'Recording a voice message' : 'Recording a video note'} · {clock(seconds)}</span>
			<button type="button" class="btn btn-ghost ml-auto" onclick={() => stop(true)}>Cancel</button>
			<button type="button" class="btn btn-primary" onclick={() => stop()}>Send</button>
		</div>
	{:else}
		<div class="relative flex items-end gap-1">
			<label class="btn btn-ghost cursor-pointer p-2" aria-label="Attach files" title="Attach files"><Icon name="paperclip" class="size-5" /><input type="file" multiple class="sr-only" onchange={upload} /></label>
			<button type="button" class="btn btn-ghost p-2 text-lg leading-none" aria-label="Sticker" title="Stickers" onclick={() => (stickersOpen = !stickersOpen)}>☺</button>
			<button type="button" class="btn btn-ghost p-2 text-base leading-none" aria-label="Poll" title="Poll" onclick={openPoll}>📊</button>
			{#if stickersOpen}
				<div class="absolute bottom-12 left-0 z-10 grid grid-cols-6 gap-1 rounded-xl border border-slate-200 bg-white p-2 shadow-lg" role="menu" aria-label="Stickers">
					{#each Object.entries(STICKERS) as [code, s] (code)}
						<button type="button" class="rounded-lg p-1 text-3xl hover:bg-slate-100" role="menuitem" aria-label={s.label} onclick={() => sticker(code)}>{s.emoji}</button>
					{/each}
				</div>
			{/if}
			<textarea class="input max-h-40 min-h-10 flex-1 resize-none" rows="1" maxlength="10000" {placeholder} aria-label="Message" bind:value={draft} onkeydown={keydown} {onfocus}></textarea>
			{#if draft.trim() || files.length}
				<button class="btn btn-primary" disabled={busy}>Send</button>
			{:else}
				<button type="button" class="btn btn-ghost p-2 text-base" aria-label="Record a voice message" title="Voice message" onclick={() => record('voice')}>🎙️</button>
				<button type="button" class="btn btn-ghost p-2 text-base" aria-label="Record a video note" title="Video note" onclick={() => record('video')}>📹</button>
			{/if}
		</div>
	{/if}
</form>

<Modal bind:open={pollOpen} title="New poll">
	<form id="poll-form" class="space-y-3" onsubmit={sendPoll}>
		<div><label class="label" for="poll-q">Question</label><input id="poll-q" class="input" required maxlength="300" bind:value={poll.question} /></div>
		<div class="space-y-2">
			<span class="label">Answers</span>
			{#each poll.options as _, i (i)}
				<div class="flex gap-2">
					<input class="input" maxlength="200" aria-label="Answer {i + 1}" bind:value={poll.options[i]} />
					{#if poll.options.length > 2}<button type="button" class="btn btn-ghost px-2" aria-label="Remove answer {i + 1}" onclick={() => poll.options.splice(i, 1)}><Icon name="x" class="size-4" /></button>{/if}
				</div>
			{/each}
			{#if poll.options.length < 10}<button type="button" class="text-sm text-brand-600 hover:underline" onclick={() => poll.options.push('')}>Add an answer</button>{/if}
		</div>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={poll.multiple} /> Several answers allowed</label>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={poll.anonymous} /> Anonymous</label>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (pollOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="poll-form">Post the poll</button>
	{/snippet}
</Modal>
