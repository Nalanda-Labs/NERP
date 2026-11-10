<script lang="ts">
	import { api } from '#lib/api.ts';

	let email = $state('');
	let sent = $state(false);
	let busy = $state(false);
	let error = $state('');

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			await api.post('/auth/forgot-password', { email });
			sent = true;
		} catch (err) {
			error = err instanceof Error ? err.message : 'Something went wrong';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Forgot password · NERP</title></svelte:head>

<div class="card space-y-5 p-8">
	{#if sent}
		<div class="space-y-3 text-center">
			<div class="mx-auto flex size-12 items-center justify-center rounded-full bg-emerald-50 text-2xl">✉️</div>
			<h1 class="text-xl font-semibold text-slate-900">Check your email</h1>
			<p class="text-sm text-slate-600">
				If <strong>{email}</strong> belongs to an active account, we've sent a link to reset the password. It expires in 1 hour.
			</p>
		</div>
		<a href="/login" class="btn btn-secondary w-full">Back to sign in</a>
	{:else}
		<form class="space-y-5" onsubmit={submit}>
			<div>
				<h1 class="text-xl font-semibold text-slate-900">Forgot your password?</h1>
				<p class="mt-1 text-sm text-slate-500">Enter your work email and we'll send you a reset link.</p>
			</div>
			{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700">{error}</p>{/if}
			<div>
				<label class="label" for="email">Email</label>
				<input id="email" class="input" type="email" autocomplete="username" required bind:value={email} />
			</div>
			<button class="btn btn-primary w-full" disabled={busy}>{busy ? 'Sending…' : 'Send reset link'}</button>
			<p class="text-center text-sm"><a href="/login" class="text-slate-500 hover:underline">Back to sign in</a></p>
		</form>
	{/if}
</div>
