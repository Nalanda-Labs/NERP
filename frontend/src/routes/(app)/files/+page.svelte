<script lang="ts">
	import { onMount } from 'svelte';
	import { api, downloadFile, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatBytes, formatDateTime } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM, type FileMeta, type Page } from '#lib/types.ts';

	const PAGE_SIZE = 50;
	let data = $state<Page<FileMeta> | null>(null);
	let offset = $state(0);
	let all = $state(false);
	let uploading = $state(0);
	let dragging = $state(false);
	let input: HTMLInputElement | undefined = $state();

	const canManage = $derived(session.can(PERM.FILES_MANAGE));

	async function load() {
		data = await api.get<Page<FileMeta>>('/files' + query({ all: all || undefined, limit: PAGE_SIZE, offset }));
	}

	async function upload(files: FileList | null) {
		if (!files?.length) return;
		for (const f of Array.from(files)) {
			uploading++;
			const fd = new FormData();
			fd.append('file', f);
			try {
				await api.post<FileMeta>('/files', fd);
			} catch (err) {
				toasts.error(err);
			} finally {
				uploading--;
			}
		}
		offset = 0;
		await load();
		if (input) input.value = '';
	}

	async function download(f: FileMeta) {
		try {
			await downloadFile(f.id, f.original_name);
		} catch (err) {
			toasts.error(err);
		}
	}

	async function remove(f: FileMeta) {
		if (!confirm(`Delete "${f.original_name}"?`)) return;
		try {
			await api.del(`/files/${f.id}`);
			toasts.success('File deleted');
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}

	onMount(load);
</script>

<svelte:head><title>Files · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">{all ? 'All files' : 'My files'}</h1>
			<p class="text-sm text-slate-500">{data ? `${data.total} files` : ' '}</p>
		</div>
		<div class="flex items-center gap-3">
			{#if canManage}
				<label class="flex items-center gap-2 text-sm text-slate-600">
					<input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={all} onchange={() => ((offset = 0), load())} />
					Show everyone's files
				</label>
			{/if}
			<button class="btn btn-primary" onclick={() => input?.click()} disabled={uploading > 0}>
				<Icon name="upload" class="size-4" /> {uploading ? 'Uploading…' : 'Upload'}
			</button>
			<input bind:this={input} type="file" multiple class="hidden" onchange={(e) => upload(e.currentTarget.files)} />
		</div>
	</div>

	<div
		role="region"
		aria-label="Drop files to upload"
		class="card overflow-hidden transition {dragging ? 'ring-2 ring-brand-500' : ''}"
		ondragover={(e) => {
			e.preventDefault();
			dragging = true;
		}}
		ondragleave={() => (dragging = false)}
		ondrop={(e) => {
			e.preventDefault();
			dragging = false;
			upload(e.dataTransfer?.files ?? null);
		}}
	>
		{#if data && data.items.length}
			<table class="min-w-full text-sm">
				<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
					<tr>
						<th class="px-5 py-3">Name</th>
						<th class="hidden px-5 py-3 sm:table-cell">Size</th>
						<th class="hidden px-5 py-3 md:table-cell">Uploaded</th>
						<th class="px-5 py-3"></th>
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-100">
					{#each data.items as f (f.id)}
						<tr class="hover:bg-slate-50">
							<td class="px-5 py-3">
								<div class="flex items-center gap-3">
									<Icon name="file" class="size-5 shrink-0 text-slate-400" />
									<span class="min-w-0">
										<span class="block truncate font-medium text-slate-800">{f.original_name}</span>
										<span class="block truncate text-xs text-slate-400">{f.mime_type}{f.is_public ? ' · public' : ''}</span>
									</span>
								</div>
							</td>
							<td class="hidden px-5 py-3 text-slate-600 sm:table-cell">{formatBytes(f.size_bytes)}</td>
							<td class="hidden px-5 py-3 text-slate-600 md:table-cell">{formatDateTime(f.created_at)}</td>
							<td class="px-5 py-3">
								<div class="flex justify-end gap-1">
									<button class="btn btn-ghost p-1.5" aria-label="Download" onclick={() => download(f)}><Icon name="download" class="size-4" /></button>
									<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Delete" onclick={() => remove(f)}><Icon name="trash" class="size-4" /></button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
			{#if data.total > PAGE_SIZE}
				<div class="flex items-center justify-between border-t border-slate-200 px-5 py-3 text-sm text-slate-500">
					<span>{offset + 1}–{Math.min(offset + PAGE_SIZE, data.total)} of {data.total}</span>
					<div class="flex gap-2">
						<button class="btn btn-secondary" disabled={offset === 0} onclick={() => ((offset -= PAGE_SIZE), load())}>Previous</button>
						<button class="btn btn-secondary" disabled={offset + PAGE_SIZE >= data.total} onclick={() => ((offset += PAGE_SIZE), load())}>Next</button>
					</div>
				</div>
			{/if}
		{:else}
			<div class="flex flex-col items-center gap-2 px-5 py-16 text-center">
				<Icon name="upload" class="size-8 text-slate-300" />
				<p class="font-medium text-slate-700">{data ? 'No files yet' : 'Loading…'}</p>
				<p class="text-sm text-slate-500">Drag files here, or use the Upload button.</p>
			</div>
		{/if}
	</div>
</div>
