<script lang="ts">
	// A link to join the company: name and work email; an invitation to set a password follows
	// by email (at once, or once someone approves the request).
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';

	type Info = { company_name: string; needs_approval: boolean; email_domains: string[] };
	const token = $derived(page.params.token ?? '');
	let info = $state<Info | null>(null);
	let missing = $state(false);
	let form = $state({ email: '', first_name: '', last_name: '' });
	let busy = $state(false);
	let error = $state('');
	let done = $state<{ needs_approval: boolean } | null>(null);

	onMount(async () => {
		try {
			info = await api.get<Info>(`/public/join/${encodeURIComponent(token)}`);
		} catch {
			missing = true;
		}
	});
	async function submit(e: SubmitEvent) {
		e.preventDefault();
		error = '';
		busy = true;
		try {
			done = await api.post<{ needs_approval: boolean }>(
				`/public/join/${encodeURIComponent(token)}`,
				form
			);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Something went wrong';
		} finally {
			busy = false;
		}
	}
	const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
</script>

<svelte:head><title>Join · NERP</title><meta name="robots" content="noindex" /></svelte:head>

<div class="card space-y-5 p-8">
	{#if missing}
		<h1 class="text-xl font-semibold text-slate-900">This link doesn't work</h1>
		<p class="text-sm text-slate-600">
			It may have expired, been used up or been turned off. Ask whoever sent it for a new one.
		</p>
		<a href="/login" class="btn btn-secondary w-full">Go to sign in</a>
	{:else if done}
		<h1 class="text-xl font-semibold text-slate-900" data-join-done>Check your email</h1>
		<p class="text-sm text-slate-600">
			{done.needs_approval
				? `Your request was sent. Once someone at ${info?.company_name} approves it, you'll get an email to set your password.`
				: `We sent an email to ${form.email}. Open it to set your password and sign in.`}
		</p>
	{:else if info}
		<form class="space-y-4" onsubmit={submit}>
			<div>
				<h1 class="text-xl font-semibold text-slate-900">Join {info.company_name}</h1>
				<p class="mt-1 text-sm text-slate-500">
					{info.needs_approval
						? 'Someone will approve your request, then you get an email to set your password.'
						: "You'll get an email to set your password."}
				</p>
			</div>
			{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">
					{cap(error)}
				</p>{/if}
			<div>
				<label class="label" for="j-email">Work email</label>
				<input
					id="j-email"
					class="input"
					type="email"
					required
					autocomplete="email"
					placeholder={info.email_domains.length ? `you@${info.email_domains[0]}` : ''}
					bind:value={form.email}
				/>
			</div>
			<div class="grid grid-cols-2 gap-3">
				<div>
					<label class="label" for="j-first">First name</label><input
						id="j-first"
						class="input"
						required
						maxlength="100"
						autocomplete="given-name"
						bind:value={form.first_name}
					/>
				</div>
				<div>
					<label class="label" for="j-last">Last name</label><input
						id="j-last"
						class="input"
						required
						maxlength="100"
						autocomplete="family-name"
						bind:value={form.last_name}
					/>
				</div>
			</div>
			<button class="btn btn-primary w-full" disabled={busy}
				>{busy ? 'Sending…' : info.needs_approval ? 'Ask to join' : 'Join'}</button
			>
		</form>
	{:else}
		<p class="text-center text-sm text-slate-400">Loading…</p>
	{/if}
</div>
