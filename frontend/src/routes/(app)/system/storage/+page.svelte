<script lang="ts">
	// Storage: what the uploaded files take, where, of what kind and whose; the largest files;
	// free disk space; and cleaning up the drive's trash and old file versions.
	import { onMount } from 'svelte';
	import { api, downloadFile } from '#lib/api.ts';
	import { formatBytes, formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Row = { key: string; files: number; bytes: number };
	type Storage = {
		files: number;
		bytes: number;
		disk: { total: number; free: number } | null;
		areas: Row[];
		kinds: Row[];
		people: { id: string | null; name: string | null; files: number; bytes: number }[];
		largest: {
			id: string;
			name: string;
			bytes: number;
			owner: string | null;
			created_at: string;
			mime_type: string;
		}[];
		encryption: {
			enabled: boolean;
			key_id: string | null;
			encrypted: number;
			other_key: number;
			not_encrypted: number;
			converting: boolean;
			keep_deleted_days: number;
			deleted: { files: number; bytes: number };
		};
		cleanup: {
			old_versions: { count: number; bytes: number };
			drive_trash: { count: number; bytes: number };
		};
	};
	const AREA: Record<string, string> = {
		drive: 'Drive',
		drive_versions: 'Older versions of drive files',
		drive_trash: 'Drive trash',
		chats: 'Chats',
		other: 'Everything else (tasks, CRM, feed, mail…)'
	};
	const KIND: Record<string, string> = {
		images: 'Images',
		video: 'Video',
		audio: 'Audio',
		pdf: 'PDF',
		documents: 'Office documents',
		archives: 'Archives',
		other: 'Other'
	};
	let s = $state<Storage | null>(null);
	let keep = $state(3);
	let days = $state(30);
	let busy = $state(false);

	async function load() {
		try {
			s = await api.get<Storage>('/admin/storage');
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	async function clean(what: 'drive_trash' | 'old_versions') {
		const msg =
			what === 'drive_trash'
				? 'Empty everyone’s drive trash now? This cannot be undone.'
				: `Delete older versions of drive files, keeping the last ${keep} of each, if older than ${days} days?`;
		if (!confirm(msg)) return;
		busy = true;
		try {
			const r = await api.post<{ removed: number; bytes: number }>(
				'/admin/storage/cleanup',
				what === 'drive_trash' ? { what } : { what, keep, older_than_days: days }
			);
			toasts.success(`Freed ${formatBytes(r.bytes)}`);
			await load();
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	const pct = (b: number) => (s && s.bytes ? Math.round((b / s.bytes) * 100) : 0);
</script>

<svelte:head><title>Storage · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<div class="flex items-center gap-2">
		<a class="btn btn-ghost px-2" href="/system" aria-label="System">‹</a>
		<h1 class="text-2xl font-semibold text-slate-900">Storage</h1>
	</div>
	{#if s}
		<div class="grid gap-3 sm:grid-cols-3">
			<div class="card p-4">
				<p class="text-xs text-slate-500">Uploaded files</p>
				<p class="text-2xl font-semibold text-slate-900" data-total>{formatBytes(s.bytes)}</p>
				<p class="text-xs text-slate-500">{s.files.toLocaleString()} files</p>
			</div>
			{#if s.disk}
				<div class="card p-4">
					<p class="text-xs text-slate-500">Free on the disk</p>
					<p
						class="text-2xl font-semibold {s.disk.free / s.disk.total < 0.1
							? 'text-red-700'
							: 'text-slate-900'}"
					>
						{formatBytes(s.disk.free)}
					</p>
					<p class="text-xs text-slate-500">of {formatBytes(s.disk.total)}</p>
				</div>
			{/if}
			<div class="card p-4">
				<p class="text-xs text-slate-500">Can be freed</p>
				<p class="text-2xl font-semibold text-slate-900">
					{formatBytes(s.cleanup.old_versions.bytes + s.cleanup.drive_trash.bytes)}
				</p>
				<p class="text-xs text-slate-500">old versions and trash</p>
			</div>
		</div>

		<div class="grid gap-4 lg:grid-cols-2">
			{#each [['Where', s.areas, AREA], ['What', s.kinds, KIND]] as [title, rows, names] (title)}
				<section class="card p-4 text-sm" aria-label={title as string}>
					<h2 class="mb-2 font-semibold text-slate-900">{title}</h2>
					{#each rows as Row[] as r (r.key)}
						<div class="py-1" data-area={r.key}>
							<div class="flex justify-between">
								<span>{(names as Record<string, string>)[r.key] ?? r.key}</span><span
									class="text-slate-500 tabular-nums"
									>{formatBytes(r.bytes)} · {r.files.toLocaleString()}</span
								>
							</div>
							<div class="mt-1 h-1.5 rounded bg-slate-100">
								<div class="h-1.5 rounded bg-brand-500" style="width: {pct(r.bytes)}%"></div>
							</div>
						</div>
					{/each}
				</section>
			{/each}
		</div>

		<section class="card space-y-2 p-4 text-sm" aria-label="Encryption">
			<h2 class="font-semibold text-slate-900">Encryption at rest</h2>
			{#if s.encryption.enabled}
				<p data-encryption="on">
					Files are stored encrypted (AES-256-GCM, key
					<code class="text-xs">{s.encryption.key_id}</code>).
					<span class="text-slate-500 tabular-nums"
						>{s.encryption.encrypted.toLocaleString()} encrypted</span
					>
					{#if s.encryption.not_encrypted + s.encryption.other_key > 0}
						<span class="text-amber-700" data-converting
							>· {(s.encryption.not_encrypted + s.encryption.other_key).toLocaleString()} still being
							encrypted{s.encryption.other_key ? ' (or moved to the new key)' : ''} in the background</span
						>
					{/if}
				</p>
			{:else}
				<p data-encryption="off">
					Files are not encrypted by NERP{s.encryption.other_key
						? ` (${s.encryption.other_key.toLocaleString()} still being decrypted)`
						: ''}. Set <code class="text-xs">FILE_ENCRYPTION_KEY</code> (at least 32 characters) in
					the server's settings and restart: new files are encrypted, existing ones in the background.
					Keep the key with the backups of <code class="text-xs">.env</code>: without it the files
					can't be read.
				</p>
			{/if}
			<p class="text-xs text-slate-500">
				The database is encrypted by the disk it is on (e.g. LUKS); secrets in it (mailbox and
				directory passwords, tokens) are encrypted by NERP. Removed files are kept aside for {s
					.encryption.keep_deleted_days} days for point-in-time restores ({formatBytes(
					s.encryption.deleted.bytes
				)}).
			</p>
		</section>

		<section class="card space-y-3 p-4 text-sm" aria-label="Clean up">
			<h2 class="font-semibold text-slate-900">Clean up</h2>
			<div class="flex flex-wrap items-center gap-2">
				<span class="flex-1"
					>Drive trash: {s.cleanup.drive_trash.count} files, {formatBytes(
						s.cleanup.drive_trash.bytes
					)} (emptied after 30 days anyway)</span
				>
				<button
					class="btn btn-secondary"
					disabled={busy || !s.cleanup.drive_trash.count}
					onclick={() => clean('drive_trash')}>Empty now</button
				>
			</div>
			<div class="flex flex-wrap items-center gap-2">
				<span class="flex-1"
					>Older versions of drive files: {s.cleanup.old_versions.count}, {formatBytes(
						s.cleanup.old_versions.bytes
					)}</span
				>
				<label class="flex items-center gap-1"
					>keep the last <input
						class="input w-16 py-1"
						type="number"
						min="0"
						max="100"
						bind:value={keep}
						aria-label="Versions to keep"
					/></label
				>
				<label class="flex items-center gap-1"
					>older than <input
						class="input w-20 py-1"
						type="number"
						min="0"
						max="3650"
						bind:value={days}
						aria-label="Older than days"
					/> days</label
				>
				<button
					class="btn btn-secondary"
					disabled={busy || !s.cleanup.old_versions.count}
					onclick={() => clean('old_versions')}>Delete</button
				>
			</div>
		</section>

		<div class="grid gap-4 lg:grid-cols-2">
			<section class="card p-4 text-sm" aria-label="Whose">
				<h2 class="mb-2 font-semibold text-slate-900">Whose</h2>
				{#each s.people as p (p.id ?? 'none')}<p class="flex justify-between py-0.5">
						<span>{p.name ?? 'NERP itself'}</span><span class="text-slate-500 tabular-nums"
							>{formatBytes(p.bytes)} · {p.files}</span
						>
					</p>{/each}
			</section>
			<section class="card p-4 text-sm" aria-label="Largest files">
				<h2 class="mb-2 font-semibold text-slate-900">Largest files</h2>
				{#each s.largest as f (f.id)}
					<p class="flex justify-between gap-2 py-0.5">
						<button
							class="truncate text-left text-brand-700 hover:underline"
							onclick={() => downloadFile(f.id, f.name).catch(toasts.error)}>{f.name}</button
						>
						<span class="shrink-0 text-slate-500 tabular-nums"
							>{formatBytes(f.bytes)} · {f.owner ?? '—'} · {formatDate(f.created_at)}</span
						>
					</p>
				{/each}
			</section>
		</div>
	{:else}
		<p class="text-sm text-slate-400">Loading…</p>
	{/if}
</div>
