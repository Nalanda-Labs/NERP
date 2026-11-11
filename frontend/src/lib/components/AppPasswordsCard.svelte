<script lang="ts">
	// App passwords: for calendar apps (CalDAV) that can't sign in through the browser.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type AppPassword = { id: string; name: string; created_at: string; last_used_at: string | null; last_used_ip: string | null };
	let items = $state<AppPassword[] | null>(null);
	let name = $state('');
	let made = $state<{ name: string; password: string } | null>(null);
	let busy = $state(false);
	const server = typeof location !== 'undefined' ? `${location.origin}/dav/` : '/dav/';

	async function load() {
		try {
			items = (await api.get<{ items: AppPassword[] }>('/auth/app-passwords')).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	async function create(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			made = await api.post('/auth/app-passwords', { name });
			name = '';
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function revoke(p: AppPassword) {
		if (!confirm(`Revoke “${p.name}”? Apps using it stop syncing.`)) return;
		try {
			await api.del(`/auth/app-passwords/${p.id}`);
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<section class="card" data-app-passwords>
	<div class="border-b border-slate-100 px-6 py-3.5">
		<h2 class="font-semibold text-slate-900">App passwords</h2>
		<p class="text-sm text-slate-500">For calendar and contacts apps on your phone or computer (Apple Calendar and Contacts, Thunderbird, DAVx5 on Android…): your calendar, and the company's employee list as an address book. They only work for those apps, not for signing in here.</p>
	</div>
	<div class="space-y-3 px-6 py-4 text-sm">
		{#if made}
			<div class="space-y-2 rounded-xl bg-emerald-50 p-4" data-new-password>
				<p class="font-medium text-emerald-900">“{made.name}” is ready. Enter these in the app (the password is shown only now):</p>
				<dl class="grid grid-cols-[7rem_1fr] gap-x-3 gap-y-1 text-emerald-900">
					<dt>Server</dt><dd><code class="break-all">{server}</code></dd>
					<dt>User name</dt><dd><code class="break-all">{session.user?.email}</code></dd>
					<dt>Password</dt><dd><code class="text-base font-semibold tracking-wide" data-password>{made.password}</code></dd>
				</dl>
				<p class="text-xs text-emerald-800">In most apps choose “CalDAV account” for the calendar and “CardDAV account” for the employee list, with the same server and password (on iPhone: Settings → Calendar or Contacts → Accounts → Add Account → Other).</p>
				<button class="btn btn-secondary" onclick={() => (made = null)}>Done</button>
			</div>
		{/if}
		<form class="flex gap-2" onsubmit={create}>
			<input class="input" aria-label="App name" placeholder="Name, e.g. My phone" maxlength="100" required bind:value={name} />
			<button class="btn btn-primary whitespace-nowrap" disabled={busy}>Make a password</button>
		</form>
		{#if items?.length}
			<ul class="divide-y divide-slate-100 rounded-xl border border-slate-200">
				{#each items as p (p.id)}
					<li class="flex items-center gap-3 px-4 py-2.5">
						<div class="min-w-0 flex-1">
							<p class="font-medium text-slate-900">{p.name}</p>
							<p class="text-xs text-slate-500" title="Made {formatDateTime(p.created_at)}">{p.last_used_at ? `Last used ${formatRelative(p.last_used_at)}${p.last_used_ip ? ` from ${p.last_used_ip}` : ''}` : 'Not used yet'}</p>
						</div>
						<button class="text-sm font-medium text-red-600 hover:underline" onclick={() => revoke(p)}>Revoke</button>
					</li>
				{/each}
			</ul>
		{/if}
	</div>
</section>
