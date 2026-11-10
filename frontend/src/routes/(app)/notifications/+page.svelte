<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { notifications } from '#lib/notifications.svelte.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { Notification, Page } from '#lib/types.ts';

	const PAGE_SIZE = 30;
	let data = $state<Page<Notification> | null>(null);
	let unreadOnly = $state(false);
	let offset = $state(0);

	async function load() {
		try {
			data = await api.get<Page<Notification>>('/notifications' + query({ unread: unreadOnly || undefined, limit: PAGE_SIZE, offset }));
		} catch (e) {
			toasts.error(e);
		}
	}

	onMount(() => {
		load();
		// Show new arrivals and read-state changes from other tabs without a reload.
		const offNew = realtime.on('notification', () => offset === 0 && load());
		const offRead = realtime.on('notifications.read', () => load());
		return () => {
			offNew();
			offRead();
		};
	});

	async function open(n: Notification) {
		await notifications.markRead(n.id);
		if (n.link) goto(n.link);
		else load();
	}

	async function markAll() {
		await notifications.markAllRead();
		load();
	}
</script>

<svelte:head><title>Notifications · NERP</title></svelte:head>

<div class="max-w-3xl space-y-5">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Notifications</h1>
			<p class="text-sm text-slate-500">{notifications.unread} unread</p>
		</div>
		<div class="flex items-center gap-3">
			<label class="flex items-center gap-2 text-sm text-slate-600">
				<input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={unreadOnly} onchange={() => ((offset = 0), load())} />
				Unread only
			</label>
			<button class="btn btn-secondary" disabled={!notifications.unread} onclick={markAll}>Mark all as read</button>
		</div>
	</div>

	<ul class="card divide-y divide-slate-100 overflow-hidden">
		{#each data?.items ?? [] as n (n.id)}
			<li>
				<button class="flex w-full gap-4 px-5 py-4 text-left hover:bg-slate-50 {n.read_at ? '' : 'bg-brand-50/40'}" onclick={() => open(n)}>
					{#if n.actor_name}
						<Avatar name={n.actor_name} fileId={n.actor_avatar_file_id} />
					{:else}
						<span class="flex size-10 shrink-0 items-center justify-center rounded-full bg-slate-100 text-slate-500"><Icon name="bell" /></span>
					{/if}
					<span class="min-w-0 flex-1">
						<span class="block {n.read_at ? 'text-slate-700' : 'font-medium text-slate-900'}">{n.title}</span>
						{#if n.body}<span class="mt-0.5 block text-sm text-slate-500">{n.body}</span>{/if}
						<span class="mt-1 block text-xs text-slate-400" title={formatDateTime(n.created_at)}>
							{n.actor_name ? `${n.actor_name} · ` : ''}{formatRelative(n.created_at)}
						</span>
					</span>
					{#if !n.read_at}<span class="mt-2 size-2.5 shrink-0 rounded-full bg-brand-500" aria-label="Unread"></span>{/if}
				</button>
			</li>
		{:else}
			<li class="px-5 py-16 text-center text-slate-500">{data ? (unreadOnly ? 'No unread notifications.' : 'No notifications yet.') : 'Loading…'}</li>
		{/each}
	</ul>

	{#if data && data.total > PAGE_SIZE}
		<div class="flex items-center justify-between text-sm text-slate-500">
			<span>{offset + 1}–{Math.min(offset + PAGE_SIZE, data.total)} of {data.total}</span>
			<div class="flex gap-2">
				<button class="btn btn-secondary" disabled={offset === 0} onclick={() => ((offset -= PAGE_SIZE), load())}>Newer</button>
				<button class="btn btn-secondary" disabled={offset + PAGE_SIZE >= data.total} onclick={() => ((offset += PAGE_SIZE), load())}>Older</button>
			</div>
		</div>
	{/if}
</div>
