<script lang="ts">
	// Public: ask for a copy of one's personal data, or for it to be deleted.
	import { api } from '#lib/api.ts';

	let kind = $state<'access' | 'erasure'>('access');
	let email = $state('');
	let name = $state('');
	let message = $state('');
	let sent = $state(false);
	let error = $state('');
	let busy = $state(false);

	async function send(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			await api.post('/public/privacy-request', { kind, email, name, message });
			sent = true;
		} catch (err: any) {
			error = err?.status === 429 ? 'Too many requests from here. Please try again later.' : (err?.message ?? 'Something went wrong.');
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Your personal data</title><meta name="robots" content="noindex" /></svelte:head>

<main class="flex min-h-screen items-center justify-center bg-slate-50 p-4">
	<div class="w-full max-w-md rounded-3xl bg-white p-8 shadow-g1">
		{#if sent}
			<h1 class="text-xl font-semibold text-slate-900" data-sent>Check your email</h1>
			<p class="mt-2 text-sm text-slate-600">We sent a link to {email}. Open it within 7 days to confirm the request; we answer within 30 days of that.</p>
		{:else}
			<h1 class="text-xl font-semibold text-slate-900">Your personal data</h1>
			<p class="mt-1 text-sm text-slate-600">Ask for a copy of the personal data we hold about you, or ask us to delete it.</p>
			<form class="mt-5 space-y-3 text-sm" onsubmit={send}>
				<fieldset class="space-y-1">
					<label class="flex items-center gap-2"><input type="radio" bind:group={kind} value="access" /> Send me a copy of my data</label>
					<label class="flex items-center gap-2"><input type="radio" bind:group={kind} value="erasure" data-erasure /> Delete my data</label>
				</fieldset>
				<label class="block">Your email<input class="input mt-1" type="email" required bind:value={email} name="email" /></label>
				<label class="block">Your name<input class="input mt-1" bind:value={name} name="name" /></label>
				<label class="block">Anything we should know (optional)<textarea class="input mt-1" rows="3" bind:value={message}></textarea></label>
				{#if error}<p class="text-red-700">{error}</p>{/if}
				<button class="btn btn-primary w-full" disabled={busy}>{busy ? 'Sending…' : 'Send the request'}</button>
			</form>
		{/if}
	</div>
</main>
