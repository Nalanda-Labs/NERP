<script lang="ts">
	// Backups: the dated backups on the server, backing up now, proving the newest restores,
	// downloading a database dump, the command that restores one, and point-in-time restores
	// (the WAL archive's state, and putting the database back as it was at a moment).
	import { onDestroy, onMount } from 'svelte';
	import { api, downloadUrl } from '#lib/api.ts';
	import { formatBytes, formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import CompanyExport from '#lib/components/CompanyExport.svelte';

	type Run = { started_at: string; finished_at: string; ok: boolean; output: string } | null;
	type Backup = {
		name: string;
		has_dump: boolean;
		manifest: {
			database_bytes?: number;
			upload_files?: number;
			last_migration?: number;
			host?: string;
		};
	};
	type Info = {
		dir: string;
		dir_exists: boolean;
		scripts: boolean;
		running: string | null;
		items: Backup[];
		last_backup: Run;
		last_verify: Run;
		restore_command: string | null;
		restore_from_app: boolean;
		last_restore: (Run & { backup?: string }) | null;
		pitr: {
			slot: string;
			set_up: boolean;
			receiving: boolean;
			last_reply_at: string | null;
			retained_bytes: number | null;
			earliest: string | null;
			command: string;
		};
	};
	let info = $state<Info | null>(null);
	let showOutput = $state<'' | 'backup' | 'verify'>('');
	let timer: ReturnType<typeof setInterval> | undefined;

	async function load() {
		try {
			info = await api.get<Info>('/admin/backups');
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(() => {
		load();
		timer = setInterval(() => info?.running && load(), 3000);
	});
	onDestroy(() => clearInterval(timer));
	async function start(path: string) {
		try {
			await api.post(path);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	// 2026-11-10T020000Z → a date
	const when = (n: string) =>
		formatDateTime(`${n.slice(0, 13)}:${n.slice(13, 15)}:${n.slice(15, 17)}Z`);
	// Restoring from here (when the server allows it): the password and the backup's name.
	let restoreOf = $state<Backup | null>(null);
	let restorePw = $state('');
	let restoreName = $state('');
	let restoring = $state(false);
	// Restoring to a moment: the moment (local time), the password and "restore".
	let momentOpen = $state(false);
	let moment = $state('');
	async function restore(e: SubmitEvent) {
		e.preventDefault();
		try {
			if (momentOpen) {
				await api.post('/admin/backups/restore-to-moment', {
					to: new Date(moment).toISOString(),
					password: restorePw,
					confirm: restoreName
				});
			} else {
				await api.post(`/admin/backups/${restoreOf!.name}/restore`, { password: restorePw, confirm: restoreName });
			}
			restoreOf = null;
			momentOpen = false;
			restorePw = '';
			restoring = true;
			// The app pauses while it restores; the health check says when it is back.
			await new Promise((r) => setTimeout(r, 3000));
			for (let i = 0; i < 600; i++) {
				const h = await fetch('/api/health').then((x) => x.json()).catch(() => null);
				if (h?.status === 'ok') break;
				await new Promise((r) => setTimeout(r, 2000));
			}
			restoring = false;
			await load();
			toasts[info?.last_restore?.ok ? 'success' : 'error'](info?.last_restore?.ok ? 'Restored' : 'Restoring did not work; see the output');
		} catch (err) {
			toasts.error(err);
		}
	}
	const copy = (t: string) =>
		navigator.clipboard?.writeText(t).then(() => toasts.success('Copied'));
</script>

<svelte:head><title>Backups · NERP</title></svelte:head>

<div class="max-w-4xl space-y-5">
	<div class="flex flex-wrap items-center gap-2">
		<a class="btn btn-ghost px-2" href="/system" aria-label="System">‹</a>
		<h1 class="text-2xl font-semibold text-slate-900">Backups</h1>
		{#if info}
			<div class="ml-auto flex gap-2">
				<button
					class="btn btn-primary"
					disabled={!!info.running || !info.scripts}
					onclick={() => start('/admin/backups')}
					>{info.running === 'backup' ? 'Backing up…' : 'Back up now'}</button
				>
				<button
					class="btn btn-secondary"
					disabled={!!info.running || !info.scripts || !info.items.length}
					onclick={() => start('/admin/backups/verify')}
					>{info.running === 'backup_verify' ? 'Checking…' : 'Check the newest restores'}</button
				>
			</div>
		{/if}
	</div>
	{#if info}
		<p class="text-sm text-slate-500">
			Each backup holds the database and the uploaded files, in <code>{info.dir}</code>. The server
			also backs up every night when its timer is set up (see the deploy guide). Keep the server's
			<code>.env</code> file safe separately: without its keys, stored passwords can't be read.
		</p>
		{#if !info.scripts}<p class="rounded-lg bg-amber-50 p-3 text-sm text-amber-900">
				The backup scripts were not found; set BACKUP_SCRIPTS to the deploy/backup folder.
			</p>{/if}
		<div class="grid gap-3 sm:grid-cols-2">
			{#each [['backup', 'Last backup', info.last_backup], ['verify', 'Last restore check', info.last_verify]] as [k, label, r] (k)}
				{@const run = r as Run}
				<section class="card p-4 text-sm" data-run={k}>
					<p class="text-xs text-slate-500">{label}</p>
					{#if run}
						<p
							class="font-medium {run.ok ? 'text-emerald-700' : 'text-red-700'}"
							data-run-ok={run.ok}
						>
							{run.ok ? 'Succeeded' : 'Failed'} · {formatDateTime(run.finished_at)}
						</p>
						<button
							class="text-xs text-brand-600 hover:underline"
							onclick={() => (showOutput = showOutput === k ? '' : (k as 'backup' | 'verify'))}
							>{showOutput === k ? 'Hide' : 'Show'} the output</button
						>
						{#if showOutput === k}<pre
								class="mt-2 max-h-64 overflow-auto rounded bg-slate-900 p-2 text-xs text-slate-100">{run.output}</pre>{/if}
					{:else}<p class="text-slate-400">Never from here.</p>{/if}
				</section>
			{/each}
		</div>
		<section class="card" aria-label="Backups">
			<table class="w-full text-sm">
				<thead
					><tr class="border-b border-slate-100 text-left text-xs text-slate-500"
						><th class="px-4 py-2">Made</th><th>Database</th><th>Files</th><th>Migration</th><th
						></th></tr
					></thead
				>
				<tbody class="divide-y divide-slate-50">
					{#each info.items as b (b.name)}
						<tr data-backup={b.name}>
							<td class="px-4 py-2"
								>{when(b.name)}{#if b.manifest.host}<span class="text-xs text-slate-400">
										· {b.manifest.host}</span
									>{/if}</td
							>
							<td>{b.manifest.database_bytes ? formatBytes(b.manifest.database_bytes) : '—'}</td>
							<td>{b.manifest.upload_files ?? '—'}</td>
							<td>{b.manifest.last_migration ?? '—'}</td>
							<td class="pr-4 text-right"
								>{#if b.has_dump && info.restore_from_app}<button class="mr-3 text-red-600 hover:underline" onclick={() => ((restoreOf = b), (restoreName = ''))} data-restore-backup={b.name}>Restore</button>{/if}{#if b.has_dump}<button
										class="text-brand-600 hover:underline"
										onclick={() =>
											downloadUrl(
												`/api/admin/backups/${b.name}/database.dump`,
												`nerp-${b.name}.dump`
											).catch(toasts.error)}>Database</button
									>{/if}</td
							>
						</tr>
					{:else}<tr
							><td class="px-4 py-3 text-slate-400" colspan="5"
								>{info.dir_exists
									? 'No backups yet.'
									: 'The folder does not exist yet; the first backup makes it.'}</td
							></tr
						>{/each}
				</tbody>
			</table>
		</section>
		{#if info.restore_command}
			<section class="card space-y-2 p-4 text-sm" aria-label="Restoring">
				<h2 class="font-semibold text-slate-900">Restoring</h2>
				<p class="text-slate-600">
					Restoring replaces everything with the backup, so it runs on the server, in the NERP folder, with NERP stopped.
					For the newest backup:
				</p>
				<div class="flex items-start gap-2">
					<code class="flex-1 rounded bg-slate-50 p-2 text-xs break-all" data-restore
						>{info.restore_command}</code
					><button class="btn btn-ghost" onclick={() => copy(info!.restore_command!)}>Copy</button>
				</div>
				<p class="text-xs text-slate-500">
					To look something up without touching the live data, add <code>--target-db</code> with an
					empty database instead of <code>--yes</code>.
				</p>
			</section>
		{/if}
		<section class="card space-y-2 p-4 text-sm" aria-label="Point-in-time restore" data-pitr>
			<h2 class="font-semibold text-slate-900">Point-in-time restore</h2>
			{#if info.pitr.receiving}
				<p data-receiving="on">
					The database's changes are received as they happen{info.pitr.last_reply_at
						? ` (last at ${formatDateTime(info.pitr.last_reply_at)})`
						: ''}.
					{#if info.pitr.earliest}
						It can be put back as it was at any moment since <strong
							>{formatDateTime(info.pitr.earliest)}</strong
						>.
					{:else}
						The next backup makes the first base backup to restore from.
					{/if}
				</p>
			{:else if info.pitr.set_up}
				<p class="text-red-700" data-receiving="stopped">
					Receiving the database's changes has stopped: start <code>nerp-wal-receive</code> again.
					Meanwhile the database server keeps them for it ({formatBytes(info.pitr.retained_bytes ?? 0)}
					so far), which fills its disk if left.
				</p>
			{:else}
				<p data-receiving="off" class="text-slate-600">
					Off: only the backups themselves can be restored. To restore any moment in between, run
					<code>deploy/backup/nerp-wal-receive.sh</code> where the backups are kept (see the deploy
					guide); the next backups then include a base backup.
				</p>
			{/if}
			<div class="flex items-start gap-2">
				<code class="flex-1 rounded bg-slate-50 p-2 text-xs break-all" data-pitr-command
					>{info.pitr.command}</code
				><button class="btn btn-ghost" onclick={() => copy(info!.pitr.command)}>Copy</button>
			</div>
			{#if info.restore_from_app && info.pitr.earliest}
				<button
					class="btn btn-secondary text-red-700"
					data-restore-moment
					onclick={() => ((momentOpen = true), (restoreName = ''), (moment = ''))}
					>Restore to a moment…</button
				>
			{/if}
		</section>
		{#if info.last_restore}
			<section class="card space-y-1 p-4 text-sm" aria-label="Last restore" data-last-restore>
				<h2 class="font-semibold text-slate-900">Last restore from here</h2>
				<p class={info.last_restore.ok ? 'text-emerald-700' : 'text-red-700'}>{info.last_restore.ok ? 'Restored' : 'Failed'}: {info.last_restore.backup ? (/^20\d\d-\d\d-\d\dT\d{6}Z$/.test(info.last_restore.backup) ? when(info.last_restore.backup) : `as at ${info.last_restore.backup}`) : ''} · {formatDateTime(info.last_restore.finished_at)}</p>
				<details><summary class="cursor-pointer text-slate-500">Output</summary><pre class="mt-1 max-h-64 overflow-auto rounded bg-slate-50 p-2 text-xs whitespace-pre-wrap">{info.last_restore.output}</pre></details>
			</section>
		{/if}
	{:else}
		<p class="text-sm text-slate-400">Loading…</p>
	{/if}
	<CompanyExport />
</div>

{#if restoring}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/50 p-4" role="alertdialog" aria-label="Restoring">
		<div class="max-w-sm rounded-2xl bg-white p-6 text-center shadow-g2"><p class="font-semibold text-slate-900">Restoring the backup…</p><p class="mt-1 text-sm text-slate-500">NERP is paused for everyone until it is done. This page continues by itself.</p></div>
	</div>
{/if}

{#if restoreOf}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-4" role="dialog" aria-modal="true" aria-label="Restore a backup">
		<form class="w-full max-w-md space-y-3 rounded-2xl bg-white p-5 text-sm shadow-g2" onsubmit={restore}>
			<h2 class="text-lg font-semibold text-slate-900">Restore the backup of {when(restoreOf.name)}?</h2>
			<p class="text-red-700">Everything in NERP goes back to that moment: whatever was added or changed since is lost, for everyone. NERP pauses while it restores.</p>
			<div><label class="label" for="rs-name">Type <code>{restoreOf.name}</code> to confirm</label><input id="rs-name" class="input font-mono" required bind:value={restoreName} autocomplete="off" /></div>
			<div><label class="label" for="rs-pw">Your password</label><input id="rs-pw" class="input" type="password" required bind:value={restorePw} autocomplete="current-password" /></div>
			<div class="flex justify-end gap-2"><button type="button" class="btn btn-secondary" onclick={() => (restoreOf = null)}>Cancel</button><button class="btn bg-red-600 text-white hover:bg-red-700" disabled={restoreName !== restoreOf.name}>Restore</button></div>
		</form>
	</div>
{/if}

{#if momentOpen && info}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 p-4" role="dialog" aria-modal="true" aria-label="Restore to a moment">
		<form class="w-full max-w-md space-y-3 rounded-2xl bg-white p-5 text-sm shadow-g2" onsubmit={restore}>
			<h2 class="text-lg font-semibold text-slate-900">Put the database back as it was at…</h2>
			<div>
				<label class="label" for="pm-at">The moment (your time)</label>
				<input id="pm-at" class="input" type="datetime-local" step="1" required bind:value={moment} />
				{#if info.pitr.earliest}<p class="mt-1 text-xs text-slate-500">Not before {formatDateTime(info.pitr.earliest)}.</p>{/if}
			</div>
			<p class="text-red-700">Everything changed after that moment is lost, for everyone; files removed since then come back. NERP pauses while it restores.</p>
			<div><label class="label" for="pm-confirm">Type <code>restore</code> to confirm</label><input id="pm-confirm" class="input font-mono" required bind:value={restoreName} autocomplete="off" /></div>
			<div><label class="label" for="pm-pw">Your password</label><input id="pm-pw" class="input" type="password" required bind:value={restorePw} autocomplete="current-password" /></div>
			<div class="flex justify-end gap-2"><button type="button" class="btn btn-secondary" onclick={() => (momentOpen = false)}>Cancel</button><button class="btn bg-red-600 text-white hover:bg-red-700" disabled={restoreName !== 'restore' || !moment}>Restore</button></div>
		</form>
	</div>
{/if}
