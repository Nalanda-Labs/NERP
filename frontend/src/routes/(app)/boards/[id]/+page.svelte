<script lang="ts">
	// One whiteboard: drawing together (changes merged and shown live), who's looking, sharing
	// with people and by link, its chat, and turning selected text into a task.
	/* eslint-disable @typescript-eslint/no-explicit-any */
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onDestroy, onMount } from 'svelte';
	import { publicLinksAllowed } from '#lib/companyRules.ts';
	import { api, requestBlob } from '#lib/api.ts';
	import { BoardSync, type BoardFile, type El } from '#lib/boards.svelte.ts';
	import ChatCallButtons from '#lib/components/ChatCallButtons.svelte';
	import Excalidraw from '#lib/components/boards/Excalidraw.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Grant = { kind: 'everyone' | 'department' | 'user'; id: string | null; level: 'view' | 'edit'; name: string | null };
	type Board = {
		id: string; title: string; version: number; level: 'view' | 'edit' | 'owner'; is_template: boolean; locked: boolean;
		owner: { id: string; name: string }; task: { id: string; number: number; title: string } | null; project: { id: string; name: string } | null;
		scene: { elements: El[]; appState: Record<string, unknown> }; files: BoardFile[]; access?: Grant[]; viewers: { id: string; name: string }[];
	};
	const id = $derived(page.params.id!);
	let board = $state<Board | null>(null);
	let sync = $state<BoardSync | null>(null);
	let exApi: any = null;
	let exLib: any = null;
	let selectedText = $state('');
	let title = $state('');

	onMount(async () => {
		try {
			board = await api.get<Board>(`/boards/${id}`);
			title = board.title;
			sync = new BoardSync(
				{
					load: (since) => api.get(`/boards/${id}/scene${since === null ? '' : `?since=${since}`}`),
					save: (body) => api.put(`/boards/${id}/scene`, body),
					addFile: async (key, dataURL) => { await api.post(`/boards/${id}/files`, { key, data_url: dataURL }); },
					file: (key) => requestBlob('GET', `/boards/${id}/files/${encodeURIComponent(key)}`)
				},
				board.level !== 'view'
			);
		} catch (e) {
			toasts.error(e);
			goto('/boards');
		}
	});
	let off: (() => void) | undefined;
	let beat: ReturnType<typeof setInterval> | undefined;
	onMount(() => {
		off = realtime.on('board.changed', (d: { id: string; version: number }) => {
			if (d.id === id && sync && d.version > sync.version) sync.pull();
		});
		// Keeps me on the list of who's looking, and catches anything missed.
		beat = setInterval(() => sync?.pull(), 20000);
	});
	onDestroy(() => {
		off?.();
		clearInterval(beat);
		sync?.flush();
	});

	function ready(a: any, lib: any) {
		exApi = a;
		exLib = lib;
		if (board && sync) sync.attach(a, { version: board.version, scene: board.scene, files: board.files, viewers: board.viewers }, lib.CaptureUpdateAction);
	}
	function changed(els: readonly El[], st: any, files: any) {
		sync?.changed(els, st, files);
		const ids = Object.keys(st?.selectedElementIds ?? {}).filter((k) => st.selectedElementIds[k]);
		selectedText = ids.length && exLib ? exLib.getTextFromElements(els.filter((e) => ids.includes(e.id)) as any).trim() : '';
	}

	async function rename() {
		if (!board || title.trim() === board.title || !title.trim()) return void (title = board?.title ?? '');
		board = { ...board, ...(await api.patch<Board>(`/boards/${id}`, { title })) };
	}
	async function toggle(field: 'is_template' | 'locked') {
		board = { ...board!, ...(await api.patch<Board>(`/boards/${id}`, { [field]: !board![field] })) };
	}
	async function remove() {
		if (!confirm(`Delete “${board?.title}”? This can't be undone.`)) return;
		await api.del(`/boards/${id}`);
		goto('/boards');
	}
	async function makeTask() {
		const lines = selectedText.split('\n').map((l) => l.trim()).filter(Boolean);
		if (!lines.length) return;
		try {
			const t = await api.post<{ id: string; number: number }>('/tasks', {
				title: lines[0].slice(0, 200),
				description: `${lines.slice(1).join('\n')}\n\nFrom the board “${board?.title}”: ${location.origin}/boards/${id}`.trim(),
				responsible_id: session.user?.id,
				project_id: board?.project?.id ?? null
			});
			toasts.success(`Task #${t.number} made`);
		} catch (e) {
			toasts.error(e);
		}
	}
	const boardChat = async () => (await api.post<{ chat_id: string }>(`/boards/${id}/chat`)).chat_id;

	// Sharing.
	let shareOpen = $state(false);
	let grants = $state<Grant[]>([]);
	let departments = $state<{ id: string; name: string }[]>([]);
	let links = $state<{ id: string; level: string; expires_at: string | null; password: boolean }[]>([]);
	let newLink = $state({ level: 'view', expires: '', password: '' });
	let madeUrl = $state('');
	let linksOn = $state(true);
	publicLinksAllowed().then((v) => (linksOn = v));
	async function openShare() {
		grants = (board?.access ?? []).map((g) => ({ ...g }));
		madeUrl = '';
		shareOpen = true;
		departments = await api.get<{ id: string; name: string }[]>('/departments').catch(() => []);
		links = (await api.get<{ items: typeof links }>(`/boards/${id}/links`)).items;
	}
	function addGrant(kind: Grant['kind'], gid: string | null, name: string | null) {
		if (grants.some((g) => g.kind === kind && g.id === gid)) return;
		grants = [...grants, { kind, id: gid, name, level: 'view' }];
	}
	async function saveAccess() {
		try {
			const r = await api.put<{ access: Grant[] }>(`/boards/${id}/access`, { grants: grants.map(({ kind, id: gid, level }) => ({ kind, id: gid, level })) });
			board = { ...board!, access: r.access };
			toasts.success('Sharing saved');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function makeLink() {
		try {
			const r = await api.post<{ url: string }>(`/boards/${id}/links`, { level: newLink.level, expires_at: newLink.expires ? new Date(`${newLink.expires}T23:59:59`).toISOString() : null, password: newLink.password || null });
			madeUrl = r.url;
			newLink = { level: 'view', expires: '', password: '' };
			links = (await api.get<{ items: typeof links }>(`/boards/${id}/links`)).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function dropLink(lid: string) {
		await api.del(`/boards/${id}/links/${lid}`);
		links = links.filter((l) => l.id !== lid);
	}
</script>

<svelte:head><title>{board?.title ?? 'Board'} · NERP</title></svelte:head>

{#if board}
	<div class="flex h-[calc(100dvh-7rem)] min-h-[480px] flex-col gap-2">
		<div class="flex flex-wrap items-center gap-2">
			<a class="btn btn-ghost px-2" href="/boards" aria-label="All boards">‹</a>
			{#if board.level === 'view'}
				<h1 class="text-lg font-semibold text-slate-900">{board.title}</h1>
				<span class="badge bg-slate-100 text-slate-600">View only</span>
			{:else}
				<input class="input w-64 py-1 text-lg font-semibold" aria-label="Board name" maxlength="200" bind:value={title} onblur={rename} onkeydown={(e) => e.key === 'Enter' && (e.currentTarget as HTMLInputElement).blur()} />
			{/if}
			{#if board.task}<a class="text-sm text-brand-600 hover:underline" href="/tasks/{board.task.id}">Task #{board.task.number}</a>{/if}
			{#if board.project}<a class="text-sm text-brand-600 hover:underline" href="/projects/{board.project.id}">{board.project.name}</a>{/if}
			<span class="text-xs text-slate-500" data-saving>{sync?.error ? `Not saved: ${sync.error}` : sync?.saving ? 'Saving…' : 'Saved'}</span>
			<div class="ml-auto flex flex-wrap items-center gap-2">
				{#if sync?.viewers.length}<span class="text-xs text-slate-500" data-viewers title="Looking at it now">👀 {sync.viewers.map((v) => v.name).join(', ')}</span>{/if}
				{#if selectedText && board.level !== 'view'}<button class="btn btn-secondary" onclick={makeTask}>Make a task</button>{/if}
				<ChatCallButtons chat={boardChat} call={false} />
				{#if board.level === 'owner'}
					<button class="btn btn-secondary" onclick={openShare}>Share</button>
					<button class="btn btn-ghost" aria-pressed={board.is_template} onclick={() => toggle('is_template')}>{board.is_template ? '★ Template' : '☆ Template'}</button>
					<button class="btn btn-ghost" aria-pressed={board.locked} onclick={() => toggle('locked')}>{board.locked ? '🔒 Locked' : 'Lock'}</button>
					<button class="btn btn-ghost hover:text-red-600" onclick={remove}>Delete</button>
				{/if}
			</div>
		</div>
		<div class="card flex-1 overflow-hidden">
			{#if sync}<Excalidraw elements={board.scene.elements} appState={board.scene.appState} readOnly={board.level === 'view'} name={board.title} onready={ready} onchange={changed} />{/if}
		</div>
	</div>
{/if}

<Modal bind:open={shareOpen} title="Share the board">
	<div class="space-y-4 text-sm">
		<div class="space-y-2">
			<span class="label">People in the company</span>
			{#each grants as g, i (g.kind + g.id)}
				<div class="flex items-center gap-2">
					<span class="flex-1 truncate">{g.kind === 'everyone' ? 'Everyone' : g.kind === 'department' ? `🏢 ${g.name}` : g.name}</span>
					<select class="input w-32 py-1" aria-label="Level for {g.name ?? 'everyone'}" bind:value={grants[i].level}><option value="view">Can view</option><option value="edit">Can edit</option></select>
					<button class="px-1 text-slate-400 hover:text-red-600" aria-label="Remove" onclick={() => (grants = grants.filter((_, j) => j !== i))}>×</button>
				</div>
			{/each}
			<div class="grid gap-2 sm:grid-cols-3">
				<UserPicker placeholder="Add a person…" onpick={(u) => u && addGrant('user', u.id, u.full_name)} />
				<select class="input" aria-label="Add a department" onchange={(e) => { const d = departments.find((x) => x.id === e.currentTarget.value); if (d) addGrant('department', d.id, d.name); e.currentTarget.value = ''; }}>
					<option value="">Add a department…</option>{#each departments as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
				</select>
				<button class="btn btn-secondary" onclick={() => addGrant('everyone', null, null)}>Everyone</button>
			</div>
			<button class="btn btn-primary" onclick={saveAccess}>Save</button>
		</div>
		<div class="space-y-2 border-t border-slate-100 pt-3">
			<span class="label">Links for anyone (also outside the company)</span>
			{#each links as l (l.id)}
				<div class="flex items-center gap-2 text-slate-600" data-link>
					<span class="flex-1">{l.level === 'edit' ? 'Can edit' : 'Can view'}{l.expires_at ? ` · until ${new Date(l.expires_at).toLocaleDateString()}` : ''}{l.password ? ' · password' : ''}</span>
					<button class="text-red-600 hover:underline" onclick={() => dropLink(l.id)}>Turn off</button>
				</div>
			{/each}
			{#if madeUrl}<p class="rounded-lg bg-emerald-50 p-2 text-emerald-900">Link (shown once): <code class="break-all" data-board-link>{madeUrl}</code></p>{/if}
			{#if linksOn}
			<div class="grid gap-2 sm:grid-cols-4">
				<select class="input" aria-label="Link level" bind:value={newLink.level}><option value="view">Can view</option><option value="edit">Can edit</option></select>
				<input class="input" type="date" aria-label="Works until" bind:value={newLink.expires} />
				<input class="input" type="text" aria-label="Link password" placeholder="Password (optional)" autocomplete="off" bind:value={newLink.password} />
				<button class="btn btn-secondary" onclick={makeLink}>Make a link</button>
			</div>
			{:else}<p class="text-xs text-amber-800" data-links-off>Links for anyone are switched off for the company.</p>{/if}
		</div>
	</div>
	{#snippet footer()}<button class="btn btn-secondary" onclick={() => (shareOpen = false)}>Close</button>{/snippet}
</Modal>
