<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { describeDevice, formatDateTime, formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { SessionInfo } from '#lib/types.ts';
	import Icon from './Icon.svelte';

	/** Omit to manage your own sessions; pass a user id for the admin view. */
	let { userId = null }: { userId?: string | null } = $props();

	let sessions = $state<SessionInfo[] | null>(null);
	const base = $derived(userId ? `/users/${userId}/sessions` : '/auth/sessions');

	async function load() {
		try {
			sessions = await api.get<SessionInfo[]>(base);
		} catch (err) {
			toasts.error(err);
		}
	}
	onMount(load);

	async function revoke(s: SessionInfo) {
		try {
			await api.del(`/auth/sessions/${s.id}`);
			toasts.success('Device signed out');
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}

	async function revokeAll() {
		const msg = userId ? 'Sign this employee out on every device?' : 'Sign out on all your other devices?';
		if (!confirm(msg)) return;
		try {
			await api.post(userId ? `/users/${userId}/sessions/revoke` : '/auth/sessions/revoke-others');
			toasts.success('Signed out');
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}

	const isMobile = (ua: string | null) => !!ua && /iPhone|iPad|Android|Mobile/.test(ua);
	const others = $derived((sessions ?? []).filter((s) => !s.current).length);
</script>

<section class="card">
	<div class="flex items-center justify-between border-b border-slate-100 px-6 py-3.5">
		<h2 class="font-semibold text-slate-900">{userId ? 'Signed-in devices' : 'Where you are signed in'}</h2>
		{#if (userId ? (sessions?.length ?? 0) : others) > 0}
			<button class="text-sm font-medium text-red-600 hover:underline" onclick={revokeAll}>
				{userId ? 'Sign out everywhere' : 'Sign out other devices'}
			</button>
		{/if}
	</div>
	<ul class="divide-y divide-slate-100">
		{#each sessions ?? [] as s (s.id)}
			<li class="flex items-center gap-4 px-6 py-3.5">
				<span class="flex size-10 shrink-0 items-center justify-center rounded-lg bg-slate-100 text-slate-500">
					<Icon name={isMobile(s.user_agent) ? 'smartphone' : 'monitor'} />
				</span>
				<div class="min-w-0 flex-1 text-sm">
					<p class="font-medium text-slate-900">
						{describeDevice(s.user_agent)}
						{#if s.current}<span class="badge ml-1 bg-emerald-50 text-emerald-700">This device</span>{/if}
					</p>
					<p class="truncate text-slate-500" title="Signed in {formatDateTime(s.created_at)}">
						{s.ip ?? 'Unknown IP'} · active {formatRelative(s.last_used_at)}
					</p>
				</div>
				{#if !s.current && !userId}
					<button class="btn btn-secondary" onclick={() => revoke(s)}>Sign out</button>
				{/if}
			</li>
		{:else}
			<li class="px-6 py-6 text-sm text-slate-500">{sessions ? 'Not signed in anywhere.' : 'Loading…'}</li>
		{/each}
	</ul>
</section>
