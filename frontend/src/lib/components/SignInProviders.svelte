<script lang="ts">
	// Settings: other accounts employees may sign in with (Google, Microsoft, Apple, Facebook, X,
	// OpenID Connect providers). Matched by a verified email, or linked from one's profile.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type P = {
		id: string;
		kind: string;
		label: string;
		client_id: string;
		has_secret: boolean;
		issuer: string | null;
		team_id: string | null;
		key_id: string | null;
		enabled: boolean;
		linked: number;
	};
	const KINDS: [string, string][] = [
		['google', 'Google'],
		['microsoft', 'Microsoft'],
		['apple', 'Apple'],
		['facebook', 'Facebook'],
		['x', 'X'],
		['oidc', 'Other (OpenID Connect)']
	];
	let items = $state<P[]>([]);
	let redirect = $state('');
	let open = $state(false);
	let form = $state({
		id: '',
		kind: 'google',
		label: '',
		client_id: '',
		secret: '',
		issuer: '',
		team_id: '',
		key_id: '',
		enabled: true
	});
	let busy = $state(false);

	async function load() {
		try {
			const r = await api.get<{ items: P[]; redirect_uri: string }>('/directory/providers');
			items = r.items;
			redirect = r.redirect_uri;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	function edit(p?: P) {
		form = p
			? {
					id: p.id,
					kind: p.kind,
					label: p.label,
					client_id: p.client_id,
					secret: '',
					issuer: p.issuer ?? '',
					team_id: p.team_id ?? '',
					key_id: p.key_id ?? '',
					enabled: p.enabled
				}
			: {
					id: '',
					kind: 'google',
					label: '',
					client_id: '',
					secret: '',
					issuer: '',
					team_id: '',
					key_id: '',
					enabled: true
				};
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const { id, secret, ...rest } = form;
			const body = { ...rest, ...(secret ? { secret } : {}) };
			if (id) await api.put(`/directory/providers/${id}`, body);
			else await api.post('/directory/providers', body);
			open = false;
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function toggle(p: P) {
		await api.put(`/directory/providers/${p.id}`, { enabled: !p.enabled }).catch(toasts.error);
		await load();
	}
	async function remove(p: P) {
		if (!confirm(`Remove “${p.label}”? ${p.linked} linked account(s) are forgotten.`)) return;
		await api.del(`/directory/providers/${p.id}`).catch(toasts.error);
		await load();
	}
	const kindName = (k: string) => KINDS.find(([x]) => x === k)?.[1] ?? k;
</script>

<section class="card space-y-3 p-6" aria-label="Other sign-in accounts" data-providers>
	<div class="flex items-start gap-3">
		<div class="flex-1">
			<h2 class="text-lg font-semibold text-slate-900">Sign in with other accounts</h2>
			<p class="text-sm text-slate-500">
				Buttons on the login page for existing employees. An account is matched by an email address
				the provider has verified; otherwise people link it from their profile first. No accounts
				are made this way.
			</p>
		</div>
		<button class="btn btn-secondary shrink-0" onclick={() => edit()}>Add</button>
	</div>
	{#each items as p (p.id)}
		<div
			class="flex flex-wrap items-center gap-2 border-t border-slate-100 pt-2 text-sm"
			data-provider-row={p.kind}
		>
			<span class="flex-1"
				><span class="font-medium">{p.label}</span>
				<span class="text-slate-500">· {kindName(p.kind)} · {p.linked} linked</span></span
			>
			<label class="flex items-center gap-1.5"
				><input type="checkbox" checked={p.enabled} onchange={() => toggle(p)} /> On</label
			>
			<button class="text-brand-600 hover:underline" onclick={() => edit(p)}>Change</button>
			<button
				class="text-slate-400 hover:text-red-600"
				aria-label="Remove {p.label}"
				onclick={() => remove(p)}>×</button
			>
		</div>
	{/each}
	<div>
		<span class="label">Redirect URI to register with each provider</span>
		<code class="block rounded-lg bg-slate-50 px-3 py-2 text-sm break-all">{redirect}</code>
	</div>
</section>

<Modal bind:open title={form.id ? 'Change the sign-in' : 'Add a sign-in'}>
	<form id="sp-form" class="space-y-3 text-sm" onsubmit={save}>
		<div class="grid gap-3 sm:grid-cols-2">
			<div>
				<label class="label" for="sp-kind">Provider</label><select
					id="sp-kind"
					class="input"
					bind:value={form.kind}
					disabled={!!form.id}
					>{#each KINDS as [k, l] (k)}<option value={k}>{l}</option>{/each}</select
				>
			</div>
			<div>
				<label class="label" for="sp-label">Button</label><input
					id="sp-label"
					class="input"
					maxlength="60"
					placeholder="Sign in with {kindName(form.kind)}"
					bind:value={form.label}
				/>
			</div>
		</div>
		{#if form.kind === 'oidc'}
			<div>
				<label class="label" for="sp-issuer">Issuer URL</label><input
					id="sp-issuer"
					class="input"
					required
					placeholder="https://id.example.com"
					bind:value={form.issuer}
				/>
			</div>
		{/if}
		<div>
			<label class="label" for="sp-client"
				>{form.kind === 'apple' ? 'Services ID' : 'Client ID'}</label
			><input
				id="sp-client"
				class="input"
				required
				autocomplete="off"
				bind:value={form.client_id}
			/>
		</div>
		{#if form.kind === 'apple'}
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<label class="label" for="sp-team">Team ID</label><input
						id="sp-team"
						class="input"
						required
						bind:value={form.team_id}
					/>
				</div>
				<div>
					<label class="label" for="sp-key">Key ID</label><input
						id="sp-key"
						class="input"
						required
						bind:value={form.key_id}
					/>
				</div>
			</div>
			<div>
				<label class="label" for="sp-secret">Private key (the .p8 file's text)</label><textarea
					id="sp-secret"
					class="input font-mono text-xs"
					rows="5"
					placeholder={form.id ? 'Unchanged' : '-----BEGIN PRIVATE KEY-----'}
					bind:value={form.secret}></textarea>
			</div>
		{:else}
			<div>
				<label class="label" for="sp-secret">Client secret</label><input
					id="sp-secret"
					class="input"
					type="password"
					autocomplete="new-password"
					placeholder={form.id ? 'Unchanged' : ''}
					bind:value={form.secret}
				/>
			</div>
		{/if}
		{#if form.kind === 'microsoft'}<p class="text-xs text-slate-500">
				Work and school accounts are matched by email only when Microsoft confirms the domain (the
				optional <code>xms_edov</code> claim); personal accounts always.
			</p>{/if}
		{#if form.kind === 'x'}<p class="text-xs text-slate-500">
				X shares the email only with the “users.email” permission; without it, people link their X
				account from their profile.
			</p>{/if}
		<label class="flex items-center gap-2"
			><input type="checkbox" bind:checked={form.enabled} /> On</label
		>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="sp-form" disabled={busy}>Save</button>
	{/snippet}
</Modal>
