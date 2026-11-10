<script lang="ts">
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';

	const token = $derived(page.url.searchParams.get('token') ?? '');
	let password = $state('');
	let confirm = $state('');
	let busy = $state(false);
	let error = $state('');
	let done = $state(false);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		error = '';
		if (password !== confirm) {
			error = 'Passwords do not match';
			return;
		}
		busy = true;
		try {
			await api.post('/auth/reset-password', { token, password });
			done = true;
		} catch (err) {
			error = err instanceof Error ? err.message : 'Something went wrong';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Reset password · NERP</title></svelte:head>

<div class="card space-y-5 p-8">
	{#if done}
		<div class="space-y-3 text-center">
			<h1 class="text-xl font-semibold text-slate-900">Password changed</h1>
			<p class="text-sm text-slate-600">You've been signed out on all devices. Sign in with your new password.</p>
		</div>
		<a href="/login" class="btn btn-primary w-full">Sign in</a>
	{:else if !token}
		<h1 class="text-xl font-semibold text-slate-900">Invalid link</h1>
		<p class="text-sm text-slate-600">This reset link is incomplete. Request a new one.</p>
		<a href="/forgot-password" class="btn btn-secondary w-full">Request a new link</a>
	{:else}
		<form class="space-y-5" onsubmit={submit}>
			<div>
				<h1 class="text-xl font-semibold text-slate-900">Choose a new password</h1>
				<p class="mt-1 text-sm text-slate-500">At least 8 characters.</p>
			</div>
			{#if error}
				<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700">
					{error.charAt(0).toUpperCase() + error.slice(1)}
					{#if /expired|invalid/.test(error)}<a href="/forgot-password" class="font-medium underline">Request a new link</a>{/if}
				</p>
			{/if}
			<div>
				<label class="label" for="pw">New password</label>
				<input id="pw" class="input" type="password" autocomplete="new-password" minlength="8" required bind:value={password} />
			</div>
			<div>
				<label class="label" for="pw2">Confirm new password</label>
				<input id="pw2" class="input" type="password" autocomplete="new-password" minlength="8" required bind:value={confirm} />
			</div>
			<button class="btn btn-primary w-full" disabled={busy}>{busy ? 'Saving…' : 'Set new password'}</button>
		</form>
	{/if}
</div>
