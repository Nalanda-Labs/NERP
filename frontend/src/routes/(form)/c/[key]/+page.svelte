<script lang="ts">
	// The visitor's chat (inside the website's chat panel, or opened directly). The session
	// token is kept in this browser so the conversation survives page changes.
	import { page } from '$app/state';
	import { onDestroy, tick } from 'svelte';
	import { api } from '#lib/api.ts';

	type Info = { title: string; greeting: string; color: string; ask_details: boolean; online: boolean; offline_message: string | null; consent_text: string | null; ask_rating: boolean };
	type Msg = { id: string; author: 'visitor' | 'operator' | 'system'; body: string; name: string | null; created_at: string };
	const key = $derived(page.params.key ?? '');
	const storeKey = $derived(`nerp.chat.${key}`);

	let info = $state<Info | null>(null);
	let missing = $state(false);
	let token = $state<string | null>(null);
	let messages = $state<Msg[]>([]);
	let status = $state<'open' | 'closed'>('open');
	let operator = $state<string | null>(null);
	let details = $state({ name: '', email: '', consent: false });
	let rating = $state<number | null>(null);
	let stars = $state(0);
	let comment = $state('');
	let draft = $state('');
	let error = $state('');
	let busy = $state(false);
	let list: HTMLElement | undefined = $state();

	function saved(): string | null {
		try {
			return localStorage.getItem(storeKey);
		} catch {
			return null;
		}
	}
	function remember(t: string | null) {
		try {
			if (t) localStorage.setItem(storeKey, t);
			else localStorage.removeItem(storeKey);
		} catch {
			/* private mode: the chat lasts while this page is open */
		}
	}

	$effect(() => {
		key;
		api.get<Info>(`/public/chat/${encodeURIComponent(key)}`)
			.then(async (i) => {
				info = i;
				token = saved();
				if (token) await refresh(true);
				else if (!i.ask_details && !i.consent_text) await begin();
			})
			.catch(() => (missing = true));
	});

	async function scrollDown() {
		await tick();
		list?.scrollTo({ top: list.scrollHeight });
	}
	async function refresh(all = false) {
		if (!token) return;
		const after = !all && messages.length ? messages[messages.length - 1].created_at : undefined;
		try {
			const r = await api.get<{ status: 'open' | 'closed'; operator: string | null; messages: Msg[]; rating: number | null }>(
				`/public/chat/${encodeURIComponent(key)}/session?token=${encodeURIComponent(token)}${after ? `&after=${encodeURIComponent(after)}` : ''}`
			);
			status = r.status;
			operator = r.operator;
			rating = r.rating;
			if (all) messages = r.messages;
			else if (r.messages.length) messages = [...messages, ...r.messages.filter((m) => !messages.some((x) => x.id === m.id))];
			if (r.messages.length || all) scrollDown();
		} catch {
			// The chat is gone (e.g. the button was removed): start over.
			token = null;
			remember(null);
		}
	}
	const timer = setInterval(() => status === 'open' && refresh(), 3000);
	onDestroy(() => clearInterval(timer));

	async function begin(e?: SubmitEvent) {
		e?.preventDefault();
		busy = true;
		error = '';
		try {
			const r = await api.post<{ token: string }>(`/public/chat/${encodeURIComponent(key)}/start`, { ...details, page_url: page.url.searchParams.get('u') ?? undefined, visitor_id: page.url.searchParams.get('vid') ?? undefined });
			token = r.token;
			remember(r.token);
			messages = [];
			status = 'open';
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
	async function send(e: SubmitEvent) {
		e.preventDefault();
		const body = draft.trim();
		if (!body || !token) return;
		busy = true;
		error = '';
		try {
			await api.post(`/public/chat/${encodeURIComponent(key)}/messages`, { token, body });
			draft = '';
			await refresh();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
	async function finish() {
		if (!token || !confirm('End this chat?')) return;
		await api.post(`/public/chat/${encodeURIComponent(key)}/end`, { token });
		await refresh();
	}
	function again() {
		token = null;
		remember(null);
		messages = [];
		status = 'open';
		rating = null;
		stars = 0;
		comment = '';
		if (!info?.ask_details && !info?.consent_text) begin();
	}
	async function rate(e: SubmitEvent) {
		e.preventDefault();
		if (!token || !stars) return;
		try {
			await api.post(`/public/chat/${encodeURIComponent(key)}/rate`, { token, rating: stars, comment: comment || null });
			rating = stars;
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}
	const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	function keydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			(e.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
		}
	}
</script>

<svelte:head><title>{info?.title ?? 'Chat'}</title><meta name="robots" content="noindex" /></svelte:head>

<main class="flex h-dvh flex-col bg-white">
	{#if missing}
		<p class="m-auto p-6 text-center text-sm text-slate-500">This chat is not available.</p>
	{:else if info}
		<header class="px-5 py-4 text-white" style="background:{info.color}">
			<h1 class="font-semibold">{info.title}</h1>
			<p class="text-sm opacity-90">{operator ? `You're chatting with ${operator}` : info.online ? 'We usually reply in a few minutes' : 'We are away right now'}</p>
		</header>

		{#if !token}
			<form class="m-auto w-full max-w-sm space-y-3 p-6" onsubmit={begin} aria-label="Start a chat">
				<p class="text-sm text-slate-700">{info.greeting}</p>
				{#if !info.online && info.offline_message}<p class="rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-900" data-offline>{info.offline_message}</p>{/if}
				{#if info.ask_details}
					<div><label class="label" for="ch-name">Your name</label><input id="ch-name" class="input" required maxlength="120" autocomplete="name" bind:value={details.name} /></div>
					<div><label class="label" for="ch-email">Email <span class="font-normal text-slate-400">(so we can reply if you leave)</span></label><input id="ch-email" class="input" type="email" maxlength="254" autocomplete="email" bind:value={details.email} /></div>
				{/if}
				{#if info.consent_text}<label class="flex items-start gap-2 text-sm text-slate-700"><input type="checkbox" class="mt-0.5 rounded border-slate-300" required bind:checked={details.consent} /> <span class="whitespace-pre-line">{info.consent_text}</span></label>{/if}
				{#if error}<p class="text-sm text-red-700" role="alert">{error}</p>{/if}
				<button class="btn w-full justify-center text-white" style="background:{info.color}" disabled={busy}>Start chat</button>
			</form>
		{:else}
			<div bind:this={list} class="flex-1 space-y-2 overflow-y-auto px-4 py-4" aria-live="polite" data-messages>
				<div class="max-w-[85%] rounded-2xl rounded-bl-md bg-slate-100 px-3.5 py-2 text-sm text-slate-800">{info.greeting}</div>
				{#each messages as m (m.id)}
					{#if m.author === 'system'}
						<p class="py-1 text-center text-xs text-slate-400">{m.body}</p>
					{:else}
						<div class="flex {m.author === 'visitor' ? 'justify-end' : 'justify-start'}" data-msg={m.author}>
							<div class="max-w-[85%] rounded-2xl px-3.5 py-2 text-sm {m.author === 'visitor' ? 'rounded-br-md text-white' : 'rounded-bl-md bg-slate-100 text-slate-800'}" style={m.author === 'visitor' ? `background:${info.color}` : ''}>
								{#if m.author === 'operator' && m.name}<p class="text-xs font-medium text-slate-500">{m.name}</p>{/if}
								<p class="break-words whitespace-pre-line">{m.body}</p>
								<p class="mt-0.5 text-right text-[10px] opacity-60">{time(m.created_at)}</p>
							</div>
						</div>
					{/if}
				{/each}
			</div>
			{#if status === 'open'}
				<form class="flex items-end gap-2 border-t border-slate-100 p-3" onsubmit={send}>
					<textarea class="input max-h-32 min-h-10 flex-1 resize-none" rows="1" maxlength="4000" placeholder="Type a message…" aria-label="Message" bind:value={draft} onkeydown={keydown}></textarea>
					<button class="btn px-4 text-white" style="background:{info.color}" disabled={busy || !draft.trim()} aria-label="Send">Send</button>
				</form>
				<div class="flex justify-between px-4 pb-2 text-xs">
					{#if error}<span class="text-red-700" role="alert">{error}</span>{:else}<span></span>{/if}
					{#if messages.length}<button class="text-slate-400 hover:text-slate-700" onclick={finish}>End chat</button>{/if}
				</div>
			{:else}
				<div class="border-t border-slate-100 p-4 text-center text-sm text-slate-600">
					<p>This chat has ended. Thank you!</p>
					{#if info.ask_rating && messages.length}
						{#if rating}
							<p class="mt-2" data-rated>You rated it {rating} out of 5. Thanks!</p>
						{:else}
							<form class="mt-2 space-y-2" onsubmit={rate} aria-label="Rate the chat">
								<p>How did we do?</p>
								<div class="flex justify-center gap-1" role="group" aria-label="Rating">
									{#each [1, 2, 3, 4, 5] as n (n)}<button type="button" class="text-2xl {n <= stars ? 'text-amber-400' : 'text-slate-300'}" aria-label="{n} out of 5" aria-pressed={n === stars} onclick={() => (stars = n)}>★</button>{/each}
								</div>
								{#if stars}<textarea class="input" rows="2" maxlength="2000" placeholder="Anything to add? (optional)" aria-label="Comment" bind:value={comment}></textarea><button class="btn text-white" style="background:{info.color}">Send</button>{/if}
							</form>
						{/if}
					{/if}
					<button class="mt-2 font-medium hover:underline" style="color:{info.color}" onclick={again}>Start a new chat</button>
				</div>
			{/if}
		{/if}
	{/if}
</main>
