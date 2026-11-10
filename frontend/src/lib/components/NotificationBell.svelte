<script lang="ts">
	import { goto } from '$app/navigation';
	import { formatRelative } from '#lib/format.ts';
	import { notifications } from '#lib/notifications.svelte.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import type { Notification } from '#lib/types.ts';
	import Avatar from './Avatar.svelte';
	import Icon from './Icon.svelte';

	let open = $state(false);
	let root: HTMLDivElement | undefined = $state();

	async function openItem(n: Notification) {
		open = false;
		await notifications.markRead(n.id);
		if (n.link) goto(n.link);
	}

	function onWindowClick(e: MouseEvent) {
		if (open && root && !root.contains(e.target as Node)) open = false;
	}
</script>

<svelte:window onclick={onWindowClick} onkeydown={(e) => e.key === 'Escape' && (open = false)} />

<div class="relative" bind:this={root}>
	<button
		class="relative rounded-lg p-2 text-slate-500 hover:bg-slate-100 hover:text-slate-800"
		aria-label="Notifications{notifications.unread ? ` (${notifications.unread} unread)` : ''}"
		onclick={() => (open = !open)}
	>
		<Icon name="bell" />
		{#if notifications.unread}
			<span class="absolute top-1 right-1 flex min-w-4 items-center justify-center rounded-full bg-red-500 px-1 text-[10px] leading-4 font-semibold text-white">
				{notifications.unread > 99 ? '99+' : notifications.unread}
			</span>
		{/if}
		<span
			class="absolute right-1.5 bottom-1.5 size-1.5 rounded-full {realtime.connected ? 'bg-emerald-500' : 'bg-slate-300'}"
			title={realtime.connected ? 'Live updates on' : 'Reconnecting…'}
		></span>
	</button>

	{#if open}
		<div class="absolute right-0 z-30 mt-2 w-[22rem] max-w-[calc(100vw-2rem)] overflow-hidden rounded-2xl bg-white shadow-g2">
			<div class="flex items-center justify-between border-b border-slate-100 px-4 py-3">
				<h3 class="text-sm font-semibold text-slate-900">Notifications</h3>
				{#if notifications.unread}
					<button class="text-xs font-medium text-brand-600 hover:underline" onclick={() => notifications.markAllRead()}>Mark all as read</button>
				{/if}
			</div>
			<ul class="max-h-[60vh] divide-y divide-slate-100 overflow-y-auto">
				{#each notifications.recent.slice(0, 10) as n (n.id)}
					<li>
						<button class="flex w-full gap-3 px-4 py-3 text-left hover:bg-slate-50 {n.read_at ? '' : 'bg-brand-50/50'}" onclick={() => openItem(n)}>
							{#if n.actor_name}
								<Avatar name={n.actor_name} fileId={n.actor_avatar_file_id} size="sm" />
							{:else}
								<span class="flex size-8 shrink-0 items-center justify-center rounded-full bg-slate-100 text-slate-500"><Icon name="bell" class="size-4" /></span>
							{/if}
							<span class="min-w-0 flex-1">
								<span class="block text-sm {n.read_at ? 'text-slate-700' : 'font-medium text-slate-900'}">{n.title}</span>
								{#if n.body}<span class="mt-0.5 block text-xs text-slate-500">{n.body}</span>{/if}
								<span class="mt-1 block text-xs text-slate-400">{formatRelative(n.created_at)}</span>
							</span>
							{#if !n.read_at}<span class="mt-1.5 size-2 shrink-0 rounded-full bg-brand-500"></span>{/if}
						</button>
					</li>
				{:else}
					<li class="px-4 py-10 text-center text-sm text-slate-500">You're all caught up.</li>
				{/each}
			</ul>
			<a href="/notifications" class="block border-t border-slate-100 px-4 py-2.5 text-center text-sm font-medium text-slate-600 hover:bg-slate-50" onclick={() => (open = false)}>
				View all notifications
			</a>
		</div>
	{/if}
</div>
