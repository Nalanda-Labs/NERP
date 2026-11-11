<script lang="ts">
	// Administration → Mailboxes: every employee's connected mailbox with its state; pausing,
	// syncing and disconnecting them; and connecting mailboxes for many employees at once.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Row = {
		id: string; owner: { id: string; name: string; email: string }; email: string; auth_kind: string; sync_enabled: boolean; last_sync_at: string | null;
		last_error: string | null; status: 'ok' | 'error' | 'paused' | 'waiting'; shared: { people: number; roles: number; departments: number }; emails: number;
	};
	type Result = { email: string; ok: boolean; error?: string };
	let rows = $state<Row[]>([]);
	let without = $state<{ id: string; name: string; email: string }[]>([]);
	let q = $state('');
	let only = $state<'all' | 'error' | 'paused'>('all');
	const STATUS: Record<Row['status'], [string, string]> = { ok: ['Syncing', 'bg-emerald-50 text-emerald-800'], error: ['Error', 'bg-red-50 text-red-800'], paused: ['Paused', 'bg-slate-100 text-slate-600'], waiting: ['Waiting for its first sync', 'bg-amber-50 text-amber-800'] };

	async function load() {
		const r = await api.get<{ items: Row[]; without_mailbox: typeof without }>('/admin/mailboxes');
		rows = r.items;
		without = r.without_mailbox;
	}
	onMount(load);
	const shown = $derived(rows.filter((r) => (only === 'all' || r.status === only) && (!q.trim() || `${r.owner.name} ${r.email}`.toLowerCase().includes(q.trim().toLowerCase()))));

	async function act(r: Row, what: 'pause' | 'resume' | 'sync' | 'remove') {
		try {
			if (what === 'remove') {
				if (!confirm(`Disconnect ${r.email} (${r.owner.name})? Emails already filed stay on their records.`)) return;
				await api.del(`/admin/mailboxes/${r.id}`);
			} else if (what === 'sync') {
				await api.post(`/admin/mailboxes/${r.id}/sync`);
				toasts.success('Syncing');
			} else await api.patch(`/admin/mailboxes/${r.id}`, { sync_enabled: what === 'resume' });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	// Connecting for many employees.
	let bulkOpen = $state(false);
	let server = $state({ imap_host: '', imap_port: 993, imap_security: 'tls', smtp_host: '', smtp_port: 587, smtp_security: 'starttls', allow_invalid_certs: false });
	let lines = $state('');
	let syncDays = $state(7);
	let createLeads = $state(false);
	let busy = $state(false);
	let results = $state<{ connected: number; failed: number; results: Result[] } | null>(null);
	function fillWithout() {
		lines = without.map((u) => `${u.email}, , `).join('\n');
	}
	async function connect(e: SubmitEvent) {
		e.preventDefault();
		const accounts = lines
			.split('\n')
			.map((l) => l.split(/[,;\t]/).map((x) => x.trim()))
			.filter((p) => p[0])
			.map(([user, email, password, username]) => ({ user_email: user, email: email || undefined, password: password ?? '', username: username || undefined }));
		if (!accounts.length) return;
		busy = true;
		try {
			results = await api.post('/admin/mailboxes/bulk', { server: { ...server, imap_port: Number(server.imap_port), smtp_port: Number(server.smtp_port) }, accounts, sync_days: Number(syncDays), create_leads: createLeads });
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Mailboxes · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Mailboxes</h1>
		<input class="input ml-auto w-56" type="search" placeholder="Person or address…" aria-label="Find a mailbox" bind:value={q} />
		<select class="input w-40" aria-label="Show" bind:value={only}><option value="all">All</option><option value="error">With errors</option><option value="paused">Paused</option></select>
		<button class="btn btn-primary" onclick={() => ((bulkOpen = !bulkOpen), (results = null))} data-bulk-connect><Icon name="plus" class="size-4" /> Connect for employees</button>
	</div>

	{#if bulkOpen}
		<form class="card space-y-4 p-5 text-sm" onsubmit={connect}>
			<h2 class="font-medium text-slate-900">Connect mailboxes for employees</h2>
			<div class="grid gap-3 sm:grid-cols-3">
				<div><label class="label" for="bk-ih">Incoming server (IMAP)</label><input id="bk-ih" class="input" required placeholder="imap.example.com" bind:value={server.imap_host} /></div>
				<div><label class="label" for="bk-ip">Port</label><input id="bk-ip" class="input" type="number" bind:value={server.imap_port} /></div>
				<div><label class="label" for="bk-is">Security</label><select id="bk-is" class="input" bind:value={server.imap_security}><option value="tls">TLS</option><option value="starttls">STARTTLS</option><option value="none">None</option></select></div>
				<div><label class="label" for="bk-sh">Outgoing server (SMTP)</label><input id="bk-sh" class="input" required placeholder="smtp.example.com" bind:value={server.smtp_host} /></div>
				<div><label class="label" for="bk-sp">Port</label><input id="bk-sp" class="input" type="number" bind:value={server.smtp_port} /></div>
				<div><label class="label" for="bk-ss">Security</label><select id="bk-ss" class="input" bind:value={server.smtp_security}><option value="starttls">STARTTLS</option><option value="tls">TLS</option><option value="none">None</option></select></div>
			</div>
			<div>
				<div class="flex items-center gap-2"><label class="label mr-auto" for="bk-lines">One per line: employee's sign-in email, mailbox address (empty: the same), password, user name (empty: the address)</label>{#if without.length}<button type="button" class="btn btn-ghost px-2 py-1 text-xs" onclick={fillWithout}>Everyone without a mailbox ({without.length})</button>{/if}</div>
				<textarea id="bk-lines" class="input font-mono text-xs" rows="6" required placeholder={'ann@company.com, , her-password\nbob@company.com, sales@company.com, his-password'} bind:value={lines}></textarea>
			</div>
			<div class="flex flex-wrap items-center gap-4">
				<label class="flex items-center gap-2">Import <input class="input w-20 py-1" type="number" min="0" max="365" bind:value={syncDays} /> days of mail</label>
				<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={createLeads} /> Leads from unknown senders</label>
				<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={server.allow_invalid_certs} /> Self-signed certificates</label>
				<button class="btn btn-primary ml-auto" disabled={busy}>{busy ? 'Connecting…' : 'Connect'}</button>
			</div>
			{#if results}
				<div class="rounded-xl bg-slate-50 p-3" data-bulk-results>
					<p class="mb-2 font-medium text-slate-800">{results.connected} connected, {results.failed} not</p>
					<ul class="space-y-0.5 text-xs">{#each results.results as r, i (i)}<li class={r.ok ? 'text-emerald-800' : 'text-red-700'}>{r.ok ? '✓' : '✗'} {r.email}{r.error ? ` — ${r.error}` : ''}</li>{/each}</ul>
				</div>
			{/if}
			<p class="text-xs text-slate-500">Each mailbox is checked (signing in to both servers) before it is kept; its employee is told and can change its settings or disconnect it. Passwords are stored encrypted.</p>
		</form>
	{/if}

	<section class="card overflow-x-auto">
		<table class="min-w-full text-sm">
			<thead class="text-left text-xs text-slate-500"><tr><th class="px-4 py-2 font-medium">Employee</th><th class="px-3 py-2 font-medium">Mailbox</th><th class="px-3 py-2 font-medium">State</th><th class="px-3 py-2 font-medium">Shared with</th><th class="px-3 py-2 text-right font-medium">Emails</th><th class="px-4 py-2"></th></tr></thead>
			<tbody class="divide-y divide-slate-50">
				{#each shown as r (r.id)}
					<tr data-mailbox={r.email}>
						<td class="px-4 py-2"><span class="text-slate-900">{r.owner.name}</span><span class="block text-xs text-slate-500">{r.owner.email}</span></td>
						<td class="px-3 py-2">{r.email}{#if r.auth_kind !== 'password'}<span class="ml-1 text-xs text-slate-400">{r.auth_kind}</span>{/if}</td>
						<td class="px-3 py-2"><span class="badge {STATUS[r.status][1]}">{STATUS[r.status][0]}</span>{#if r.last_error}<span class="block max-w-64 truncate text-xs text-red-700" title={r.last_error}>{r.last_error}</span>{:else if r.last_sync_at}<span class="block text-xs text-slate-500">{formatRelative(r.last_sync_at)}</span>{/if}</td>
						<td class="px-3 py-2 text-xs text-slate-600">{[r.shared.people && `${r.shared.people} people`, r.shared.roles && `${r.shared.roles} roles`, r.shared.departments && `${r.shared.departments} departments`].filter(Boolean).join(', ') || '—'}</td>
						<td class="px-3 py-2 text-right tabular-nums">{r.emails}</td>
						<td class="px-4 py-2 text-right whitespace-nowrap">
							<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => act(r, 'sync')}>Sync</button>
							<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => act(r, r.sync_enabled ? 'pause' : 'resume')}>{r.sync_enabled ? 'Pause' : 'Resume'}</button>
							<button class="btn btn-ghost px-2 py-1 text-xs text-red-700" onclick={() => act(r, 'remove')}>Disconnect</button>
						</td>
					</tr>
				{:else}
					<tr><td colspan="6" class="px-4 py-8 text-center text-slate-500">No mailboxes{q || only !== 'all' ? ' match' : ' are connected yet'}.</td></tr>
				{/each}
			</tbody>
		</table>
	</section>
	{#if without.length}<p class="text-xs text-slate-500">{without.length} employee{without.length === 1 ? ' has' : 's have'} no mailbox connected.</p>{/if}
</div>
