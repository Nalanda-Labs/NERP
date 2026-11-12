<script lang="ts">
	// One-click full company export: every table as CSV and the stored files, in a ZIP.
	import { onDestroy, onMount } from 'svelte';
	import { api, downloadUrl } from '#lib/api.ts';
	import { formatBytes, formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Export = { id: string; status: 'running' | 'done' | 'failed'; include_files: boolean; requested_by_name: string | null; file_name: string | null; size_bytes: number | null; sha256: string | null; tables: number | null; files: number | null; error: string | null; created_at: string; finished_at: string | null };
	let items = $state<Export[]>([]);
	let keepDays = $state(7);
	let withFiles = $state(true);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function load() {
		try {
			const r = await api.get<{ items: Export[]; keep_days: number }>('/admin/exports');
			items = r.items;
			keepDays = r.keep_days;
			clearTimeout(timer);
			if (items.some((x) => x.status === 'running')) timer = setTimeout(load, 2000);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	onDestroy(() => clearTimeout(timer));

	async function start() {
		try {
			await api.post('/admin/exports', { include_files: withFiles });
			toasts.success('Export started');
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function remove(x: Export) {
		try {
			await api.del(`/admin/exports/${x.id}`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	const running = $derived(items.some((x) => x.status === 'running'));
</script>

<section class="card space-y-3 p-4 text-sm" aria-label="Company export">
	<div class="flex flex-wrap items-center gap-3">
		<h2 class="font-semibold text-slate-900">Full company export</h2>
		<label class="ml-auto flex items-center gap-1.5"><input type="checkbox" bind:checked={withFiles} data-export-files /> With stored files</label>
		<button class="btn btn-primary" disabled={running} onclick={start} data-export-start>{running ? 'Exporting…' : 'Export everything'}</button>
	</div>
	<p class="text-slate-500">A ZIP with every table as CSV (one consistent snapshot), the stored files under their own names, and a manifest of columns, checksums and the audit trail's head — readable without NERP. Passwords, keys and other secrets are left out. Exports are kept for {keepDays} days.</p>
	{#if items.length}
		<ul class="divide-y divide-slate-100">
			{#each items as x (x.id)}
				<li class="flex flex-wrap items-center gap-2 py-2" data-export={x.status}>
					<span class="text-slate-700">{formatDateTime(x.created_at)}</span>
					<span class="text-xs text-slate-500">{x.requested_by_name ?? ''}{x.include_files ? '' : ' · without files'}</span>
					{#if x.status === 'running'}<span class="text-xs text-amber-700">Running…</span>
					{:else if x.status === 'failed'}<span class="text-xs text-red-700">Failed: {x.error}</span>
					{:else}<span class="text-xs text-slate-500">{x.tables} tables, {x.files} files, {formatBytes(x.size_bytes ?? 0)}</span>{/if}
					<span class="ml-auto flex gap-3">
						{#if x.status === 'done' && x.file_name}<button class="text-brand-700 hover:underline" onclick={() => downloadUrl(`/api/admin/exports/${x.id}/download`, x.file_name!)}>Download</button>{/if}
						{#if x.status !== 'running'}<button class="text-slate-500 hover:text-red-700" onclick={() => remove(x)}>Remove</button>{/if}
					</span>
					{#if x.sha256}<code class="w-full truncate text-xs text-slate-400" title="SHA-256 of the archive">{x.sha256}</code>{/if}
				</li>
			{/each}
		</ul>
	{/if}
</section>
