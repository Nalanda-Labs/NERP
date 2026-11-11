<script lang="ts">
	// A knowledge base page: read it (with its contents), edit it in the Markdown editor
	// (pictures and files, links to other pages, live preview), see and restore its history,
	// add sub-pages.
	import { beforeNavigate, goto, replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import KbTree from '#lib/components/KbTree.svelte';
	import MarkdownEditor, { type Uploaded } from '#lib/components/MarkdownEditor.svelte';
	import MarkdownView from '#lib/components/MarkdownView.svelte';
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
	let busy = $state(false);
	const dirty = $derived(editing && !!p && (draft.title !== p.title || draft.body !== p.body));

	async function load(id: string) {
		try {
			p = await api.get<Page>(`/kb/pages/${id}`);
			missing = false;
			tree = (await api.get<{ pages: typeof tree }>(`/kb/spaces/${p.space_id}`)).pages;
			if (page.url.searchParams.has('edit') && p.can_edit) startEdit(true);
		} catch {
			missing = true;
		}
	}
	$effect(() => {
		editing = false;
		load(page.params.id ?? '');
	});
	// While editing, the address says so (?edit=1) and unsaved changes are kept in this
	// browser, so a reload comes back to the editor with them.
	const draftKey = (id: string) => `nerp:kb-draft:${id}`;
	function readDraft(id: string): { title: string; body: string; version: number } | null {
		try {
			const raw = localStorage.getItem(draftKey(id));
			return raw ? JSON.parse(raw) : null;
		} catch {
			return null;
		}
	}
	function dropDraft(id: string) {
		try {
			localStorage.removeItem(draftKey(id));
		} catch {
			/* storage unavailable */
		}
	}
	function setEditUrl(on: boolean) {
		const url = new URL(page.url.href);
		if (on === url.searchParams.has('edit')) return;
		if (on) url.searchParams.set('edit', '1');
		else url.searchParams.delete('edit');
		replaceState(url, {});
	}
	function startEdit(restore = false) {
		const kept = restore ? readDraft(p!.id) : null;
		draft = kept ? { title: kept.title, body: kept.body } : { title: p!.title, body: p!.body };
		if (kept) toasts.success(kept.version === p!.version ? 'Your unsaved changes are back' : 'Your unsaved changes are back; someone saved the page meanwhile');
		editing = true;
		setEditUrl(true);
	}
	function stopEditing() {
		editing = false;
		dropDraft(p!.id);
		setEditUrl(false);
	}
	$effect(() => {
		if (!editing || !p) return;
		const d = { title: draft.title, body: draft.body, version: p.version };
		try {
			if (dirty) localStorage.setItem(draftKey(p.id), JSON.stringify(d));
			else localStorage.removeItem(draftKey(p.id));
		} catch {
			/* storage unavailable: nothing is kept */
		}
	});
	const renderMd = async (md: string) => (await api.post<{ html: string }>('/kb/preview', { body: md })).html;
	async function upload(file: File): Promise<Uploaded> {
		const fd = new FormData();
		fd.append('file', file, file.name);
		const f = await api.post<{ id: string }>('/files', fd);
		return api.post<Uploaded>(`/kb/pages/${p!.id}/files`, { file_id: f.id });
	}
	const links = $derived(tree.filter((x) => x.id !== p?.id).map((x) => ({ id: x.id, title: x.title, href: `/kb/page/${x.id}` })));
	// The page's contents: its headings.
	const toc = $derived(p ? [...p.html.matchAll(/<h([123]) id="(h-[^"]+)">([\s\S]*?)<\/h\1>/g)].map((m) => ({ level: Number(m[1]), id: m[2], text: m[3].replace(/<[^>]+>/g, '') })) : []);
	beforeNavigate((nav) => {
		// Reloading or closing the tab keeps the changes (see above); going elsewhere asks.
		if (nav.type === 'leave') return;
		if (dirty && !busy && !confirm('Leave without saving your changes?')) nav.cancel();
	});
	function cancel() {
		if (dirty && !confirm('Throw away your changes?')) return;
		stopEditing();
	}
	async function save() {
		if (busy || !draft.title.trim()) return;
		busy = true;
		try {
			p = await api.patch<Page>(`/kb/pages/${p!.id}`, { ...draft, version: p!.version });
			editing = false;
			dropDraft(p.id);
			tree = (await api.get<{ pages: typeof tree }>(`/kb/spaces/${p.space_id}`)).pages;
			toasts.success('Saved');
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
		{#if !editing}
			<aside class="hidden w-60 shrink-0 lg:block" aria-label="Pages">
				<a href="/kb/{p.space_id}" class="mb-2 block truncate px-2 text-sm font-semibold text-slate-900 hover:underline">{p.space_name}</a>
				<KbTree pages={tree} current={p.id} />
			</aside>
		{/if}
		<div class="min-w-0 flex-1">
			<nav class="mb-2 flex flex-wrap gap-1 text-sm text-slate-500" aria-label="Breadcrumbs">
				<a class="hover:underline" href="/kb">Knowledge base</a> / <a class="hover:underline" href="/kb/{p.space_id}">{p.space_name}</a>
				{#each p.path as x (x.id)} / <a class="hover:underline" href="/kb/page/{x.id}">{x.title}</a>{/each}
			</nav>
			{#if editing}
				<div class="mt-4 space-y-4">
					<div class="flex flex-wrap items-center gap-2">
						<input class="input min-w-0 flex-1 text-xl font-semibold" maxlength="200" aria-label="Title" bind:value={draft.title} />
						<button class="btn btn-primary" disabled={busy || !draft.title.trim()} onclick={save}>{busy ? 'Saving…' : 'Save'}</button>
						<button class="btn btn-ghost" onclick={cancel}>Cancel</button>
					</div>
					<MarkdownEditor class="h-[calc(100dvh-15rem)] min-h-[480px]" bind:value={draft.body} render={renderMd} {upload} {links} onsave={save} label="Page text (Markdown)" placeholder={'# Heading\n\nWrite with **bold**, lists, links, tables, pictures… Type [[ to link a page.'} />
					{#if dirty}<p class="text-xs text-amber-700">Unsaved changes · Ctrl+S saves</p>{/if}
				</div>
			{:else}
				<div class="flex flex-wrap items-start gap-3">
					<h1 class="min-w-0 flex-1 text-3xl font-semibold text-slate-900">{p.title}</h1>
					<button class="btn btn-ghost" onclick={openHistory}><Icon name="history" class="size-4" /> History</button>
					{#if p.can_edit}
						<button class="btn btn-secondary" onclick={subPage}><Icon name="plus" class="size-4" /> Sub-page</button>
						<button class="btn btn-primary" onclick={() => startEdit()}><Icon name="edit" class="size-4" /> Edit</button>
					{/if}
				</div>
				<p class="mt-1 text-xs text-slate-500">Version {p.version} · {p.updated_by_name ?? 'someone'} · {formatRelative(p.updated_at)}</p>
				<div class="mt-6 flex gap-8">
					<article class="min-w-0 max-w-3xl flex-1" data-page-body><MarkdownView html={p.html} /></article>
					{#if toc.length >= 3}
						<nav class="sticky top-20 hidden max-h-[70vh] w-56 shrink-0 self-start overflow-auto text-sm xl:block" aria-label="On this page" data-toc>
							<p class="mb-2 text-xs font-semibold tracking-wide text-slate-500 uppercase">On this page</p>
							<ul class="space-y-1 border-l border-slate-100">
								{#each toc as h (h.id)}<li><a href="#{h.id}" class="block truncate border-l-2 border-transparent py-0.5 text-slate-600 hover:border-brand-500 hover:text-slate-900 {h.level === 3 ? 'pl-6' : h.level === 2 ? 'pl-4' : 'pl-2 font-medium'}" style="margin-left:-1px">{h.text}</a></li>{/each}
							</ul>
						</nav>
					{/if}
				</div>
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
		<div class="mt-3 max-h-[50vh] overflow-auto"><MarkdownView html={viewing.html} /></div>
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
