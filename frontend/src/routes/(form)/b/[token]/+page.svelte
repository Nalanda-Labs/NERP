<script lang="ts">
	// A whiteboard opened with a public link (view or edit), with its password if it has one.
	/* eslint-disable @typescript-eslint/no-explicit-any */
	import { onDestroy, onMount } from 'svelte';
	import { page } from '$app/state';
	import { BoardSync, type BoardFile, type El } from '#lib/boards.svelte.ts';
	import Excalidraw from '#lib/components/boards/Excalidraw.svelte';

	type Opened = { id: string; title: string; level: 'view' | 'edit'; version: number; scene: { elements: El[]; appState: Record<string, unknown> }; files: BoardFile[]; viewers: { id: string; name: string }[] };
	const token = $derived(page.params.token!);
	let board = $state<Opened | null>(null);
	let needPassword = $state(false);
	let password = $state('');
	let error = $state('');
	let missing = $state(false);
	let sync = $state<BoardSync | null>(null);
	let timer: ReturnType<typeof setInterval> | undefined;

	const call = async (method: string, path: string, body?: unknown) => {
		const headers: Record<string, string> = {};
		if (password) headers['X-Board-Password'] = password;
		if (body) headers['content-type'] = 'application/json';
		const r = await fetch(`/api/public/boards/${encodeURIComponent(token)}${path}`, { method, headers, body: body ? JSON.stringify(body) : undefined });
		if (!r.ok) {
			const d = await r.json().catch(() => ({}));
			throw Object.assign(new Error(d?.error?.message ?? 'Something went wrong'), { status: r.status });
		}
		return r;
	};
	async function open(e?: SubmitEvent) {
		e?.preventDefault();
		error = '';
		try {
			board = await (await call('GET', '')).json();
			needPassword = false;
			sync = new BoardSync(
				{
					load: async (since) => (await call('GET', `/scene${since === null ? '' : `?since=${since}`}`)).json(),
					save: async (b) => (await call('PUT', '/scene', b)).json(),
					addFile: async (key, dataURL) => { await call('POST', '/files', { key, data_url: dataURL }); },
					file: async (key) => (await call('GET', `/files/${encodeURIComponent(key)}`)).blob()
				},
				board!.level === 'edit'
			);
			// Guests have no live connection: they ask every few seconds.
			timer = setInterval(() => sync?.pull(), 3000);
		} catch (err: any) {
			if (err.status === 401) {
				needPassword = true;
				if (password) error = err.message;
			} else if (err.status === 404) missing = true;
			else error = err.message;
		}
	}
	onMount(() => open());
	onDestroy(() => {
		clearInterval(timer);
		sync?.flush();
	});
	function ready(a: any, lib: any) {
		if (board && sync) sync.attach(a, { version: board.version, scene: board.scene, files: board.files, viewers: board.viewers }, lib.CaptureUpdateAction);
	}
</script>

<svelte:head><title>{board?.title ?? 'Board'}</title></svelte:head>

{#if missing}
	<main class="flex min-h-dvh items-center justify-center bg-slate-50 p-4"><p class="rounded-2xl bg-white p-8 text-slate-600 shadow-sm">This board isn't available.</p></main>
{:else if needPassword}
	<main class="flex min-h-dvh items-center justify-center bg-slate-50 p-4">
		<form class="w-full max-w-sm space-y-3 rounded-2xl bg-white p-6 shadow-sm" onsubmit={open}>
			<h1 class="text-lg font-semibold text-slate-900">This board needs a password</h1>
			<input class="input" type="password" aria-label="Password" required bind:value={password} />
			{#if error}<p class="text-sm text-red-600" role="alert">{error}</p>{/if}
			<button class="btn btn-primary w-full justify-center">Open</button>
		</form>
	</main>
{:else if board && sync}
	<main class="flex h-dvh flex-col">
		<div class="flex items-center gap-2 border-b border-slate-200 bg-white px-4 py-2">
			<h1 class="font-semibold text-slate-900">{board.title}</h1>
			<span class="text-xs text-slate-500" data-saving>{board.level === 'view' ? 'View only' : sync.error ? `Not saved: ${sync.error}` : sync.saving ? 'Saving…' : 'Saved'}</span>
			{#if sync.viewers.length}<span class="ml-auto text-xs text-slate-500">👀 {sync.viewers.map((v) => v.name).join(', ')}</span>{/if}
		</div>
		<div class="flex-1"><Excalidraw elements={board.scene.elements} appState={board.scene.appState} readOnly={board.level === 'view'} name={board.title} onready={ready} onchange={(els, st, files) => sync?.changed(els, st, files)} /></div>
	</main>
{:else if error}
	<main class="flex min-h-dvh items-center justify-center p-4"><p class="text-red-600">{error}</p></main>
{/if}
