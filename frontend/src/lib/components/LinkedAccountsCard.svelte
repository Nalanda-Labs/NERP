<script lang="ts">
	// Profile: outside accounts (Google, Apple, …) one signs in with; link more or unlink.
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import { formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Linked = {
		provider_id: string;
		kind: string;
		label: string;
		email: string | null;
		created_at: string;
		last_used_at: string | null;
	};
	let linked = $state<Linked[]>([]);
	let providers = $state<{ id: string; kind: string; label: string }[]>([]);

	onMount(async () => {
		const p = page.url.searchParams;
		if (p.get('linked')) toasts.success('Account linked');
		if (p.get('link_error')) toasts.error(new Error(p.get('link_error')!));
		if (p.get('linked') || p.get('link_error'))
			history.replaceState(history.state, '', location.pathname);
		[linked, providers] = await Promise.all([
			api.get<{ items: Linked[] }>('/auth/identities').then((r) => r.items),
			api.get<{ items: typeof providers }>('/auth/providers').then((r) => r.items)
		]).catch(() => [[], []] as [Linked[], typeof providers]);
	});
	async function link(id: string) {
		try {
			location.href = (await api.post<{ url: string }>(`/auth/providers/${id}/link`)).url;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function unlink(l: Linked) {
		if (!confirm(`Stop signing in with ${l.label}?`)) return;
		linked = (await api.del<{ items: Linked[] }>(`/auth/identities/${l.provider_id}`)).items;
	}
	const unlinked = $derived(providers.filter((p) => !linked.some((l) => l.provider_id === p.id)));
</script>

{#if providers.length || linked.length}
	<section class="card" aria-label="Linked accounts">
		<h2 class="border-b border-slate-100 px-6 py-3.5 font-semibold text-slate-900">
			Accounts you sign in with
		</h2>
		<div class="space-y-2 px-6 py-4 text-sm">
			{#each linked as l (l.provider_id)}
				<div class="flex items-center gap-2" data-linked={l.kind}>
					<span class="flex-1"
						><span class="font-medium">{l.label.replace(/^Sign in with /, '')}</span
						>{#if l.email}<span class="text-slate-500"> · {l.email}</span>{/if}<span
							class="text-xs text-slate-400"
						>
							· {l.last_used_at
								? `used ${formatRelative(l.last_used_at)}`
								: `linked ${formatRelative(l.created_at)}`}</span
						></span
					>
					<button class="text-slate-500 hover:text-red-600" onclick={() => unlink(l)}>Unlink</button
					>
				</div>
			{:else}<p class="text-slate-500">
					None yet. Accounts with the same verified email are linked the first time you use them.
				</p>{/each}
			{#if unlinked.length}
				<div class="flex flex-wrap gap-2 pt-1">
					{#each unlinked as p (p.id)}<button class="btn btn-secondary" onclick={() => link(p.id)}
							>Link {p.label.replace(/^Sign in with /, '')}</button
						>{/each}
				</div>
			{/if}
		</div>
	</section>
{/if}
