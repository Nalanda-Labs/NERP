<script lang="ts">
	// Guests from outside the company, and the chats and workgroups they are in. They are
	// invited from those chats and groups; deactivate one from their row here.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Guest = {
		id: string;
		name: string;
		email: string;
		active: boolean;
		invite_pending: boolean;
		chats: string | null;
		groups: string | null;
	};
	let items = $state<Guest[] | null>(null);
	async function load() {
		items = (await api.get<{ items: Guest[] }>('/guests')).items;
	}
	onMount(() => load().catch(toasts.error));
	async function deactivate(g: Guest) {
		if (!confirm(`Stop ${g.name} signing in?`)) return;
		try {
			await api.patch(`/users/${g.id}`, { is_active: false });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Guests · NERP</title></svelte:head>

<div class="max-w-4xl space-y-4">
	<div class="flex items-center gap-2">
		<a class="btn btn-ghost px-2" href="/employees" aria-label="Employees">‹</a>
		<h1 class="text-2xl font-semibold text-slate-900">Guests</h1>
	</div>
	<p class="text-sm text-slate-500">
		People from outside the company. Each sees only the chats and workgroups they were invited to
		(and the tasks, files, calls and boards there). Invite them from a chat's or a group's members.
	</p>
	<section class="card divide-y divide-slate-50 text-sm">
		{#each items ?? [] as g (g.id)}
			<div
				class="flex flex-wrap items-center gap-3 px-5 py-3 {g.active ? '' : 'text-slate-400'}"
				data-guest={g.email}
			>
				<div class="min-w-0 flex-1">
					<p class="font-medium">
						{g.name}
						<span class="font-normal text-slate-500">{g.email}</span>{#if g.invite_pending}<span
								class="badge ml-1 bg-slate-100 text-slate-600">Invited</span
							>{/if}{#if !g.active}<span class="badge ml-1 bg-slate-100 text-slate-600"
								>Deactivated</span
							>{/if}
					</p>
					<p class="text-xs text-slate-500">
						{[g.chats && `Chats: ${g.chats}`, g.groups && `Groups: ${g.groups}`]
							.filter(Boolean)
							.join(' · ') || 'In nothing yet'}
					</p>
				</div>
				{#if g.active}<button
						class="text-slate-500 hover:text-red-600"
						onclick={() => deactivate(g)}>Deactivate</button
					>{/if}
			</div>
		{:else}
			<p class="px-5 py-4 text-slate-400">{items ? 'No guests.' : 'Loading…'}</p>
		{/each}
	</section>
</div>
