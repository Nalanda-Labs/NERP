<script lang="ts">
	// Call history: my calls, who was on them, how long they lasted, which I missed.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Call = {
		id: string;
		chat_id: string | null;
		chat_title: string | null;
		started_by: string | null;
		started_by_name: string | null;
		started_at: string;
		ended_at: string | null;
		seconds: number | null;
		outgoing: boolean;
		joined: boolean;
		missed: boolean;
		people: string[];
		recorded: boolean;
	};
	let calls = $state<Call[]>([]);
	let missedOnly = $state(false);
	let more = $state(false);
	const PAGE = 50;

	async function load(append = false) {
		const before = append ? calls.at(-1)?.started_at : undefined;
		const r = await api.get<{ items: Call[] }>('/calls' + query({ missed: missedOnly || undefined, before, limit: PAGE }));
		calls = append ? [...calls, ...r.items] : r.items;
		more = r.items.length === PAGE;
	}
	onMount(() => load());

	const length = (s: number | null) => (s == null ? 'going on' : s < 60 ? `${s} s` : s < 3600 ? `${Math.floor(s / 60)} min ${s % 60} s` : `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} min`);
	async function callBack(c: Call) {
		try {
			if (c.chat_id) {
				const k = await api.post<{ id: string }>('/calls', { chat_id: c.chat_id });
				goto(`/call/${k.id}`);
			}
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Calls · NERP</title></svelte:head>

<div class="mx-auto max-w-4xl space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<a href="/messenger" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Messenger</a>
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Calls</h1>
		<label class="flex items-center gap-2 text-sm text-slate-600"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={missedOnly} onchange={() => load()} /> Missed only</label>
	</div>
	<div class="card">
		<ul class="divide-y divide-slate-100">
			{#each calls as c (c.id)}
				<li class="flex items-center gap-4 px-4 py-3" data-call={c.id} data-missed={c.missed}>
					<span class="text-lg" title={c.missed ? 'Missed' : c.outgoing ? 'Outgoing' : 'Incoming'}>{c.missed ? '↙︎' : c.outgoing ? '↗︎' : '↙︎'}</span>
					<div class="min-w-0 flex-1">
						<p class="truncate text-sm font-medium {c.missed ? 'text-red-600' : 'text-slate-900'}">{c.chat_title ?? c.people.join(', ')}</p>
						<p class="truncate text-xs text-slate-500">
							{c.outgoing ? 'You called' : `${c.started_by_name ?? 'Someone'} called`}{c.chat_title && c.people.length ? ` · ${c.people.length} ${c.people.length === 1 ? 'other' : 'others'}` : ''} · {formatDateTime(c.started_at)} · {c.missed ? 'missed' : length(c.seconds)}{c.recorded ? ' · recorded' : ''}
						</p>
					</div>
					{#if c.chat_id}<a class="btn btn-ghost px-2 py-1 text-sm" href="/messenger?chat={c.chat_id}">Chat</a>{/if}
					{#if c.chat_id}<button class="btn btn-secondary px-3 py-1 text-sm" onclick={() => callBack(c)}>📹 Call</button>{/if}
				</li>
			{:else}
				<li class="px-4 py-10 text-center text-sm text-slate-500">{missedOnly ? 'No missed calls.' : 'No calls yet.'}</li>
			{/each}
		</ul>
		{#if more}<div class="border-t border-slate-100 p-3 text-center"><button class="text-sm text-brand-600 hover:underline" onclick={() => load(true)}>Earlier calls</button></div>{/if}
	</div>
</div>
