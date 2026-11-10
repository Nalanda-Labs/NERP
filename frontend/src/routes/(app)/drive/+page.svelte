<script lang="ts">
	// Drive: My drive, the company drive, what is shared with me, and the trash.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, downloadUrl, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import { formatBytes, formatDateTime, formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Entry = { id: string; kind: 'folder' | 'file'; space: string; parent_id: string | null; name: string; size_bytes: number; mime_type: string | null; version: number; owner_name: string | null; updated_by_name: string | null; updated_at: string; shared: boolean };
	type Detail = Entry & { level: 'own' | 'edit' | 'view'; versions?: { version: number; size_bytes: number; by: string | null; at: string }[]; shares?: { user_id: string | null; department_id: string | null; level: string; name: string }[]; links?: { id: string; expires_at: string | null; downloads: number; created_at: string }[]; url?: string };
	const space = $derived(page.url.searchParams.get('space') ?? 'personal');
	const folder = $derived(page.url.searchParams.get('folder'));
	// A workgroup's files (space=group&project=…).
	const project = $derived(page.url.searchParams.get('project'));
	let items = $state<Entry[]>([]);
	let path = $state<{ id: string; name: string }[]>([]);
	let canAdd = $state(false);
	let group = $state<{ id: string; name: string } | null>(null);
	let trashItems = $state<{ id: string; kind: string; name: string; deleted_at: string; purged_on: string }[]>([]);
	let q = $state('');
	let hits = $state<Entry[] | null>(null);
	let dragging = $state(false);
	let busy = $state(false);

	async function load() {
		if (space === 'trash' && !folder) {
			trashItems = (await api.get<{ items: typeof trashItems }>('/drive/trash')).items;
			return;
		}
		try {
			const r = await api.get<{ items: Entry[]; path: typeof path; can_add: boolean; project: typeof group }>('/drive' + query({ space: folder ? undefined : space, folder: folder ?? undefined, project_id: folder ? undefined : (project ?? undefined) }));
			items = r.items;
			path = r.path;
			canAdd = r.can_add;
			group = r.project;
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		space;
		folder;
		hits = null;
		q = '';
		load();
		const f = page.url.searchParams.get('file');
		if (f) openDetail(f);
	});
	const go = (s: string, f: string | null = null) => goto(`/drive?${f ? `folder=${f}` : `space=${s}`}`);
	const TABS = [['personal', 'My drive'], ['company', 'Company'], ['shared', 'Shared with me'], ['trash', 'Trash']] as const;
	const where = $derived(folder ? (items[0]?.space ?? space) : space);

	async function upload(files: FileList | File[]) {
		busy = true;
		for (const f of Array.from(files)) {
			try {
				const fd = new FormData();
				fd.append('file', f);
				const up = await api.post<{ id: string }>('/files', fd);
				await api.post('/drive/files', { file_id: up.id, name: f.name, parent_id: folder, space: folder ? undefined : space, project_id: folder ? undefined : project });
			} catch (e) {
				toasts.error(e);
			}
		}
		busy = false;
		load();
	}
	async function newFolder() {
		const name = prompt('Folder name');
		if (!name?.trim()) return;
		try {
			await api.post('/drive/folders', { name, parent_id: folder, space: folder ? undefined : space, project_id: folder ? undefined : project });
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
	function drop(e: DragEvent) {
		e.preventDefault();
		dragging = false;
		if (canAdd && e.dataTransfer?.files.length) upload(e.dataTransfer.files);
	}
	let t: ReturnType<typeof setTimeout> | undefined;
	function search() {
		clearTimeout(t);
		if (q.trim().length < 2) return void (hits = null);
		t = setTimeout(async () => (hits = (await api.get<{ items: Entry[] }>(`/drive/search?q=${encodeURIComponent(q.trim())}`)).items), 250);
	}

	// Details: versions, sharing, links, rename, delete.
	let detail = $state<Detail | null>(null);
	let detailOpen = $state(false);
	let sharePeople = $state<{ id: string; name: string; avatar_file_id: string | null }[]>([]);
	let shareLevel = $state<'view' | 'edit'>('view');
	let linkDays = $state('');
	let newLink = $state<string | null>(null);
	async function openDetail(id: string) {
		detail = await api.get<Detail>(`/drive/nodes/${id}`);
		sharePeople = (detail.shares ?? []).filter((s) => s.user_id).map((s) => ({ id: s.user_id!, name: s.name, avatar_file_id: null }));
		shareLevel = (detail.shares?.find((s) => s.user_id)?.level as 'view' | 'edit') ?? 'view';
		newLink = null;
		detailOpen = true;
	}
	function open(e: Entry) {
		if (e.kind === 'folder') go(space, e.id);
		else openDetail(e.id);
	}
	async function rename() {
		const name = prompt('New name', detail!.name);
		if (!name?.trim() || name === detail!.name) return;
		try {
			detail = await api.patch<Detail>(`/drive/nodes/${detail!.id}`, { name });
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function remove() {
		if (!confirm(`Move “${detail!.name}” to the trash?`)) return;
		await api.del(`/drive/nodes/${detail!.id}`);
		detailOpen = false;
		load();
	}
	async function saveShares() {
		try {
			detail = await api.put<Detail>(`/drive/nodes/${detail!.id}/shares`, sharePeople.map((p) => ({ user_id: p.id, level: shareLevel })));
			toasts.success('Sharing saved');
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function makeLink() {
		try {
			const r = await api.post<Detail>(`/drive/nodes/${detail!.id}/links`, { expires_days: linkDays ? Number(linkDays) : null });
			newLink = r.url ?? null;
			detail = r;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function dropLink(id: string) {
		detail = await api.del<Detail>(`/drive/links/${id}`);
	}
	async function restoreVersion(v: number) {
		detail = await api.post<Detail>(`/drive/nodes/${detail!.id}/versions/${v}/restore`, {});
		load();
	}
	async function putBack(id: string) {
		await api.post(`/drive/nodes/${id}/restore`, {});
		load();
	}
	async function destroy(id: string) {
		if (!confirm('Delete for good? This can\'t be undone.')) return;
		await api.del(`/drive/trash/${id}`);
		load();
	}
	const icon = (e: { kind: string; mime_type?: string | null }) => (e.kind === 'folder' ? 'folder' : 'file');
</script>

<svelte:head><title>Drive · NERP</title></svelte:head>

<div class="space-y-4" role="region" aria-label="Drive" ondragover={(e) => (e.preventDefault(), canAdd && (dragging = true))} ondragleave={() => (dragging = false)} ondrop={drop}>
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Drive</h1>
		<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist">
			{#each TABS as [k, l] (k)}<button role="tab" aria-selected={folder ? where === k : space === k} class="rounded-full px-3 py-1.5 font-medium {(folder ? where === k : space === k) ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => go(k)}>{l}</button>{/each}
		</div>
		<input class="input ml-auto w-60" type="search" placeholder="Find a file…" aria-label="Find a file" bind:value={q} oninput={search} />
		{#if canAdd && space !== 'trash'}
			<button class="btn btn-secondary" onclick={newFolder}><Icon name="folder" class="size-4" /> New folder</button>
			<label class="btn btn-primary cursor-pointer"><Icon name="upload" class="size-4" /> {busy ? 'Uploading…' : 'Upload'}<input type="file" multiple class="sr-only" aria-label="Upload files" onchange={(e) => upload((e.currentTarget as HTMLInputElement).files!)} /></label>
		{/if}
	</div>

	{#if path.length || group}
		<nav class="flex flex-wrap items-center gap-1 text-sm text-slate-500" aria-label="Folders">
			{#if group}
				<a class="hover:underline" href="/projects/{group.id}?tab=files">{group.name}</a>
			{:else}
				<button class="hover:underline" onclick={() => go(where)}>{TABS.find((x) => x[0] === where)?.[1]}</button>
			{/if}
			{#each path as p (p.id)} / <button class="hover:underline {p.id === folder ? 'font-medium text-slate-900' : ''}" onclick={() => go(where, p.id)}>{p.name}</button>{/each}
		</nav>
	{/if}

	{#if hits}
		<div class="card divide-y divide-slate-50" aria-label="Search results">
			{#each hits as h (h.id)}<button class="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-slate-50" onclick={() => open(h)}><Icon name={icon(h)} class="size-4 text-slate-400" /><span class="flex-1">{h.name}</span><span class="text-xs text-slate-400">{h.space === 'company' ? 'Company' : h.owner_name}</span></button>{:else}<p class="px-4 py-6 text-center text-sm text-slate-500">Nothing found.</p>{/each}
		</div>
	{:else if space === 'trash' && !folder}
		<div class="card divide-y divide-slate-50">
			{#each trashItems as x (x.id)}
				<div class="flex items-center gap-3 px-4 py-2.5 text-sm" data-trash={x.name}>
					<Icon name={x.kind === 'folder' ? 'folder' : 'file'} class="size-4 text-slate-400" /><span class="flex-1">{x.name}</span>
					<span class="text-xs text-slate-500">deleted {formatRelative(x.deleted_at)} · gone {formatRelative(x.purged_on)}</span>
					<button class="text-brand-600 hover:underline" onclick={() => putBack(x.id)}>Put back</button>
					<button class="text-slate-400 hover:text-red-600" onclick={() => destroy(x.id)}>Delete for good</button>
				</div>
			{:else}<p class="px-4 py-10 text-center text-sm text-slate-500">The trash is empty. Things stay here for 30 days.</p>{/each}
		</div>
	{:else}
		<div class="card overflow-x-auto {dragging ? 'ring-2 ring-brand-400' : ''}">
			<table class="min-w-full text-sm">
				<thead class="border-b border-slate-100 text-left text-xs text-slate-500"><tr><th class="px-4 py-3">Name</th><th class="px-4 py-3">Changed</th><th class="px-4 py-3 text-right">Size</th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each items as e (e.id)}
						<tr class="cursor-pointer hover:bg-slate-50" data-node={e.name} onclick={() => open(e)}>
							<td class="px-4 py-2.5"><span class="flex items-center gap-2"><Icon name={icon(e)} class="size-4 {e.kind === 'folder' ? 'text-brand-600' : 'text-slate-400'}" /><span class="font-medium text-slate-900">{e.name}</span>{#if e.shared}<span class="badge bg-slate-100 text-slate-600">shared</span>{/if}{#if e.version > 1}<span class="text-xs text-slate-400">v{e.version}</span>{/if}</span></td>
							<td class="px-4 py-2.5 whitespace-nowrap text-slate-500">{formatRelative(e.updated_at)}{e.updated_by_name ? ` · ${e.updated_by_name}` : ''}</td>
							<td class="px-4 py-2.5 text-right text-slate-500 tabular-nums">{e.kind === 'file' ? formatBytes(e.size_bytes) : ''}</td>
						</tr>
					{:else}
						<tr><td colspan="3" class="px-4 py-14 text-center text-slate-500">{space === 'shared' && !folder ? 'Nothing is shared with you yet.' : canAdd ? 'Empty. Upload files or drop them here.' : 'Empty.'}</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<Modal bind:open={detailOpen} title={detail?.name ?? ''}>
	{#if detail}
		<div class="space-y-5 text-sm" data-detail>
			<p class="text-slate-500">{detail.kind === 'file' ? `${formatBytes(detail.size_bytes)} · ` : ''}changed {formatDateTime(detail.updated_at)}{detail.updated_by_name ? ` by ${detail.updated_by_name}` : ''}</p>
			{#if detail.kind === 'file'}
				<div class="flex gap-2"><button class="btn btn-primary" onclick={() => downloadUrl(`/api/drive/nodes/${detail!.id}/download`, detail!.name).catch(toasts.error)}><Icon name="download" class="size-4" /> Download</button>
					{#if detail.level !== 'view'}<label class="btn btn-secondary cursor-pointer">New version<input type="file" class="sr-only" aria-label="New version" onchange={async (e) => { const f = (e.currentTarget as HTMLInputElement).files?.[0]; if (!f) return; const fd = new FormData(); fd.append('file', f); const up = await api.post<{ id: string }>('/files', fd); await api.post('/drive/files', { file_id: up.id, name: detail!.name, parent_id: detail!.parent_id, space: detail!.space }); openDetail(detail!.id); load(); }} /></label>{/if}</div>
				<section aria-label="Versions">
					<h3 class="mb-1 font-medium">Versions</h3>
					<ul class="divide-y divide-slate-100">{#each detail.versions ?? [] as v (v.version)}<li class="flex items-center gap-3 py-1.5" data-version={v.version}><span class="w-8 font-medium">v{v.version}</span><span class="flex-1 text-slate-600">{v.by ?? ''} · {formatDateTime(v.at)} · {formatBytes(v.size_bytes)}</span><button class="text-brand-600 hover:underline" onclick={() => downloadUrl(`/api/drive/nodes/${detail!.id}/download?version=${v.version}`, detail!.name)}>Download</button>{#if detail.level !== 'view' && v.version !== detail.version}<button class="text-brand-600 hover:underline" onclick={() => restoreVersion(v.version)}>Restore</button>{/if}</li>{/each}</ul>
				</section>
			{/if}
			{#if detail.level === 'own' && detail.space === 'personal'}
				<section aria-label="Sharing" class="space-y-2">
					<h3 class="font-medium">Shared with</h3>
					<MultiUserPicker bind:value={sharePeople} placeholder="Share with…" />
					<div class="flex items-center gap-2"><select class="input w-auto" aria-label="Share level" bind:value={shareLevel}><option value="view">Can view</option><option value="edit">Can edit</option></select><button class="btn btn-secondary" onclick={saveShares}>Save sharing</button></div>
				</section>
			{/if}
			{#if detail.level === 'own' && detail.kind === 'file'}
				<section aria-label="Public links" class="space-y-2">
					<h3 class="font-medium">Public links</h3>
					{#each detail.links ?? [] as l (l.id)}<div class="flex items-center gap-3 text-xs"><span class="flex-1 text-slate-600">Made {formatRelative(l.created_at)} · {l.expires_at ? `until ${formatDateTime(l.expires_at)}` : 'no end'} · {l.downloads} download{l.downloads === 1 ? '' : 's'}</span><button class="text-slate-400 hover:text-red-600" onclick={() => dropLink(l.id)}>Remove</button></div>{/each}
					{#if newLink}<code class="block rounded-lg bg-slate-50 px-3 py-2 text-xs break-all" data-link>{newLink}</code><p class="text-xs text-slate-500">Copy it now: it isn't shown again. Anyone with it can download the file.</p>{/if}
					<div class="flex items-center gap-2"><input class="input w-28" type="number" min="1" max="365" placeholder="Days" aria-label="Days the link works" bind:value={linkDays} /><button class="btn btn-secondary" onclick={makeLink}>Make a link</button></div>
				</section>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		{#if detail?.level === 'own'}<button class="btn btn-ghost mr-auto hover:text-red-600" onclick={remove}>Delete</button><button class="btn btn-secondary" onclick={rename}>Rename</button>{/if}
		<button class="btn btn-primary" onclick={() => (detailOpen = false)}>Close</button>
	{/snippet}
</Modal>
