<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import type { Session } from '#lib/types.ts';

	let form = $state({ company_name: '', first_name: '', last_name: '', email: '', password: '' });
	let confirm = $state('');
	let error = $state('');
	let busy = $state(false);

	onMount(async () => {
		const status = await api.get<{ needs_setup: boolean }>('/setup');
		if (!status.needs_setup) goto('/login', { replace: true });
	});

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		error = '';
		if (form.password !== confirm) {
			error = 'Passwords do not match';
			return;
		}
		busy = true;
		try {
			session.apply(await api.post<Session>('/setup', form));
			goto('/', { replace: true });
		} catch (err) {
			error = err instanceof Error ? err.message : 'Setup failed';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Set up · NERP</title></svelte:head>

<form class="card space-y-5 p-8" onsubmit={submit}>
	<div>
		<h1 class="text-xl font-semibold text-slate-900">Welcome to NERP</h1>
		<p class="mt-1 text-sm text-slate-500">Create your company and the first administrator account.</p>
	</div>
	{#if error}
		<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700">{error.charAt(0).toUpperCase() + error.slice(1)}</p>
	{/if}
	<div>
		<label class="label" for="company">Company name</label>
		<input id="company" class="input" required bind:value={form.company_name} />
	</div>
	<div class="grid grid-cols-2 gap-3">
		<div>
			<label class="label" for="first">First name</label>
			<input id="first" class="input" autocomplete="given-name" required bind:value={form.first_name} />
		</div>
		<div>
			<label class="label" for="last">Last name</label>
			<input id="last" class="input" autocomplete="family-name" required bind:value={form.last_name} />
		</div>
	</div>
	<div>
		<label class="label" for="email">Email</label>
		<input id="email" class="input" type="email" autocomplete="username" required bind:value={form.email} />
	</div>
	<div class="grid grid-cols-2 gap-3">
		<div>
			<label class="label" for="pw">Password</label>
			<input id="pw" class="input" type="password" autocomplete="new-password" minlength="8" required bind:value={form.password} />
		</div>
		<div>
			<label class="label" for="pw2">Confirm</label>
			<input id="pw2" class="input" type="password" autocomplete="new-password" minlength="8" required bind:value={confirm} />
		</div>
	</div>
	<button class="btn btn-primary w-full" disabled={busy}>{busy ? 'Creating…' : 'Create company'}</button>
</form>
