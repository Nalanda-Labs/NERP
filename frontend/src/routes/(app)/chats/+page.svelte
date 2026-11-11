<script lang="ts">
	// Contact centre → Chats: website chats waiting, mine, all open, closed; answer them here.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount, tick } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session as me } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Session = {
		id: string;
		channel_name: string;
		visitor_name: string | null;
		visitor_email: string | null;
		page_url: string | null;
		status: 'open' | 'closed';
		assigned_to: string | null;
		assigned_name: string | null;
		item_id: string | null;
		item_code: string | null;
		item_title: string | null;
		unread: number;
		last_message: string | null;
		last_message_at: string;
		closed_at: string | null;
		channel_id: string;
		offline: boolean;
		rating: number | null;
		rating_comment: string | null;
		review_score: number | null;
		review_comment: string | null;
	};
	type Canned = { id: string; channel_id: string | null; shortcut: string; title: string; body: string };
	type Channel = { id: string; name: string; operators: string[] };
	type Msg = { id: string; author: 'visitor' | 'operator' | 'system'; user_name: string | null; body: string; created_at: string };

	let tab = $state<'waiting' | 'mine' | 'open' | 'closed'>('open');
	let items = $state<Session[]>([]);
	let current = $state<Session | null>(null);
	let messages = $state<Msg[]>([]);
	let draft = $state('');
	let busy = $state(false);
	let list: HTMLElement | undefined = $state();
	const selected = $derived(page.url.searchParams.get('session'));
	let canned = $state<Canned[]>([]);
	let channels = $state<Channel[]>([]);
	let people = $state<{ id: string; name: string }[]>([]);
	let canManage = $state(false);
	// Saved replies for this chat's channel matching what's typed after "/".
	const suggestions = $derived(
		draft.startsWith('/') && current
			? canned.filter((c) => (!c.channel_id || c.channel_id === current!.channel_id) && c.shortcut.startsWith(draft.slice(1).trim().toLowerCase())).slice(0, 6)
			: []
	);
	function useCanned(c: Canned) {
		draft = c.body;
	}

	async function load() {
		const r = await api.get<{ items: Session[] }>(
			'/chat/sessions' + query({ status: tab === 'closed' ? 'closed' : 'open', who: tab === 'mine' ? 'mine' : tab === 'waiting' ? 'waiting' : undefined })
		);
		items = r.items;
	}
	async function open(id: string | null) {
		if (!id) {
			current = null;
			return;
		}
		try {
			const r = await api.get<{ session: Session; messages: Msg[] }>(`/chat/sessions/${id}`);
			current = r.session;
			messages = r.messages;
			await tick();
			list?.scrollTo({ top: list.scrollHeight });
			if (r.session.unread && r.session.status === 'open') {
				await api.post(`/chat/sessions/${id}/read`, {});
				load();
			}
		} catch {
			current = null;
		}
	}
	onMount(() => {
		load();
		api.get<{ items: Canned[] }>('/chat/canned').then((r) => (canned = r.items)).catch(() => {});
		api.get<{ items: Channel[]; people: { id: string; name: string }[]; can_manage: boolean }>('/chat/channels').then((r) => ((channels = r.items), (people = r.people), (canManage = r.can_manage))).catch(() => {});
		const off = [
			realtime.on('chat.message', (d: { session_id: string }) => {
				load();
				if (d.session_id === current?.id) open(d.session_id);
			}),
			realtime.on('chat.changed', (d: { session_id: string }) => {
				load();
				if (d.session_id === current?.id) open(d.session_id);
			})
		];
		return () => off.forEach((f) => f());
	});
	$effect(() => {
		open(selected);
	});
	function pick(id: string) {
		goto(`/chats?session=${id}`, { replace: true });
	}
	async function act(path: string, body: object = {}) {
		if (!current) return;
		busy = true;
		try {
			const r = await api.post<{ session: Session; messages: Msg[] }>(`/chat/sessions/${current.id}/${path}`, body);
			current = r.session;
			messages = r.messages;
			await tick();
			list?.scrollTo({ top: list.scrollHeight });
			load();
			return true;
		} catch (e) {
			toasts.error(e);
			return false;
		} finally {
			busy = false;
		}
	}
	async function send(e: SubmitEvent) {
		e.preventDefault();
		if (!draft.trim()) return;
		if (await act('messages', { body: draft })) draft = '';
	}
	function keydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			(e.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
		}
	}
	// Passing a chat on.
	let passOpen = $state(false);
	let passTo = $state({ channel: '', user: '' });
	const passOps = $derived((channels.find((c) => c.id === (passTo.channel || current?.channel_id))?.operators ?? []).filter((u) => u !== me.user?.id));
	async function pass(e: SubmitEvent) {
		e.preventDefault();
		if (!current) return;
		try {
			const body: Record<string, string> = {};
			if (passTo.channel && passTo.channel !== current.channel_id) body.channel_id = passTo.channel;
			if (passTo.user) body.user_id = passTo.user;
			// Same channel, nobody named: the next operator there.
			if (!body.user_id && !body.channel_id) body.channel_id = current.channel_id;
			await api.post(`/chat/sessions/${current.id}/transfer`, body);
			passOpen = false;
			toasts.success('Passed on');
			await open(current.id);
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	// A supervisor's review.
	let review = $state({ score: 0, comment: '' });
	async function saveReview() {
		if (!current || !review.score) return;
		try {
			await api.post(`/chat/sessions/${current.id}/review`, { score: review.score, comment: review.comment || null });
			toasts.success('Review saved');
			await open(current.id);
		} catch (err) {
			toasts.error(err);
		}
	}
	const who = (s: Session) => s.visitor_name ?? s.visitor_email ?? 'Website visitor';
	const TABS = [
		['waiting', 'Waiting'],
		['mine', 'Mine'],
		['open', 'All open'],
		['closed', 'Closed']
	] as const;
</script>

<svelte:head><title>Chats · NERP</title></svelte:head>

<div class="flex h-[calc(100dvh-8rem)] min-h-[32rem] gap-4">
	<section class="card flex w-80 shrink-0 flex-col" aria-label="Chats">
		<div class="flex items-center justify-between px-4 pt-4">
			<h1 class="text-lg font-semibold text-slate-900">Chats</h1>
			<span class="flex gap-3">{#if canManage}<a href="/chats/stats" class="text-sm text-brand-600 hover:underline">Figures</a>{/if}<a href="/chats/channels" class="text-sm text-brand-600 hover:underline">Chat buttons</a></span>
		</div>
		<div class="flex gap-1 px-3 py-3" role="tablist">
			{#each TABS as [k, l] (k)}
				<button role="tab" aria-selected={tab === k} class="rounded-full px-3 py-1 text-sm font-medium {tab === k ? 'bg-slate-900 text-white' : 'text-slate-600 hover:bg-slate-100'}" onclick={() => ((tab = k), load())}>{l}</button>
			{/each}
		</div>
		<ul class="flex-1 divide-y divide-slate-50 overflow-y-auto">
			{#each items as s (s.id)}
				<li>
					<button class="block w-full px-4 py-3 text-left hover:bg-slate-50 {current?.id === s.id ? 'bg-brand-50' : ''}" onclick={() => pick(s.id)} data-chat={who(s)}>
						<span class="flex items-center gap-2">
							<span class="min-w-0 flex-1 truncate font-medium text-slate-900">{who(s)}</span>
							{#if s.unread}<span class="rounded-full bg-red-600 px-1.5 text-xs font-semibold text-white" data-unread>{s.unread}</span>{/if}
							<span class="text-xs text-slate-400">{formatRelative(s.last_message_at)}</span>
						</span>
						<span class="block truncate text-sm text-slate-500">{s.last_message ?? ''}</span>
						<span class="block text-xs text-slate-400">{s.channel_name}{s.assigned_name ? ` · ${s.assigned_name}` : ' · nobody yet'}</span>
					</button>
				</li>
			{:else}
				<li class="px-4 py-10 text-center text-sm text-slate-500">{tab === 'closed' ? 'No closed chats.' : 'No chats waiting.'}</li>
			{/each}
		</ul>
	</section>

	<section class="card flex min-w-0 flex-1 flex-col" aria-label="Conversation">
		{#if current}
			<div class="flex flex-wrap items-center gap-3 border-b border-slate-100 px-5 py-3">
				<div class="min-w-0 flex-1">
					<h2 class="font-semibold text-slate-900">{who(current)}</h2>
					<p class="truncate text-sm text-slate-500">
						{current.visitor_email ?? 'no email'} · {current.channel_name}
						{#if current.page_url}· <a class="hover:underline" href={current.page_url} target="_blank" rel="noopener noreferrer">{current.page_url}</a>{/if}
					</p>
				</div>
				{#if current.item_id}<a class="btn btn-secondary" href="/crm/{current.item_code}/{current.item_id}" data-crm-link><Icon name="user" class="size-4" /> {current.item_title}</a>{/if}
				{#if current.status === 'open'}
					{#if current.assigned_to !== me.user?.id}<button class="btn btn-secondary" disabled={busy} onclick={() => act('take')}>Take</button>{/if}
					<button class="btn btn-secondary" disabled={busy} onclick={() => ((passTo = { channel: current!.channel_id, user: '' }), (passOpen = true))}>Pass on</button>
					<button class="btn btn-secondary" disabled={busy} onclick={() => confirm('End this chat? Its transcript goes on the CRM record.') && act('close')}>End chat</button>
				{:else}
					<span class="badge bg-slate-100 text-slate-600">Closed {current.closed_at ? formatDateTime(current.closed_at) : ''}</span>
				{/if}
			</div>
			{#if current.rating || current.review_score || (canManage && current.status === 'closed')}
				<div class="flex flex-wrap items-center gap-4 border-b border-slate-100 px-5 py-2 text-sm" data-marks>
					{#if current.rating}<span>Visitor: <span class="text-amber-500">{'★'.repeat(current.rating)}</span>{current.rating_comment ? ` “${current.rating_comment}”` : ''}</span>{/if}
					{#if current.review_score}<span>Review: {current.review_score}/5{current.review_comment ? ` · ${current.review_comment}` : ''}</span>{/if}
					{#if canManage && !current.review_score}
						<span class="flex items-center gap-2">Review:
							<select class="input w-20 py-0.5" aria-label="Review score" bind:value={review.score}><option value={0}>–</option>{#each [1, 2, 3, 4, 5] as n (n)}<option value={n}>{n}</option>{/each}</select>
							<input class="input w-48 py-0.5" aria-label="Review comment" placeholder="Comment" bind:value={review.comment} />
							<button class="btn btn-secondary py-0.5" disabled={!review.score} onclick={saveReview}>Save</button>
						</span>
					{/if}
				</div>
			{/if}
			{#if current.offline}<p class="border-b border-amber-100 bg-amber-50 px-5 py-1.5 text-xs text-amber-900">Started outside working hours.</p>{/if}
			<div bind:this={list} class="flex-1 space-y-2 overflow-y-auto px-5 py-4" data-conversation>
				{#each messages as m (m.id)}
					{#if m.author === 'system'}
						<p class="py-1 text-center text-xs text-slate-400">{m.body}</p>
					{:else}
						<div class="flex {m.author === 'operator' ? 'justify-end' : 'justify-start'}">
							<div class="max-w-[70%] rounded-2xl px-3.5 py-2 text-sm {m.author === 'operator' ? 'rounded-br-md bg-brand-600 text-white' : 'rounded-bl-md bg-slate-100 text-slate-800'}">
								{#if m.author === 'operator'}<p class="text-xs opacity-75">{m.user_name}</p>{/if}
								<p class="break-words whitespace-pre-line">{m.body}</p>
								<p class="mt-0.5 text-right text-[10px] opacity-60">{formatDateTime(m.created_at)}</p>
							</div>
						</div>
					{/if}
				{/each}
			</div>
			{#if current.status === 'open'}
				{#if suggestions.length}
					<ul class="border-t border-slate-100 px-3 py-2 text-sm" data-canned>
						{#each suggestions as c (c.id)}<li><button type="button" class="w-full rounded px-2 py-1 text-left hover:bg-slate-100" onclick={() => useCanned(c)}><span class="font-mono text-brand-700">/{c.shortcut}</span> {c.title} <span class="text-slate-400">— {c.body.slice(0, 60)}</span></button></li>{/each}
					</ul>
				{/if}
				<form class="flex items-end gap-2 border-t border-slate-100 p-3" onsubmit={send}>
					<textarea class="input max-h-40 min-h-10 flex-1 resize-none" rows="2" maxlength="4000" placeholder="Reply… (type / for saved replies; Enter sends, Shift+Enter for a new line)" aria-label="Reply" bind:value={draft} onkeydown={keydown}></textarea>
					<button class="btn btn-primary" disabled={busy || !draft.trim()}>Send</button>
				</form>
			{/if}
		{:else}
			<div class="m-auto max-w-sm p-8 text-center text-sm text-slate-500">
				<Icon name="message" class="mx-auto mb-3 size-8 text-slate-300" />
				Pick a chat. New chats from your website's chat button appear here, and the operator they go to is notified.
			</div>
		{/if}
	</section>
</div>

<Modal bind:open={passOpen} title="Pass the chat on">
	<form id="pass-form" class="space-y-3 text-sm" onsubmit={pass}>
		<div><label class="label" for="pass-ch">Channel</label><select id="pass-ch" class="input" bind:value={passTo.channel} onchange={() => (passTo.user = '')}>{#each channels as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select></div>
		<div><label class="label" for="pass-u">Colleague</label><select id="pass-u" class="input" bind:value={passTo.user}><option value="">Whoever is next</option>{#each passOps as u (u)}<option value={u}>{people.find((p) => p.id === u)?.name ?? u}</option>{/each}</select></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (passOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="pass-form">Pass on</button>
	{/snippet}
</Modal>
