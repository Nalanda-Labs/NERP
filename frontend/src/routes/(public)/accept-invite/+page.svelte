<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import type { Session } from '#lib/types.ts';

	type Invite = { email: string; first_name: string; last_name: string; company_name: string };
	const token = $derived(page.url.searchParams.get('token') ?? '');
	let invite = $state<Invite | null>(null);
	let loadError = $state('');
	let password = $state('');
	let confirm = $state('');
	let busy = $state(false);
	let error = $state('');

	onMount(async () => {
		if (!token) {
			loadError = 'This invitation link is incomplete.';
			return;
		}
		try {
			invite = await api.get<Invite>(`/auth/invite/${encodeURIComponent(token)}`);
		} catch (err) {
			loadError = err instanceof Error ? err.message : 'This invitation is not valid.';
		}
	});

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		error = '';
		if (password !== confirm) {
			error = 'Passwords do not match';
			return;
		}
		busy = true;
		try {
			session.apply(await api.post<Session>('/auth/accept-invite', { token, password }));
			goto('/profile', { replace: true });
		} catch (err) {
			error = err instanceof Error ? err.message : 'Something went wrong';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Join · NERP</title></svelte:head>

<div class="card space-y-5 p-8">
	{#if loadError}
		<h1 class="text-xl font-semibold text-slate-900">Invitation unavailable</h1>
		<p class="text-sm text-slate-600">{loadError.charAt(0).toUpperCase() + loadError.slice(1)}</p>
		<a href="/login" class="btn btn-secondary w-full">Go to sign in</a>
	{:else if invite}
		<form class="space-y-5" onsubmit={submit}>
			<div>
				<h1 class="text-xl font-semibold text-slate-900">Join {invite.company_name}</h1>
				<p class="mt-1 text-sm text-slate-500">Welcome, {invite.first_name}! Choose a password to finish setting up your account.</p>
			</div>
			{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700">{error.charAt(0).toUpperCase() + error.slice(1)}</p>{/if}
			<div>
				<label class="label" for="email">Email</label>
				<input id="email" class="input" value={invite.email} disabled autocomplete="username" />
			</div>
			<div>
				<label class="label" for="pw">Password</label>
				<input id="pw" class="input" type="password" autocomplete="new-password" minlength="8" required bind:value={password} />
			</div>
			<div>
				<label class="label" for="pw2">Confirm password</label>
				<input id="pw2" class="input" type="password" autocomplete="new-password" minlength="8" required bind:value={confirm} />
			</div>
			<button class="btn btn-primary w-full" disabled={busy}>{busy ? 'Joining…' : 'Join and sign in'}</button>
		</form>
	{:else}
		<p class="text-center text-sm text-slate-400">Loading…</p>
	{/if}
</div>
