<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import type { Session, TwoFactorChallenge } from '#lib/types.ts';

	let email = $state('');
	let password = $state('');
	let code = $state('');
	let challenge = $state<string | null>(null);
	// The company requires two-step sign-in and this person hasn't set it up yet.
	let setup = $state<{ challenge: string; qr_png_base64: string; secret: string; mobile: string } | null>(null);
	let mobile = $state('');
	let recovery = $state<{ codes: string[]; session: Session } | null>(null);
	let useRecovery = $state(false);
	let error = $state('');
	let busy = $state(false);
	let company = $state<string | null>(null);
	let sso = $state<{ enabled: boolean; label: string | null }>({ enabled: false, label: null });
	let providers = $state<{ id: string; kind: string; label: string }[]>([]);

	onMount(async () => {
		// A sign-on attempt that failed comes back here with the reason.
		const params = new URLSearchParams(location.search);
		const ssoError = params.get('sso_error');
		if (ssoError) error = ssoError;
		// After the identity provider: the code, or setting one up.
		const step = params.get('step');
		const c = params.get('challenge');
		if (c && step === 'code') challenge = c;
		if (c && step === 'setup') startSetup(c).catch((e) => (error = e instanceof Error ? e.message : 'Sign-in failed'));
		if (ssoError || c) history.replaceState(history.state, '', location.pathname);
		const status = await api.get<{ needs_setup: boolean; company_name: string | null }>('/setup');
		if (status.needs_setup) return goto('/setup', { replace: true });
		company = status.company_name;
		api.get<typeof sso>('/auth/sso').then((s) => (sso = s)).catch(() => {});
		api.get<{ items: typeof providers }>('/auth/providers').then((r) => (providers = r.items)).catch(() => {});
		if (await session.restore()) goto('/', { replace: true });
	});

	function done(s: Session) {
		session.apply(s);
		const next = new URLSearchParams(location.search).get('next');
		// Only paths on this site: not //host or /\host (browsers read both as another site).
		goto(next && /^\/(?![/\\])/.test(next) ? next : '/', { replace: true });
	}

	async function startSetup(c: string) {
		const r = await api.post<{ qr_png_base64: string; secret: string; mobile: string | null }>('/auth/login/2fa-setup/start', { challenge: c });
		setup = { challenge: c, qr_png_base64: r.qr_png_base64, secret: r.secret, mobile: r.mobile ?? '' };
		mobile = r.mobile ?? '';
		code = '';
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			if (setup) {
				const r = await api.post<Session & { recovery_codes: string[] }>('/auth/login/2fa-setup', { challenge: setup.challenge, mobile, code });
				recovery = { codes: r.recovery_codes, session: r };
				setup = null;
			} else if (challenge) {
				done(await api.post<Session>('/auth/login/2fa', { challenge, code }));
			} else {
				const res = await api.post<Session | TwoFactorChallenge | { two_factor_setup_required: true; challenge: string }>('/auth/login', { email, password });
				if ('two_factor_required' in res) {
					challenge = res.challenge;
					code = '';
				} else if ('two_factor_setup_required' in res) {
					await startSetup(res.challenge);
				} else {
					done(res);
				}
			}
		} catch (err) {
			error = err instanceof Error ? err.message : 'Sign-in failed';
			// An expired or exhausted challenge sends you back to the password step.
			if (/expired|sign in again/.test(error)) {
				challenge = null;
				setup = null;
			}
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Sign in · NERP</title></svelte:head>

{#if recovery}
	<div class="card space-y-5 p-8" data-recovery-codes>
		<div>
			<h1 class="text-xl font-semibold text-slate-900">Save your recovery codes</h1>
			<p class="mt-1 text-sm text-slate-500">If you lose your phone, each of these codes lets you sign in once. Keep them somewhere safe — they are shown only now.</p>
		</div>
		<ul class="grid grid-cols-2 gap-2 rounded-xl bg-slate-50 p-4 font-mono text-sm">
			{#each recovery.codes as c (c)}<li>{c}</li>{/each}
		</ul>
		<button class="btn btn-primary w-full" onclick={() => done(recovery!.session)}>I've saved them — continue</button>
	</div>
{:else if setup}
	<form class="card space-y-5 p-8" onsubmit={submit} aria-label="Set up two-step sign-in">
		<div>
			<h1 class="text-xl font-semibold text-slate-900">Set up two-step sign-in</h1>
			<p class="mt-1 text-sm text-slate-500">Your company asks everyone to confirm sign-ins with a code from their phone.</p>
		</div>
		{#if error}
			<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700">{error.charAt(0).toUpperCase() + error.slice(1)}</p>
		{/if}
		<div>
			<label class="label" for="mobile">1. Your mobile number</label>
			<input id="mobile" class="input" type="tel" autocomplete="tel" placeholder="+44 7700 900123" required bind:value={mobile} />
		</div>
		<div>
			<p class="label">2. Scan this with an authenticator app on that phone</p>
			<p class="mb-2 text-xs text-slate-500">Google Authenticator, Microsoft Authenticator, 1Password, Aegis… any app that shows 6-digit codes.</p>
			<div class="flex flex-col items-center gap-2">
				<img src="data:image/png;base64,{setup.qr_png_base64}" alt="QR code for your authenticator app" class="size-44 rounded-lg border border-slate-200" />
				<details class="text-xs text-slate-500"><summary class="cursor-pointer">Can't scan it?</summary><p class="mt-1">Enter this key in the app: <code class="font-mono break-all text-slate-800" data-secret>{setup.secret}</code></p></details>
			</div>
		</div>
		<div>
			<label class="label" for="setup-code">3. Enter the code the app shows</label>
			<input id="setup-code" class="input text-center font-mono text-lg tracking-[0.5em]" inputmode="numeric" autocomplete="one-time-code" maxlength="7" placeholder="000000" required bind:value={code} />
		</div>
		<button class="btn btn-primary w-full" disabled={busy}>{busy ? 'Checking…' : 'Turn on and sign in'}</button>
	</form>
{:else}
<form class="card space-y-5 p-8" onsubmit={submit}>
	{#if !challenge}
		<div>
			<h1 class="text-xl font-semibold text-slate-900">Sign in</h1>
			<p class="mt-1 text-sm text-slate-500">{company ? `to ${company}` : 'to your company workspace'}</p>
		</div>
	{:else}
		<div>
			<h1 class="text-xl font-semibold text-slate-900">Two-factor authentication</h1>
			<p class="mt-1 text-sm text-slate-500">
				{useRecovery ? 'Enter one of your recovery codes.' : 'Enter the 6-digit code from your authenticator app.'}
			</p>
		</div>
	{/if}

	{#if error}
		<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700">{error.charAt(0).toUpperCase() + error.slice(1)}</p>
	{/if}

	{#if !challenge}
		<div>
			<label class="label" for="email">Email</label>
			<input id="email" class="input" type="email" autocomplete="username" required bind:value={email} />
		</div>
		<div>
			<div class="flex items-baseline justify-between">
				<label class="label" for="password">Password</label>
				<a href="/forgot-password" class="text-xs font-medium text-brand-600 hover:underline">Forgot password?</a>
			</div>
			<input id="password" class="input" type="password" autocomplete="current-password" required bind:value={password} />
		</div>
		<button class="btn btn-primary w-full" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
		{#if sso.enabled || providers.length}
			<div class="flex items-center gap-3 text-xs text-slate-400"><span class="h-px flex-1 bg-slate-200"></span>or<span class="h-px flex-1 bg-slate-200"></span></div>
		{/if}
		{#if sso.enabled}
			<a class="btn btn-secondary w-full" href="/api/auth/sso/start" data-sso>{sso.label}</a>
		{/if}
		{#each providers as p (p.id)}
			<a class="btn btn-secondary w-full" href="/api/auth/providers/{p.id}/start" data-provider={p.kind}>{p.label}</a>
		{/each}
	{:else}
		<div>
			<label class="label" for="code">{useRecovery ? 'Recovery code' : 'Authentication code'}</label>
			{#if useRecovery}
				<input id="code" class="input font-mono tracking-wider" autocomplete="off" placeholder="xxxxx-xxxxx" required bind:value={code} />
			{:else}
				<!-- svelte-ignore a11y_autofocus -->
				<input id="code" class="input text-center font-mono text-lg tracking-[0.5em]" inputmode="numeric" autocomplete="one-time-code" maxlength="7" placeholder="000000" required autofocus bind:value={code} />
			{/if}
		</div>
		<button class="btn btn-primary w-full" disabled={busy}>{busy ? 'Verifying…' : 'Verify'}</button>
		<div class="flex justify-between text-xs">
			<button type="button" class="font-medium text-brand-600 hover:underline" onclick={() => ((useRecovery = !useRecovery), (code = ''), (error = ''))}>
				{useRecovery ? 'Use authenticator app' : 'Use a recovery code'}
			</button>
			<button type="button" class="text-slate-500 hover:underline" onclick={() => ((challenge = null), (error = ''))}>Back</button>
		</div>
	{/if}
</form>
{/if}
