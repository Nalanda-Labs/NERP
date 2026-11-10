<script lang="ts">
	// A knowledge base page: read it, edit it (Markdown with a preview), see and restore its
	// history, add sub-pages.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import KbTree from '#lib/components/KbTree.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Page = {
		id: string;
		space_id: string;
		space_name: string;
		parent_id: string | null;
		title: string;
		body: string;
		html: string;
		version: number;
		updated_by_name: string | null;
		updated_at: string;
		can_edit: boolean;
		path: { id: string; title: string }[];
		children: { id: string; title: string }[];
	};
	let p = $state<Page | null>(null);
	let tree = $state<{ id: string; parent_id: string | null; title: string }[]>([]);
	let missing = $state(false);
	let editing = $state(false);
	let draft = $state({ title: '', body: '' });
	let preview = $state('');
	let busy = $state(false);

	async function load(id: string) {
		try {
			p = await api.get<Page>(`/kb/pages/${id}`);
			missing = false;
			tree = (await api.get<{ pages: typeof tree }>(`/kb/spaces/${p.space_id}`)).pages;
			if (page.url.searchParams.has('edit') && p.can_edit) startEdit();
		} catch {
			missing = true;
		}
	}
	$effect(() => {
		editing = false;
		load(page.params.id ?? '');
	});
	function startEdit() {
		draft = { title: p!.title, body: p!.body };
		preview = p!.html;
		editing = true;
	}
	let t: ReturnType<typeof setTimeout> | undefined;
	function typed() {
		clearTimeout(t);
		t = setTimeout(async () => (preview = (await api.post<{ html: string }>('/kb/preview', { body: draft.body })).html), 400);
	}
	async function save() {
		busy = true;
		try {
			p = await api.patch<Page>(`/kb/pages/${p!.id}`, { ...draft, version: p!.version });
			editing = false;
			tree = (await api.get<{ pages: typeof tree }>(`/kb/spaces/${p.space_id}`)).pages;
			goto(`/kb/page/${p.id}`, { replace: true });
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	async function subPage() {
		const title = prompt('Title of the new page');
		if (!title?.trim()) return;
		const n = await api.post<{ id: string }>(`/kb/spaces/${p!.space_id}/pages`, { title, body: '', parent_id: p!.id });
		goto(`/kb/page/${n.id}?edit=1`);
	}
	async function remove() {
		if (!confirm(`Delete “${p!.title}”${p!.children.length ? ' and its sub-pages' : ''}?`)) return;
		await api.del(`/kb/pages/${p!.id}`);
		goto(p!.parent_id ? `/kb/page/${p!.parent_id}` : `/kb/${p!.space_id}`);
	}

	// History.
	let historyOpen = $state(false);
	let history = $state<{ version: number; title: string; saved_by: string | null; saved_at: string; length: number }[]>([]);
	let viewing = $state<{ version: number; title: string; html: string; saved_at: string } | null>(null);
	async function openHistory() {
		history = (await api.get<{ items: typeof history }>(`/kb/pages/${p!.id}/versions`)).items;
		viewing = null;
		historyOpen = true;
	}
	async function view(v: number) {
		viewing = await api.get(`/kb/pages/${p!.id}/versions/${v}`);
	}
	async function restore(v: number) {
		if (!confirm(`Restore version ${v}? It becomes the newest version; nothing is lost.`)) return;
		try {
			p = await api.post<Page>(`/kb/pages/${p!.id}/versions/${v}/restore`, {});
			historyOpen = false;
			toasts.success(`Version ${v} restored`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>{p?.title ?? 'Page'} · NERP</title></svelte:head>

{#if missing}
	<div class="card p-10 text-center text-slate-500">This page doesn't exist, or you can't read it.</div>
{:else if p}
	<div class="flex gap-6">
		<aside class="hidden w-60 shrink-0 lg:block" aria-label="Pages">
			<a href="/kb/{p.space_id}" class="mb-2 block truncate px-2 text-sm font-semibold text-slate-900 hover:underline">{p.space_name}</a>
			<KbTree pages={tree} current={p.id} />
		</aside>
		<div class="min-w-0 flex-1">
			<nav class="mb-2 flex flex-wrap gap-1 text-sm text-slate-500" aria-label="Breadcrumbs">
				<a class="hover:underline" href="/kb">Knowledge base</a> / <a class="hover:underline" href="/kb/{p.space_id}">{p.space_name}</a>
				{#each p.path as x (x.id)} / <a class="hover:underline" href="/kb/page/{x.id}">{x.title}</a>{/each}
			</nav>
			{#if editing}
				<div class="space-y-3">
					<input class="input text-xl font-semibold" maxlength="200" aria-label="Title" bind:value={draft.title} />
					<div class="grid gap-4 xl:grid-cols-2">
						<textarea class="input min-h-[60vh] font-mono text-sm" aria-label="Page text (Markdown)" bind:value={draft.body} oninput={typed} placeholder={'# Heading\n\nText with **bold**, lists, links and tables.'}></textarea>
						<div class="card prose-kb min-h-[60vh] overflow-auto p-5" aria-label="Preview">{@html preview}</div>
					</div>
					<p class="text-xs text-slate-500">Markdown: # headings, **bold**, *italic*, - lists, [links](https://…), | tables |, `code`. Link to another page with its address, e.g. /kb/page/….</p>
					<div class="flex gap-2"><button class="btn btn-primary" disabled={busy || !draft.title.trim()} onclick={save}>Save</button><button class="btn btn-ghost" onclick={() => (editing = false)}>Cancel</button></div>
				</div>
			{:else}
				<div class="flex flex-wrap items-start gap-3">
					<h1 class="min-w-0 flex-1 text-3xl font-semibold text-slate-900">{p.title}</h1>
					<button class="btn btn-ghost" onclick={openHistory}><Icon name="history" class="size-4" /> History</button>
					{#if p.can_edit}
						<button class="btn btn-secondary" onclick={subPage}><Icon name="plus" class="size-4" /> Sub-page</button>
						<button class="btn btn-primary" onclick={startEdit}><Icon name="edit" class="size-4" /> Edit</button>
					{/if}
				</div>
				<p class="mt-1 text-xs text-slate-500">Version {p.version} · {p.updated_by_name ?? 'someone'} · {formatRelative(p.updated_at)}</p>
				<article class="prose-kb mt-6 max-w-3xl" data-page-body>{@html p.html}</article>
				{#if !p.body.trim()}<p class="mt-6 text-slate-400">This page is empty.</p>{/if}
				{#if p.children.length}
					<div class="mt-8 max-w-3xl border-t border-slate-100 pt-4">
						<p class="mb-2 text-sm font-medium text-slate-500">In this section</p>
						<ul class="space-y-1">{#each p.children as c (c.id)}<li><a class="text-brand-600 hover:underline" href="/kb/page/{c.id}">{c.title}</a></li>{/each}</ul>
					</div>
				{/if}
				{#if p.can_edit}<button class="mt-8 text-sm text-slate-400 hover:text-red-600" onclick={remove}>Delete this page</button>{/if}
			{/if}
		</div>
	</div>
{/if}

<Modal bind:open={historyOpen} title="History">
	{#if viewing}
		<button class="mb-3 text-sm text-brand-600 hover:underline" onclick={() => (viewing = null)}>← All versions</button>
		<p class="mb-2 text-sm text-slate-500">Version {viewing.version}, {formatDateTime(viewing.saved_at)}</p>
		<h2 class="text-xl font-semibold">{viewing.title}</h2>
		<div class="prose-kb mt-3 max-h-[50vh] overflow-auto">{@html viewing.html}</div>
	{:else}
		<ul class="divide-y divide-slate-100">
			{#each history as h (h.version)}
				<li class="flex items-center gap-3 py-2 text-sm" data-version={h.version}>
					<span class="w-10 font-medium">v{h.version}</span>
					<span class="flex-1 text-slate-600">{h.saved_by ?? 'someone'} · {formatDateTime(h.saved_at)}</span>
					<button class="text-brand-600 hover:underline" onclick={() => view(h.version)}>View</button>
					{#if p?.can_edit && h.version !== p.version}<button class="text-brand-600 hover:underline" onclick={() => restore(h.version)}>Restore</button>{/if}
				</li>
			{/each}
		</ul>
	{/if}
	{#snippet footer()}<button class="btn btn-secondary" onclick={() => (historyOpen = false)}>Close</button>{/snippet}
</Modal>
