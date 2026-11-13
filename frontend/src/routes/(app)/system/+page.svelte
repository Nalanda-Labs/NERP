<script lang="ts">
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { Job, Page } from '#lib/types.ts';

	type System = {
		jobs: Record<string, number>;
		instance: string;
		worker_enabled: boolean;
		live_connections_on_this_instance: number;
		smtp: { host: string; port: number; security: string; from: string };
		public_url: string;
		doc_engines: { typst: string | null; latex: string | null; office: string | null };
		redis?: { aof: boolean; snapshots: boolean | null; snapshot_schedule: string | null; last_save: string | null; last_save_ok: boolean | null; persistent: boolean };
		mail_oauth: { google: boolean; microsoft: boolean; redirect_uri: string };
	};

	const PAGE_SIZE = 25;
	let info = $state<System | null>(null);
	let jobs = $state<Page<Job> | null>(null);
	let status = $state('');
	let offset = $state(0);
	let testTo = $state(session.user?.email ?? '');
	let sending = $state(false);

	async function load() {
		try {
			[info, jobs] = await Promise.all([
				api.get<System>('/admin/system'),
				api.get<Page<Job>>('/admin/jobs' + query({ status, limit: PAGE_SIZE, offset }))
			]);
		} catch (e) {
			toasts.error(e);
		}
	}

	onMount(() => {
		load();
		const t = setInterval(load, 5000);
		return () => clearInterval(t);
	});

	async function retry(j: Job) {
		try {
			await api.post(`/admin/jobs/${j.id}/retry`);
			toasts.success('Job queued again');
			load();
		} catch (e) {
			toasts.error(e);
		}
	}

	async function sendTest(e: SubmitEvent) {
		e.preventDefault();
		sending = true;
		try {
			const r = await api.post<{ to: string }>('/admin/email/test', { to: testTo });
			toasts.success(`Test email queued for ${r.to}`);
			setTimeout(load, 1500);
		} catch (err) {
			toasts.error(err);
		} finally {
			sending = false;
		}
	}

	const tones: Record<string, string> = {
		pending: 'bg-sky-50 text-sky-700',
		running: 'bg-violet-50 text-violet-700',
		done: 'bg-emerald-50 text-emerald-700',
		failed: 'bg-red-50 text-red-700'
	};

	function describe(j: Job): string {
		if (j.kind === 'email.send') return `To ${j.payload.to}: ${j.payload.subject}`;
		return '';
	}
</script>

<svelte:head><title>System · NERP</title></svelte:head>

<div class="space-y-6">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">System</h1>
		<p class="text-sm text-slate-500">Background jobs, outgoing email and live connections. <a class="text-brand-600 hover:underline" href="/system/storage">Storage →</a> <a class="ml-2 text-brand-600 hover:underline" href="/system/backups">Backups →</a> <a class="ml-2 text-brand-600 hover:underline" href="/system/health">Health →</a> <a class="ml-2 text-brand-600 hover:underline" href="/system/sandbox">Sandbox →</a> <a class="ml-2 text-brand-600 hover:underline" href="/system/sms">Text messages →</a></p>
	</div>

	<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
		{#each ['pending', 'running', 'done', 'failed'] as s (s)}
			<button class="card p-5 text-left hover:border-brand-200 {status === s ? 'ring-2 ring-brand-500' : ''}" onclick={() => ((status = status === s ? '' : s), (offset = 0), load())}>
				<span class="block text-2xl font-semibold text-slate-900">{info?.jobs[s] ?? 0}</span>
				<span class="text-sm text-slate-500 capitalize">{s} jobs</span>
			</button>
		{/each}
	</div>

	<div class="grid gap-6 lg:grid-cols-2">
		<section class="card">
			<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">Email</h2>
			<div class="space-y-4 p-5 text-sm">
				{#if info}
					<dl class="grid grid-cols-[8rem_1fr] gap-y-1.5">
						<dt class="text-slate-500">SMTP server</dt><dd class="font-mono text-slate-800">{info.smtp.host}:{info.smtp.port} ({info.smtp.security})</dd>
						<dt class="text-slate-500">Sender</dt><dd class="text-slate-800">{info.smtp.from}</dd>
						<dt class="text-slate-500">Links point to</dt><dd class="font-mono text-slate-800">{info.public_url}</dd>
					</dl>
				{/if}
				<form class="flex gap-2" onsubmit={sendTest}>
					<input class="input" type="email" required bind:value={testTo} placeholder="Recipient" />
					<button class="btn btn-primary shrink-0" disabled={sending}><Icon name="send" class="size-4" /> Send test</button>
				</form>
			</div>
		</section>
		<section class="card">
			<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">This server</h2>
			{#if info}
				<dl class="grid grid-cols-[10rem_1fr] gap-y-1.5 p-5 text-sm">
					<dt class="text-slate-500">Instance</dt><dd class="font-mono text-slate-800">{info.instance}</dd>
					<dt class="text-slate-500">Job worker</dt><dd class="text-slate-800">{info.worker_enabled ? 'Running' : 'Disabled on this instance'}</dd>
					<dt class="text-slate-500">Live connections</dt><dd class="text-slate-800">{info.live_connections_on_this_instance}</dd>
					<dt class="text-slate-500">Your live updates</dt><dd class="text-slate-800">{realtime.connected ? 'Connected' : 'Reconnecting…'}</dd>
					<dt class="text-slate-500">Mailbox sign-in</dt>
					<dd class="space-y-0.5 text-slate-800">
						<p>Google: {#if info.mail_oauth?.google}configured{:else}<span class="text-amber-700">not configured</span>{/if} · Microsoft: {#if info.mail_oauth?.microsoft}configured{:else}<span class="text-amber-700">not configured</span>{/if}</p>
						<p class="text-xs text-slate-500">Redirect URI: <code>{info.mail_oauth?.redirect_uri}</code></p>
					</dd>
					<dt class="text-slate-500">Redis</dt>
					<dd class="space-y-0.5 text-slate-800" data-redis={info.redis?.persistent ? 'persistent' : 'volatile'}>
						{#if info.redis?.persistent}
							<p>Keeps its data across restarts ({[info.redis.aof ? 'append-only file' : '', info.redis.snapshots ? 'snapshots' : ''].filter(Boolean).join(' and ')}){#if info.redis.last_save_ok === false}<span class="text-red-700"> · the last snapshot failed</span>{/if}</p>
						{:else}
							<p class="text-amber-700">Keeps nothing across a restart{info.redis?.snapshots === null ? ' (or it doesn’t say)' : ''}</p>
							<p class="text-xs text-slate-500">Sign-ins are in the database either way; a Redis restart resets rate limits and lockouts. Turn on <code>appendonly yes</code> (see SETUP.md).</p>
						{/if}
					</dd>
					<dt class="text-slate-500">Document engines</dt>
					<dd class="space-y-0.5 text-slate-800">
						{#each [['Typst', info.doc_engines?.typst], ['LuaLaTeX', info.doc_engines?.latex], ['LibreOffice', info.doc_engines?.office]] as [n, v] (n)}
							<p>{n}: {#if v}<span class="text-slate-500">{v}</span>{:else}<span class="text-amber-700">not installed</span>{/if}</p>
						{/each}
					</dd>
				</dl>
			{/if}
		</section>
	</div>

	<section class="card overflow-hidden">
		<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3.5">
			<h2 class="font-semibold text-slate-900">Jobs {status ? `· ${status}` : ''}</h2>
			<button class="btn btn-ghost p-1.5" aria-label="Refresh" onclick={load}><Icon name="refresh" class="size-4" /></button>
		</div>
		<table class="min-w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
				<tr>
					<th class="px-5 py-3">Job</th>
					<th class="px-5 py-3">Status</th>
					<th class="hidden px-5 py-3 md:table-cell">Attempts</th>
					<th class="hidden px-5 py-3 md:table-cell">Created</th>
					<th class="px-5 py-3"></th>
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-100">
				{#each jobs?.items ?? [] as j (j.id)}
					<tr class="align-top">
						<td class="px-5 py-3">
							<div class="font-mono text-xs text-slate-800">{j.kind}</div>
							{#if describe(j)}<div class="max-w-md truncate text-slate-500">{describe(j)}</div>{/if}
							{#if j.last_error}<div class="mt-1 max-w-md text-xs break-words text-red-600">{j.last_error}</div>{/if}
						</td>
						<td class="px-5 py-3">
							<span class="badge {tones[j.status]}">{j.status}</span>
							{#if j.status === 'pending' && new Date(j.run_at) > new Date()}
								<div class="mt-1 text-xs text-slate-400">retry {formatRelative(j.run_at).replace(' ago', '')}</div>
							{/if}
						</td>
						<td class="hidden px-5 py-3 text-slate-600 md:table-cell">{j.attempts} / {j.max_attempts}</td>
						<td class="hidden px-5 py-3 whitespace-nowrap text-slate-500 md:table-cell">{formatDateTime(j.created_at)}</td>
						<td class="px-5 py-3 text-right">
							{#if j.status === 'failed'}<button class="btn btn-secondary" onclick={() => retry(j)}>Retry</button>{/if}
						</td>
					</tr>
				{:else}
					<tr><td colspan="5" class="px-5 py-10 text-center text-slate-500">{jobs ? 'No jobs.' : 'Loading…'}</td></tr>
				{/each}
			</tbody>
		</table>
		{#if jobs && jobs.total > PAGE_SIZE}
			<div class="flex items-center justify-between border-t border-slate-200 px-5 py-3 text-sm text-slate-500">
				<span>{offset + 1}–{Math.min(offset + PAGE_SIZE, jobs.total)} of {jobs.total}</span>
				<div class="flex gap-2">
					<button class="btn btn-secondary" disabled={offset === 0} onclick={() => ((offset -= PAGE_SIZE), load())}>Newer</button>
					<button class="btn btn-secondary" disabled={offset + PAGE_SIZE >= jobs.total} onclick={() => ((offset += PAGE_SIZE), load())}>Older</button>
				</div>
			</div>
		{/if}
	</section>
</div>
