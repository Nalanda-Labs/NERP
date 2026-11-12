<script lang="ts">
	// SAML 2.0 single sign-on: NERP's addresses and metadata for the identity provider's
	// administrator, the identity provider's metadata (from its address, or pasted), and who
	// may sign in.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Saml = {
		config: {
			enabled: boolean;
			metadata_url: string;
			metadata_xml: string;
			button_label: string;
			allowed_domains: string[];
			auto_create: boolean;
			email_attribute: string;
			sp_certificate: string;
		};
		entity_id: string;
		acs_url: string;
		metadata_url: string;
		idp: { ok: boolean; entity_id?: string; error?: string } | null;
	};
	let s = $state<Saml | null>(null);
	let form = $state({ enabled: false, metadata_url: '', metadata_xml: '', button_label: '', domains: '', auto_create: false, email_attribute: '' });
	let paste = $state(false);
	let busy = $state('');

	function take(x: Saml) {
		s = x;
		const c = x.config;
		form = {
			enabled: c.enabled,
			metadata_url: c.metadata_url,
			metadata_xml: c.metadata_url ? '' : c.metadata_xml,
			button_label: c.button_label,
			domains: c.allowed_domains.join(', '),
			auto_create: c.auto_create,
			email_attribute: c.email_attribute
		};
		paste = !c.metadata_url && !!c.metadata_xml;
	}
	onMount(async () => {
		try {
			take((await api.get<{ saml: Saml }>('/directory')).saml);
		} catch (e) {
			toasts.error(e);
		}
	});
	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = 'save';
		try {
			take(
				await api.put<Saml>('/directory/saml', {
					enabled: form.enabled,
					metadata_url: paste ? '' : form.metadata_url,
					metadata_xml: paste ? form.metadata_xml : '',
					button_label: form.button_label,
					allowed_domains: form.domains.split(/[\s,;]+/).filter(Boolean),
					auto_create: form.auto_create,
					email_attribute: form.email_attribute
				})
			);
			toasts.success('SAML settings saved');
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = '';
		}
	}
	async function reread() {
		busy = 'refresh';
		try {
			take(await api.post<Saml>('/directory/saml/refresh'));
			toasts.success('Metadata read again');
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = '';
		}
	}
	const copy = (t: string) => navigator.clipboard?.writeText(t).then(() => toasts.success('Copied'));
</script>

{#if s}
	<form class="card space-y-4 p-6" onsubmit={save} data-saml-settings>
		<div class="flex items-start justify-between gap-4">
			<div>
				<h2 class="text-lg font-semibold text-slate-900">SAML single sign-on</h2>
				<p class="text-sm text-slate-500">
					For identity providers that speak SAML 2.0: Microsoft Entra ID or ADFS, Okta, Google Workspace, Keycloak,
					Shibboleth, OneLogin…
				</p>
			</div>
			<label class="flex shrink-0 items-center gap-2 text-sm font-medium"><input type="checkbox" bind:checked={form.enabled} data-saml-enabled /> On</label>
		</div>

		<div class="space-y-2 rounded-lg bg-slate-50 p-3 text-sm">
			<p class="font-medium text-slate-700">Give the identity provider these (or just the metadata address):</p>
			{#each [['Metadata', s.metadata_url], ['Entity ID', s.entity_id], ['Assertion consumer service (HTTP-POST)', s.acs_url]] as [label, value] (label)}
				<div class="flex items-center gap-2">
					<span class="w-64 shrink-0 text-slate-500">{label}</span>
					<code class="flex-1 break-all" data-saml-address={label}>{value}</code>
					<button type="button" class="btn btn-ghost px-2" onclick={() => copy(value)}>Copy</button>
				</div>
			{/each}
			<p class="text-xs text-slate-500">
				Ask for the email address as the NameID or as an <code>email</code> attribute, and <code>givenName</code> and
				<code>sn</code> for names. NERP signs its requests; responses must be signed (SHA-256 or stronger).
			</p>
		</div>

		<div class="space-y-2">
			<div class="flex items-center gap-4 text-sm">
				<span class="label mb-0">The identity provider's metadata</span>
				<label class="flex items-center gap-1"><input type="radio" name="saml-md" checked={!paste} onchange={() => (paste = false)} /> From its address</label>
				<label class="flex items-center gap-1"><input type="radio" name="saml-md" checked={paste} onchange={() => (paste = true)} data-saml-paste /> Paste it</label>
			</div>
			{#if paste}
				<textarea class="input font-mono text-xs" rows="6" placeholder="<md:EntityDescriptor …>" bind:value={form.metadata_xml} aria-label="Identity provider metadata"></textarea>
			{:else}
				<div class="flex gap-2">
					<input class="input" placeholder="https://login.example.com/…/federationmetadata.xml" bind:value={form.metadata_url} aria-label="Metadata address" data-saml-metadata-url />
					{#if s.config.metadata_url}<button type="button" class="btn btn-secondary shrink-0" disabled={!!busy} onclick={reread}>{busy === 'refresh' ? 'Reading…' : 'Read again'}</button>{/if}
				</div>
			{/if}
			{#if s.idp}
				<p class="text-sm {s.idp.ok ? 'text-emerald-700' : 'text-red-700'}" data-saml-idp>
					{s.idp.ok ? `Identity provider: ${s.idp.entity_id}` : s.idp.error}
				</p>
			{/if}
		</div>

		<div class="grid gap-4 sm:grid-cols-2">
			<div>
				<label class="label" for="saml-label">Button text</label>
				<input id="saml-label" class="input" bind:value={form.button_label} />
			</div>
			<div>
				<label class="label" for="saml-domains">Allowed email domains</label>
				<input id="saml-domains" class="input" placeholder="Any — or e.g. example.com" bind:value={form.domains} />
			</div>
			<div>
				<label class="label" for="saml-email">Email attribute</label>
				<input id="saml-email" class="input" placeholder="Usual names, else the NameID" bind:value={form.email_attribute} />
			</div>
		</div>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={form.auto_create} /> Create an employee account the first time someone signs in</label>
		<div class="flex gap-2">
			<button class="btn btn-primary" disabled={!!busy}>{busy === 'save' ? 'Saving…' : 'Save'}</button>
			<a class="btn btn-secondary" href="/api/auth/saml/metadata" target="_blank" rel="noopener">NERP's metadata</a>
		</div>
		<p class="text-xs text-slate-500">People are matched by email. Sign-ins must start here (the button on the login page); a code from the authenticator app is still asked for when the person has one or the company requires it.</p>
	</form>
{/if}
