<script lang="ts">
	// Connecting personal mailboxes so client emails are filed in the CRM and can be
	// answered from records.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { myMailboxes } from '#lib/components/crm/EmailComposer.svelte';
	import { formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { User } from '#lib/types.ts';

	type Mailbox = {
		id: string;
		email: string;
		display_name: string;
		imap_host: string;
		imap_port: number;
		imap_security: string;
		smtp_host: string;
		smtp_port: number;
		smtp_security: string;
		smtp_auth: boolean;
		username: string;
		allow_invalid_certs: boolean;
		shared_with: string[];
		create_leads: boolean;
		sync_days: number;
		sync_enabled: boolean;
		last_sync_at: string | null;
		last_error: string | null;
		is_owner: boolean;
		auth_kind: 'password' | 'google' | 'microsoft';
	};

	// Well-known providers that accept app passwords over IMAP/SMTP.
	const PRESETS: Record<string, Partial<Mailbox>> = {
		custom: {},
		fastmail: { imap_host: 'imap.fastmail.com', imap_port: 993, imap_security: 'tls', smtp_host: 'smtp.fastmail.com', smtp_port: 465, smtp_security: 'tls' },
		zoho: { imap_host: 'imap.zoho.com', imap_port: 993, imap_security: 'tls', smtp_host: 'smtp.zoho.com', smtp_port: 465, smtp_security: 'tls' },
		icloud: { imap_host: 'imap.mail.me.com', imap_port: 993, imap_security: 'tls', smtp_host: 'smtp.mail.me.com', smtp_port: 587, smtp_security: 'starttls' },
		yahoo: { imap_host: 'imap.mail.yahoo.com', imap_port: 993, imap_security: 'tls', smtp_host: 'smtp.mail.yahoo.com', smtp_port: 465, smtp_security: 'tls' }
	};

	let items = $state<Mailbox[]>([]);
	let providers = $state<{ google: boolean; microsoft: boolean; redirect_uri: string } | null>(null);
	async function load() {
		items = (await api.get<{ items: Mailbox[] }>('/crm/mailboxes')).items;
		myMailboxes(true);
	}
	onMount(async () => {
		// Back from Google / Microsoft sign-in.
		const done = page.url.searchParams.get('connected');
		const err = page.url.searchParams.get('error');
		if (done) toasts.success(`${done} connected — recent client emails are being filed`);
		if (err) toasts.error(new Error(err));
		if (done || err) goto('/crm/mailboxes', { replaceState: true });
		await load();
		providers = await api.get('/crm/mailboxes/oauth/providers');
	});

	/** Sends the browser to Google / Microsoft to sign in; it comes back to this page. */
	async function signIn(kind: 'google' | 'microsoft', mailboxId?: string) {
		try {
			const r = await api.post<{ url: string }>(`/crm/mailboxes/oauth/${kind}/start`, mailboxId ? { mailbox_id: mailboxId } : {});
			window.location.href = r.url;
		} catch (e) {
			toasts.error(e);
		}
	}
	const PROVIDER_NAME = { google: 'Google', microsoft: 'Microsoft', password: '' };

	type Draft = Omit<Mailbox, 'id' | 'last_sync_at' | 'last_error' | 'is_owner' | 'shared_with' | 'auth_kind'> & { id: string | null; password: string; shared: PickedUser[]; auth_kind: Mailbox['auth_kind'] };
	let draft = $state<Draft | null>(null);
	let open = $state(false);
	let saving = $state(false);
	let testing = $state(false);
	let result = $state<{ imap: { ok: boolean; error: string | null }; smtp: { ok: boolean; error: string | null } } | null>(null);

	async function edit(m: Mailbox | null) {
		result = null;
		const shared: PickedUser[] = [];
		for (const id of m?.shared_with ?? []) {
			try {
				const u = await api.get<User>(`/users/${id}`);
				shared.push({ id, name: u.full_name, avatar_file_id: u.avatar_file_id });
			} catch {
				/* user gone */
			}
		}
		draft = m
			? { ...m, password: '', shared }
			: { id: null, auth_kind: 'password', email: '', display_name: '', imap_host: '', imap_port: 993, imap_security: 'tls', smtp_host: '', smtp_port: 587, smtp_security: 'starttls', smtp_auth: true, username: '', allow_invalid_certs: false, create_leads: false, sync_days: 7, sync_enabled: true, password: '', shared: [] };
		open = true;
	}

	function preset(name: string) {
		if (draft) Object.assign(draft, PRESETS[name]);
	}
	// Guess servers from the address for custom domains: imap./smtp. + domain.
	function guess() {
		if (!draft || draft.id) return;
		const domain = draft.email.split('@')[1];
		if (domain && !draft.imap_host) draft.imap_host = `imap.${domain}`;
		if (domain && !draft.smtp_host) draft.smtp_host = `smtp.${domain}`;
	}

	function body() {
		const d = draft!;
		if (d.auth_kind !== 'password') return { display_name: d.display_name, create_leads: d.create_leads, sync_days: Number(d.sync_days), sync_enabled: d.sync_enabled, shared_with: d.shared.map((u) => u.id) };
		return {
			email: d.email, display_name: d.display_name, imap_host: d.imap_host, imap_port: Number(d.imap_port), imap_security: d.imap_security,
			smtp_host: d.smtp_host, smtp_port: Number(d.smtp_port), smtp_security: d.smtp_security, smtp_auth: d.smtp_auth,
			username: d.username || d.email, password: d.password || undefined, allow_invalid_certs: d.allow_invalid_certs,
			create_leads: d.create_leads, sync_days: Number(d.sync_days), sync_enabled: d.sync_enabled, shared_with: d.shared.map((u) => u.id)
		};
	}

	async function test() {
		testing = true;
		try {
			result = await api.post('/crm/mailboxes/test', body());
		} catch (e) {
			toasts.error(e);
		} finally {
			testing = false;
		}
	}

	async function save() {
		saving = true;
		try {
			if (draft!.id) await api.patch(`/crm/mailboxes/${draft!.id}`, body());
			else await api.post('/crm/mailboxes', body());
			open = false;
			await load();
			toasts.success(draft!.id ? 'Mailbox saved' : 'Mailbox connected — recent client emails are being filed');
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}

	async function remove(m: Mailbox) {
		if (!confirm(`Disconnect ${m.email}? Emails already filed stay on their records.`)) return;
		try {
			await api.del(`/crm/mailboxes/${m.id}`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	async function syncNow(m: Mailbox) {
		await api.post(`/crm/mailboxes/${m.id}/sync`);
		toasts.success('Checking for new mail…');
		setTimeout(load, 4000);
	}
</script>

<svelte:head><title>Mailboxes · NERP</title></svelte:head>

<div class="max-w-4xl space-y-5">
	<div class="flex items-start justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Mailboxes</h1>
			<p class="text-sm text-slate-500">Emails with your clients are filed on their leads, contacts, companies and deals, and you can answer them from there. Private mail stays private: only messages with CRM clients are stored.</p>
		</div>
		<div class="flex shrink-0 flex-wrap justify-end gap-2">
			{#if providers?.google}<button class="btn btn-secondary" onclick={() => signIn('google')}><Icon name="mail" class="size-4" /> Connect Gmail</button>{/if}
			{#if providers?.microsoft}<button class="btn btn-secondary" onclick={() => signIn('microsoft')}><Icon name="mail" class="size-4" /> Connect Microsoft 365</button>{/if}
			<button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> Other mailbox (IMAP)</button>
		</div>
	</div>
	{#if providers && (!providers.google || !providers.microsoft)}
		<p class="rounded-xl bg-slate-50 px-4 py-3 text-xs text-slate-500">
			{!providers.google && !providers.microsoft ? 'Gmail and Microsoft 365 sign-in are' : !providers.google ? 'Gmail sign-in is' : 'Microsoft 365 sign-in is'} not set up on this server.
			An administrator registers an OAuth app with the provider (redirect URI <code class="text-slate-700">{providers.redirect_uri}</code>) and sets
			{!providers.google ? 'GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET' : ''}{!providers.google && !providers.microsoft ? ' and ' : ''}{!providers.microsoft ? 'MICROSOFT_CLIENT_ID / MICROSOFT_CLIENT_SECRET' : ''}.
		</p>
	{/if}

	<section class="card">
		<ul class="divide-y divide-slate-50">
			{#each items as m (m.id)}
				<li class="flex items-center gap-3 px-5 py-3 text-sm">
					<Icon name="mail" class="size-5 shrink-0 {m.last_error ? 'text-red-500' : 'text-brand-600'}" />
					<div class="min-w-0 flex-1">
						<p class="font-medium text-slate-900">{m.display_name ? `${m.display_name} · ` : ''}{m.email}{#if m.auth_kind !== 'password'}<span class="badge ml-2 bg-brand-50 text-brand-700">{PROVIDER_NAME[m.auth_kind]}</span>{/if}{#if !m.is_owner}<span class="badge ml-2 bg-slate-100 text-slate-500">shared with you</span>{/if}</p>
						<p class="text-xs {m.last_error ? 'text-red-600' : 'text-slate-500'}">
							{#if m.last_error}{m.last_error}{:else if !m.sync_enabled}Receiving paused{:else if m.last_sync_at}Checked {formatRelative(m.last_sync_at)}{:else}Not checked yet{/if}
							{#if m.create_leads} · new senders become leads{/if}
						</p>
					</div>
					{#if m.is_owner && m.auth_kind !== 'password' && m.last_error}
						<button class="btn btn-secondary text-sm" onclick={() => signIn(m.auth_kind as 'google' | 'microsoft', m.id)}>Reconnect</button>
					{/if}
					<button class="btn btn-ghost text-sm" onclick={() => syncNow(m)}><Icon name="refresh" class="size-4" /> Check now</button>
					{#if m.is_owner}
						<button class="btn btn-ghost text-sm" onclick={() => edit(m)}><Icon name="edit" class="size-4" /> Edit</button>
						<button class="text-slate-300 hover:text-red-600" aria-label="Disconnect {m.email}" onclick={() => remove(m)}><Icon name="trash" class="size-4" /></button>
					{/if}
				</li>
			{:else}
				<li class="px-5 py-6 text-sm text-slate-500">No mailbox connected yet.</li>
			{/each}
		</ul>
	</section>
</div>

<Modal bind:open title={draft?.id ? 'Mailbox settings' : 'Connect a mailbox'} size="lg">
	{#if draft}
		<div class="space-y-4 text-sm">
			{#if draft.auth_kind !== 'password'}
				<div class="grid gap-3 sm:grid-cols-2">
					<div><span class="label">Email address</span><p class="py-2">{draft.email} <span class="text-xs text-slate-500">· signed in with {PROVIDER_NAME[draft.auth_kind]}</span></p></div>
					<div><label class="label" for="mb-name">Sender name</label><input id="mb-name" class="input" maxlength="200" bind:value={draft.display_name} /></div>
				</div>
			{:else}
			<div class="grid gap-3 sm:grid-cols-2">
				<div><label class="label" for="mb-email">Email address</label><input id="mb-email" class="input" type="email" bind:value={draft.email} onblur={guess} /></div>
				<div><label class="label" for="mb-name">Sender name</label><input id="mb-name" class="input" maxlength="200" placeholder="Your name" bind:value={draft.display_name} /></div>
				<div><label class="label" for="mb-user">Login <span class="font-normal text-slate-400">(if not the address)</span></label><input id="mb-user" class="input" bind:value={draft.username} placeholder={draft.email} /></div>
				<div><label class="label" for="mb-pass">{draft.id ? 'New password (optional)' : 'Password or app password'}</label><input id="mb-pass" class="input" type="password" autocomplete="new-password" bind:value={draft.password} /></div>
			</div>
			{#if !draft.id}
				<div class="flex flex-wrap items-center gap-2 text-xs">
					<span class="text-slate-500">Provider:</span>
					{#each [['fastmail', 'Fastmail'], ['zoho', 'Zoho'], ['icloud', 'iCloud'], ['yahoo', 'Yahoo'], ['custom', 'Other / own server']] as [k, l] (k)}
						<button type="button" class="rounded-full border border-slate-200 px-2.5 py-1 hover:bg-slate-50" onclick={() => preset(k)}>{l}</button>
					{/each}
				</div>
			{/if}
			<div class="grid gap-3 sm:grid-cols-[1fr_6rem_8rem]">
				<div><label class="label" for="mb-ih">Incoming (IMAP) server</label><input id="mb-ih" class="input" bind:value={draft.imap_host} placeholder="imap.example.com" /></div>
				<div><label class="label" for="mb-ip">Port</label><input id="mb-ip" class="input" type="number" bind:value={draft.imap_port} /></div>
				<div><label class="label" for="mb-is">Security</label><select id="mb-is" class="input" bind:value={draft.imap_security}><option value="tls">SSL/TLS</option><option value="starttls">STARTTLS</option><option value="none">None</option></select></div>
				<div><label class="label" for="mb-sh">Outgoing (SMTP) server</label><input id="mb-sh" class="input" bind:value={draft.smtp_host} placeholder="smtp.example.com" /></div>
				<div><label class="label" for="mb-sp">Port</label><input id="mb-sp" class="input" type="number" bind:value={draft.smtp_port} /></div>
				<div><label class="label" for="mb-ss">Security</label><select id="mb-ss" class="input" bind:value={draft.smtp_security}><option value="tls">SSL/TLS</option><option value="starttls">STARTTLS</option><option value="none">None</option></select></div>
			</div>
			{/if}
			<div class="space-y-2">
				{#if draft.auth_kind === 'password'}<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.smtp_auth} /> The outgoing server needs the login</label>{/if}
				<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.create_leads} /> Create a lead when someone unknown writes</label>
				<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.sync_enabled} /> Receive mail</label>
				{#if draft.auth_kind === 'password'}<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.allow_invalid_certs} /> Accept self-signed certificates <span class="text-xs text-slate-400">(own or test servers only)</span></label>{/if}
				<div class="flex items-center gap-2">
					<label for="mb-days">On connecting, file mail from the last</label>
					<input id="mb-days" class="input w-20 py-1" type="number" min="0" max="365" bind:value={draft.sync_days} /> days
				</div>
			</div>
			<div>
				<span class="label">Colleagues who may also send from it</span>
				<MultiUserPicker bind:value={draft.shared} />
			</div>
			{#if result}
				<div class="grid gap-2 sm:grid-cols-2" role="status">
					{#each [['Incoming (IMAP)', result.imap], ['Outgoing (SMTP)', result.smtp]] as [label, r] (label)}
						{@const res = r as { ok: boolean; error: string | null }}
						<p class="rounded-lg px-3 py-2 text-xs {res.ok ? 'bg-emerald-50 text-emerald-800' : 'bg-red-50 text-red-800'}">{label}: {res.ok ? 'connected' : res.error}</p>
					{/each}
				</div>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		{#if draft?.auth_kind === 'password'}<button class="btn btn-secondary mr-auto" disabled={testing || !draft?.imap_host || !draft?.smtp_host || (!draft?.id && !draft?.password)} onclick={test}>{testing ? 'Testing…' : 'Test connection'}</button>{/if}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" disabled={saving || (draft?.auth_kind === 'password' && (!draft?.email || !draft?.imap_host || !draft?.smtp_host || (!draft?.id && !draft?.password)))} onclick={save}>{saving ? 'Connecting…' : draft?.id ? 'Save' : 'Connect'}</button>
	{/snippet}
</Modal>
