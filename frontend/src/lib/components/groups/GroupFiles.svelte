<script lang="ts">
	// A workgroup's files: folders, uploads, downloads. Versions, links and the rest are in
	// the drive ("Open in Drive").
	import { api, downloadUrl, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatBytes, formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { projectId }: { projectId: string } = $props();

	type Entry = { id: string; kind: 'folder' | 'file'; name: string; size_bytes: number; version: number; updated_by_name: string | null; updated_at: string };
	let folder = $state<string | null>(null);
	let items = $state<Entry[] | null>(null);
	let path = $state<{ id: string; name: string }[]>([]);
	let canAdd = $state(false);
	let busy = $state(false);
	let dragging = $state(false);

	async function load() {
		try {
			const r = await api.get<{ items: Entry[]; path: typeof path; can_add: boolean }>('/drive' + query(folder ? { folder } : { space: 'group', project_id: projectId }));
			items = r.items;
			path = r.path;
			canAdd = r.can_add;
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		folder;
		load();
	});
	const where = () => (folder ? { parent_id: folder } : { space: 'group', project_id: projectId });

	async function upload(list: FileList | File[]) {
		busy = true;
		for (const f of Array.from(list)) {
			try {
				const fd = new FormData();
				fd.append('file', f);
				const up = await api.post<{ id: string }>('/files', fd);
				await api.post('/drive/files', { file_id: up.id, name: f.name, ...where() });
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
			await api.post('/drive/folders', { name, ...where() });
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
</script>

<div
	class="card max-w-4xl {dragging ? 'ring-2 ring-brand-400' : ''}"
	role="region"
	aria-label="Group files"
	ondragover={(e) => (e.preventDefault(), (dragging = canAdd))}
	ondragleave={() => (dragging = false)}
	ondrop={drop}
>
	<div class="flex flex-wrap items-center gap-2 border-b border-slate-100 p-4">
		<nav class="flex min-w-0 flex-1 flex-wrap items-center gap-1 text-sm" aria-label="Group folders">
			<button class="font-medium {folder ? 'text-brand-700 hover:underline' : 'text-slate-900'}" onclick={() => (folder = null)}>Files</button>
			{#each path as p (p.id)}
				<Icon name="chevronRight" class="size-4 text-slate-400" />
				<button class="font-medium {p.id === folder ? 'text-slate-900' : 'text-brand-700 hover:underline'}" onclick={() => (folder = p.id)}>{p.name}</button>
			{/each}
		</nav>
		{#if canAdd}
			<button class="btn btn-secondary" onclick={newFolder}><Icon name="folder" class="size-4" /> New folder</button>
			<label class="btn btn-primary cursor-pointer {busy ? 'opacity-60' : ''}"><Icon name="upload" class="size-4" /> Upload<input type="file" multiple class="sr-only" aria-label="Upload files to the group" onchange={(e) => upload(e.currentTarget.files ?? [])} /></label>
		{/if}
		<a class="btn btn-ghost" href="/drive?{folder ? `folder=${folder}` : `space=group&project=${projectId}`}">Open in Drive</a>
	</div>
	{#if items}
		<ul class="divide-y divide-slate-100">
			{#each items as e (e.id)}
				<li class="flex items-center gap-3 px-4 py-2.5" data-group-node={e.name}>
					<Icon name={e.kind === 'folder' ? 'folder' : 'file'} class="size-5 shrink-0 {e.kind === 'folder' ? 'text-amber-500' : 'text-slate-400'}" />
					{#if e.kind === 'folder'}
						<button class="min-w-0 flex-1 truncate text-left text-sm font-medium text-slate-800 hover:text-brand-700" onclick={() => (folder = e.id)}>{e.name}</button>
					{:else}
						<button class="min-w-0 flex-1 truncate text-left text-sm font-medium text-slate-800 hover:text-brand-700" onclick={() => downloadUrl(`/api/drive/nodes/${e.id}/download`, e.name).catch(toasts.error)}>{e.name}</button>
						<span class="text-xs text-slate-500">{formatBytes(e.size_bytes)}{e.version > 1 ? ` · v${e.version}` : ''}</span>
					{/if}
					<span class="hidden w-40 truncate text-right text-xs text-slate-400 sm:block">{e.updated_by_name ?? ''} · {formatRelative(e.updated_at)}</span>
				</li>
			{:else}
				<li class="p-10 text-center text-sm text-slate-500">{canAdd ? 'No files yet — upload some or drop them here.' : 'No files yet.'}</li>
			{/each}
		</ul>
	{/if}
</div>
