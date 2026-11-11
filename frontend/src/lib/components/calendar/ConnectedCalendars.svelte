<script lang="ts">
	// Calendars elsewhere (Google, Microsoft 365, iCloud and other CalDAV accounts) synced both
	// ways with my NERP calendar.
	import { api } from '#lib/api.ts';
	import { formatRelative } from '#lib/format.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type Account = { id: string; kind: 'caldav' | 'google' | 'microsoft'; name: string; calendar_name: string | null; calendar_url: string | null; push: boolean; last_sync_at: string | null; last_error: string | null; events: number; from_there: number };
	let { open = $bindable(false), onchanged }: { open?: boolean; onchanged?: () => void } = $props();

	const KIND: Record<string, string> = { caldav: 'CalDAV', google: 'Google', microsoft: 'Microsoft 365' };
	let accounts = $state<Account[] | null>(null);
	let providers = $state<{ google: boolean; microsoft: boolean }>({ google: false, microsoft: false });
	let adding = $state(false);
	let form = $state({ url: '', username: '', password: '', push: true });
	let found = $state<{ url: string; name: string }[] | null>(null);
	let busy = $state('');

	async function load() {
		try {
			accounts = (await api.get<{ items: Account[] }>('/calendar/accounts')).items;
			providers = await api.get('/calendar/accounts/providers');
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		if (open) load();
	});
	async function oauth(kind: string) {
		busy = kind;
		try {
			location.href = (await api.post<{ url: string }>(`/calendar/accounts/oauth/${kind}`)).url;
		} catch (e) {
			toasts.error(e);
			busy = '';
		}
	}
	async function find(e: SubmitEvent) {
		e.preventDefault();
		busy = 'find';
		try {
			found = (await api.post<{ items: { url: string; name: string }[] }>('/calendar/accounts/caldav/discover', form)).items;
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = '';
		}
	}
	async function connect(url: string) {
		busy = url;
		try {
			const a = await api.post<Account>('/calendar/accounts/caldav', { ...form, url });
			toasts.success(a.last_error ? `Connected, but the sync failed: ${a.last_error}` : 'Connected and synced');
			adding = false;
			found = null;
			form = { url: '', username: '', password: '', push: true };
			await load();
			onchanged?.();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = '';
		}
	}
	async function syncNow(a: Account) {
		busy = a.id;
		try {
			const r = await api.post<Account>(`/calendar/accounts/${a.id}/sync`);
			r.last_error ? toasts.error(r.last_error) : toasts.success('Synced');
			await load();
			onchanged?.();
		} finally {
			busy = '';
		}
	}
	async function setPush(a: Account, push: boolean) {
		await api.patch(`/calendar/accounts/${a.id}`, { push });
		await load();
	}
	async function disconnect(a: Account) {
		if (!confirm(`Stop syncing with ${a.name}? Events that came from there are removed from NERP (nothing is deleted there).`)) return;
		await api.del(`/calendar/accounts/${a.id}`);
		await load();
		onchanged?.();
	}
</script>

<Modal bind:open title="Connected calendars">
	<div class="space-y-4 text-sm">
		<p class="text-slate-600">Sync your NERP calendar both ways with a calendar elsewhere: events there show here, your NERP events go there, and changes go both ways every few minutes.</p>
		{#each accounts ?? [] as a (a.id)}
			<div class="space-y-2 rounded-xl border border-slate-200 p-3" data-account={a.name}>
				<div class="flex items-start gap-2">
					<div class="min-w-0 flex-1">
						<p class="font-medium text-slate-900">{KIND[a.kind]} · {a.name}</p>
						<p class="text-xs text-slate-500">{a.calendar_name ?? ''} · {a.events} events synced · {a.last_sync_at ? `last sync ${formatRelative(a.last_sync_at)}` : 'not synced yet'}</p>
						{#if a.last_error}<p class="mt-1 text-xs text-red-600" data-error>{a.last_error}</p>{/if}
					</div>
					<button class="btn btn-ghost px-2 py-1" disabled={busy === a.id} onclick={() => syncNow(a)}>{busy === a.id ? 'Syncing…' : 'Sync now'}</button>
					<button class="btn btn-ghost px-2 py-1 hover:text-red-600" onclick={() => disconnect(a)}>Disconnect</button>
				</div>
				<label class="flex items-center gap-2 text-slate-600"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={a.push} onchange={(e) => setPush(a, e.currentTarget.checked)} /> Also send my NERP events there</label>
			</div>
		{/each}
		{#if adding}
			<form class="space-y-2 rounded-xl bg-slate-50 p-3" onsubmit={find} aria-label="CalDAV account">
				<p class="font-medium text-slate-800">CalDAV account</p>
				<p class="text-xs text-slate-500">iCloud: server <code>https://caldav.icloud.com</code>, your Apple ID and an app-specific password. Nextcloud, Fastmail, Yahoo and others: the server address from their help pages.</p>
				<input class="input" aria-label="Server address" placeholder="https://caldav.example.com" required bind:value={form.url} />
				<div class="grid gap-2 sm:grid-cols-2">
					<input class="input" aria-label="User name" placeholder="User name" required autocomplete="off" bind:value={form.username} />
					<input class="input" type="password" aria-label="Password" placeholder="Password" required autocomplete="new-password" bind:value={form.password} />
				</div>
				<label class="flex items-center gap-2 text-slate-600"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.push} /> Also send my NERP events there</label>
				{#if found}
					<ul class="space-y-1" data-found>
						{#each found as c (c.url)}
							<li class="flex items-center gap-2 rounded-lg bg-white px-3 py-2"><span class="flex-1 truncate">{c.name}</span><button type="button" class="btn btn-primary px-3 py-1" disabled={!!busy} onclick={() => connect(c.url)}>{busy === c.url ? 'Connecting…' : 'Sync this one'}</button></li>
						{/each}
					</ul>
				{:else}
					<div class="flex gap-2"><button class="btn btn-primary" disabled={busy === 'find'}>{busy === 'find' ? 'Looking…' : 'Find calendars'}</button><button type="button" class="btn btn-secondary" onclick={() => (adding = false)}>Cancel</button></div>
				{/if}
			</form>
		{:else}
			<div class="flex flex-wrap gap-2">
				{#if providers.google}<button class="btn btn-secondary" disabled={!!busy} onclick={() => oauth('google')}>Connect Google Calendar</button>{/if}
				{#if providers.microsoft}<button class="btn btn-secondary" disabled={!!busy} onclick={() => oauth('microsoft')}>Connect Microsoft 365</button>{/if}
				<button class="btn btn-secondary" onclick={() => ((adding = true), (found = null))}>Connect a CalDAV account (iCloud, Nextcloud…)</button>
			</div>
		{/if}
	</div>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Close</button>
	{/snippet}
</Modal>
