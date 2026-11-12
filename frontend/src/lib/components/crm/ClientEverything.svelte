<script lang="ts">
	// A client's whole history across modules, newest first, filtered by kind.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { itemId }: { itemId: string } = $props();
	type Ev = { at: string; kind: string; title: string; text: string | null; link: string | null; record: { id: string; type: string; title: string | null } | null };
	type Page = { items: Ev[]; next_before: string | null; kinds: { kind: string; label: string }[]; records: number };
	let items = $state<Ev[]>([]);
	let kinds = $state<Page['kinds']>([]);
	let next = $state<string | null>(null);
	let only = $state<string[]>([]);
	let loading = $state(false);

	async function load(more = false) {
		loading = true;
		try {
			const q = new URLSearchParams({ limit: '30' });
			if (only.length) q.set('kinds', only.join(','));
			if (more && next) q.set('before', next);
			const r = await api.get<Page>(`/crm/items/${itemId}/everything?${q}`);
			items = more ? [...items, ...r.items] : r.items;
			kinds = r.kinds;
			next = r.next_before;
		} catch (e) {
			toasts.error(e);
		} finally {
			loading = false;
		}
	}
	$effect(() => {
		itemId;
		untrack(() => {
			only = [];
			load();
		});
	});
	function toggle(k: string) {
		only = only.includes(k) ? only.filter((x) => x !== k) : [...only, k];
		load();
	}
	const ICON: Record<string, string> = { comment: '💬', record: '📁', activity: '📞', email: '✉️', chat: '🗨️', ticket: '🎫', money: '💰', document: '📄', booking: '📅', visit: '🌐', order: '🛒', form: '📝', marketing: '📣', task: '✔️' };
</script>

<section class="card p-4 text-sm" aria-label="Everything">
	<div class="flex items-center gap-2">
		<h2 class="font-semibold text-slate-900">Everything</h2>
		<span class="text-xs text-slate-500">across every module</span>
	</div>
	<div class="mt-2 flex flex-wrap gap-1">
		{#each kinds as k (k.kind)}
			<button class="rounded-full px-2 py-0.5 text-xs {only.includes(k.kind) ? 'bg-slate-900 text-white' : 'bg-slate-100 text-slate-700 hover:bg-slate-200'}" onclick={() => toggle(k.kind)} data-kind={k.kind}>{k.label}</button>
		{/each}
	</div>
	<ol class="mt-3 space-y-2">
		{#each items as e, i (i)}
			<li class="flex gap-2" data-ev={e.kind}>
				<span class="w-5 shrink-0 text-center" aria-hidden="true">{ICON[e.kind] ?? '•'}</span>
				<div class="min-w-0 flex-1">
					<p class="text-slate-800">{#if e.link}<a class="hover:underline" href={e.link}>{e.title}</a>{:else}{e.title}{/if}</p>
					{#if e.text}<p class="line-clamp-2 text-xs text-slate-500">{e.text}</p>{/if}
					<p class="text-xs text-slate-400">{formatDateTime(e.at)}{e.record && e.record.id !== itemId && e.record.title ? ` · ${e.record.title}` : ''}</p>
				</div>
			</li>
		{:else}
			<li class="text-slate-500">{loading ? 'Loading…' : 'Nothing yet.'}</li>
		{/each}
	</ol>
	{#if next}<button class="mt-2 text-brand-700 hover:underline" disabled={loading} onclick={() => load(true)} data-more>Show older</button>{/if}
</section>
