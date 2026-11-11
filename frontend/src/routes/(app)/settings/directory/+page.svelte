<script lang="ts">
	// Employees from an LDAP directory, and single sign-on through an OpenID Connect provider.
	import { onMount } from 'svelte';
	import SignInProviders from '#lib/components/SignInProviders.svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	interface Ldap {
		enabled: boolean;
		url: string;
		starttls: boolean;
		skip_tls_verify: boolean;
		bind_dn: string;
		base_dn: string;
		user_filter: string;
		attr_id: string;
		attr_email: string;
		attr_first_name: string;
		attr_last_name: string;
		attr_phone: string;
		attr_position: string;
		attr_department: string;
		auto_sync: boolean;
	}
	interface Sso {
		enabled: boolean;
		issuer: string;
		client_id: string;
		button_label: string;
		allowed_domains: string[];
		auto_create: boolean;
	}
	interface SyncResult { found?: number; created?: number; updated?: number; reactivated?: number; deactivated?: number; skipped?: number; error?: string }
	interface Config {
		/** The server was started with LDAP_ENABLED=true. */
		ldap_available: boolean;
		ldap: Ldap;
		ldap_has_password: boolean;
		ldap_synced_at: string | null;
		ldap_sync_result: SyncResult | null;
		sso: Sso;
		sso_has_secret: boolean;
		sso_redirect_uri: string;
		accounts: Record<string, number>;
	}

	let cfg = $state<Config | null>(null);
	let ldap = $state<Ldap | null>(null);
	let sso = $state<Sso | null>(null);
	let domains = $state('');
	let bindPassword = $state('');
	let clientSecret = $state('');
	let busy = $state('');
	let ldapTest = $state<{ ok: boolean; found?: number; error?: string; sample?: { email: string; name: string; dn: string }[] } | null>(null);
	let ssoTest = $state<{ ok: boolean; error?: string } | null>(null);

	// Refreshes one form (or both) without touching unsaved edits in the other.
	function load(c: Config, part: 'ldap' | 'sso' | 'both' = 'both') {
		cfg = c;
		if (part !== 'sso') {
			ldap = { ...c.ldap };
			bindPassword = '';
		}
		if (part !== 'ldap') {
			sso = { ...c.sso };
			domains = c.sso.allowed_domains.join(', ');
			clientSecret = '';
		}
	}
	onMount(async () => load(await api.get<Config>('/directory')));

	async function run(what: string, fn: () => Promise<void>) {
		busy = what;
		try {
			await fn();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = '';
		}
	}

	const saveLdap = (e?: SubmitEvent) => {
		e?.preventDefault();
		return run('ldap', async () => {
			load(await api.put<Config>('/directory/ldap', { ...ldap, ...(bindPassword ? { bind_password: bindPassword } : {}) }), 'ldap');
			toasts.success('Directory settings saved');
		});
	};
	const testLdap = () => run('ldap-test', async () => {
		ldapTest = null;
		ldapTest = await api.post('/directory/ldap/test');
	});
	const syncLdap = () => run('ldap-sync', async () => {
		const r = await api.post<SyncResult>('/directory/ldap/sync');
		if (r.error) toasts.error(r.error);
		else toasts.success(`Synced: ${r.created} added, ${r.updated} updated, ${r.deactivated} deactivated`);
		const c = await api.get<Config>('/directory');
		cfg = { ...c, ldap: cfg!.ldap, sso: cfg!.sso };
	});
	const saveSso = (e?: SubmitEvent) => {
		e?.preventDefault();
		return run('sso', async () => {
			const allowed = domains.split(/[\s,;]+/).filter(Boolean);
			load(await api.put<Config>('/directory/sso', { ...sso, allowed_domains: allowed, ...(clientSecret ? { client_secret: clientSecret } : {}) }), 'sso');
			toasts.success('Sign-on settings saved');
		});
	};
	const testSso = () => run('sso-test', async () => {
		ssoTest = null;
		ssoTest = await api.post('/directory/sso/test');
	});
	const attrs: [keyof Ldap, string][] = [
		['attr_id', 'Unique ID'],
		['attr_email', 'Email'],
		['attr_first_name', 'First name'],
		['attr_last_name', 'Last name'],
		['attr_phone', 'Phone'],
		['attr_position', 'Position'],
		['attr_department', 'Department']
	];
</script>

<svelte:head><title>Directory & sign-on · NERP</title></svelte:head>

<div class="max-w-3xl space-y-5">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">Directory & sign-on</h1>
		<p class="text-sm text-slate-500">Bring employees in from your company directory, and let them sign in with your identity provider.</p>
	</div>

	{#if cfg && ldap && sso}
		{#if !cfg.ldap_available}
			<div class="card p-6" data-ldap-off>
				<h2 class="text-lg font-semibold text-slate-900">LDAP directory</h2>
				<p class="mt-1 text-sm text-slate-500">LDAP sign-in is turned off on this server. To use it, start the server with <code class="font-mono">LDAP_ENABLED=true</code> (in its <code class="font-mono">.env</code>), then set it up here.</p>
			</div>
		{:else}
		<form class="card space-y-4 p-6" onsubmit={saveLdap} data-ldap>
			<div class="flex items-start justify-between gap-4">
				<div>
					<h2 class="text-lg font-semibold text-slate-900">LDAP directory</h2>
					<p class="text-sm text-slate-500">Active Directory, OpenLDAP and other LDAP servers. Directory employees sign in with their directory password; someone in the directory without an account gets one when they first sign in.</p>
				</div>
				<label class="flex shrink-0 items-center gap-2 text-sm font-medium"><input type="checkbox" bind:checked={ldap.enabled} data-ldap-enabled /> On</label>
			</div>
			<div class="grid gap-4 sm:grid-cols-2">
				<div class="sm:col-span-2">
					<label class="label" for="ldap-url">Server address</label>
					<input id="ldap-url" class="input" placeholder="ldaps://dc1.example.com:636" bind:value={ldap.url} />
				</div>
				<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={ldap.starttls} /> Use StartTLS (ldap:// only)</label>
				<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={ldap.skip_tls_verify} /> Accept any certificate (testing only)</label>
				<div>
					<label class="label" for="ldap-bind">Service account DN</label>
					<input id="ldap-bind" class="input" placeholder="cn=nerp,ou=service,dc=example,dc=com" bind:value={ldap.bind_dn} />
				</div>
				<div>
					<label class="label" for="ldap-pass">Service account password</label>
					<input id="ldap-pass" class="input" type="password" autocomplete="new-password" placeholder={cfg.ldap_has_password ? 'Saved — leave empty to keep' : ''} bind:value={bindPassword} />
				</div>
				<div>
					<label class="label" for="ldap-base">Base DN</label>
					<input id="ldap-base" class="input" placeholder="dc=example,dc=com" bind:value={ldap.base_dn} />
				</div>
				<div>
					<label class="label" for="ldap-filter">Employee filter</label>
					<input id="ldap-filter" class="input font-mono text-sm" bind:value={ldap.user_filter} />
				</div>
			</div>
			<details class="rounded-lg border border-slate-200 px-4 py-3">
				<summary class="cursor-pointer text-sm font-medium text-slate-700">Attributes</summary>
				<div class="mt-3 grid gap-3 sm:grid-cols-3">
					{#each attrs as [key, label] (key)}
						<div>
							<label class="label" for="attr-{key}">{label}</label>
							<input id="attr-{key}" class="input font-mono text-sm" bind:value={ldap[key] as string} />
						</div>
					{/each}
				</div>
				<p class="mt-2 text-xs text-slate-500">Departments are matched by name when an account is created. For Active Directory use sAMAccountName or objectGUID as the ID.</p>
			</details>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={ldap.auto_sync} /> Sync every hour</label>

			{#if ldapTest}
				<div class="rounded-lg px-3 py-2 text-sm {ldapTest.ok ? 'bg-emerald-50 text-emerald-800' : 'bg-red-50 text-red-700'}" data-ldap-test>
					{#if ldapTest.ok}
						Connected — {ldapTest.found} employee{ldapTest.found === 1 ? '' : 's'} found{ldapTest.sample?.length ? ', e.g.' : '.'}
						<ul class="mt-1 list-disc pl-5">
							{#each ldapTest.sample ?? [] as p (p.dn)}<li>{p.name} &lt;{p.email}&gt;</li>{/each}
						</ul>
					{:else}
						{ldapTest.error}
					{/if}
				</div>
			{/if}
			{#if cfg.ldap_synced_at}
				<p class="text-sm text-slate-500" data-ldap-last>
					Last sync {formatDateTime(cfg.ldap_synced_at)}:
					{#if cfg.ldap_sync_result?.error}<span class="text-red-600">{cfg.ldap_sync_result.error}</span>
					{:else if cfg.ldap_sync_result}{cfg.ldap_sync_result.found} found, {cfg.ldap_sync_result.created} added, {cfg.ldap_sync_result.updated} updated, {cfg.ldap_sync_result.reactivated} reactivated, {cfg.ldap_sync_result.deactivated} deactivated{/if}
				</p>
			{/if}
			<div class="flex flex-wrap gap-2">
				<button class="btn btn-primary" disabled={!!busy}>{busy === 'ldap' ? 'Saving…' : 'Save'}</button>
				<button type="button" class="btn btn-secondary" disabled={!!busy || !cfg.ldap.url} onclick={testLdap}>{busy === 'ldap-test' ? 'Testing…' : 'Test connection'}</button>
				<button type="button" class="btn btn-secondary" disabled={!!busy || !cfg.ldap.enabled} onclick={syncLdap}>{busy === 'ldap-sync' ? 'Syncing…' : 'Sync now'}</button>
			</div>
			<p class="text-xs text-slate-500">Test and sync use the saved settings. People who leave the directory are deactivated, and turned back on if they return. Employees whose email is already in NERP are linked to their directory entry and then sign in with the directory password.</p>
		</form>
		{/if}

		<form class="card space-y-4 p-6" onsubmit={saveSso} data-sso-settings>
			<div class="flex items-start justify-between gap-4">
				<div>
					<h2 class="text-lg font-semibold text-slate-900">Single sign-on</h2>
					<p class="text-sm text-slate-500">OpenID Connect: Microsoft Entra ID, Google Workspace, Okta, Keycloak and others. Adds a button to the sign-in page.</p>
				</div>
				<label class="flex shrink-0 items-center gap-2 text-sm font-medium"><input type="checkbox" bind:checked={sso.enabled} data-sso-enabled /> On</label>
			</div>
			<div class="grid gap-4 sm:grid-cols-2">
				<div class="sm:col-span-2">
					<label class="label" for="sso-issuer">Issuer URL</label>
					<input id="sso-issuer" class="input" placeholder="https://login.microsoftonline.com/…/v2.0" bind:value={sso.issuer} />
				</div>
				<div>
					<label class="label" for="sso-client">Client ID</label>
					<input id="sso-client" class="input" bind:value={sso.client_id} />
				</div>
				<div>
					<label class="label" for="sso-secret">Client secret</label>
					<input id="sso-secret" class="input" type="password" autocomplete="new-password" placeholder={cfg.sso_has_secret ? 'Saved — leave empty to keep' : ''} bind:value={clientSecret} />
				</div>
				<div>
					<label class="label" for="sso-label">Button text</label>
					<input id="sso-label" class="input" bind:value={sso.button_label} />
				</div>
				<div>
					<label class="label" for="sso-domains">Allowed email domains</label>
					<input id="sso-domains" class="input" placeholder="Any — or e.g. example.com, example.org" bind:value={domains} />
				</div>
				<div class="sm:col-span-2">
					<span class="label">Redirect URI to register with the provider</span>
					<code class="block rounded-lg bg-slate-50 px-3 py-2 text-sm break-all" data-redirect-uri>{cfg.sso_redirect_uri}</code>
				</div>
			</div>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={sso.auto_create} /> Create an employee account the first time someone signs in</label>
			{#if ssoTest}
				<p class="rounded-lg px-3 py-2 text-sm {ssoTest.ok ? 'bg-emerald-50 text-emerald-800' : 'bg-red-50 text-red-700'}" data-sso-test>{ssoTest.ok ? 'The provider answered.' : ssoTest.error}</p>
			{/if}
			<div class="flex flex-wrap gap-2">
				<button class="btn btn-primary" disabled={!!busy}>{busy === 'sso' ? 'Saving…' : 'Save'}</button>
				<button type="button" class="btn btn-secondary" disabled={!!busy || !cfg.sso.issuer} onclick={testSso}>{busy === 'sso-test' ? 'Checking…' : 'Check provider'}</button>
			</div>
			<p class="text-xs text-slate-500">People are matched by email. The provider's sign-in replaces the NERP password; a code from the authenticator app is still asked for when the person has one or the company requires it.</p>
		</form>

		<SignInProviders />

		<p class="text-sm text-slate-500" data-accounts>
			Active accounts: {cfg.accounts.local ?? 0} with a NERP password, {cfg.accounts.ldap ?? 0} from the directory, {cfg.accounts.sso ?? 0} through sign-on only.
		</p>
	{/if}
</div>
