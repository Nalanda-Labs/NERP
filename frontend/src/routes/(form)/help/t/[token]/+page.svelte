<script lang="ts">
	// A customer's request, opened with their private link: the conversation, answering,
	// closing it and rating the answer.
	import { page } from '$app/state';
	import { onMount } from 'svelte';

	type View = {
		number: number; title: string; requester_name: string | null; created_at: string; status: string; category: string;
		closed_at: string | null; waiting_for: 'support' | 'customer'; mark_id: string | null; can_write: boolean; company: string;
		marks: { id: string; name: string }[];
		messages: { id: string; by_support: boolean; author_name: string; body: string; created_at: string; files: { id: string; name: string; size: number }[] }[];
	};
	const token = $derived(page.params.token!);
	const fresh = $derived(page.url.searchParams.get('new'));
	let v = $state<View | null>(null);
	let missing = $state(false);
	let body = $state('');
	let busy = $state(false);
	let error = $state('');
	const base = $derived(`/api/public/support/t/${encodeURIComponent(token)}`);
	onMount(async () => {
		const r = await fetch(base);
		if (!r.ok) return void (missing = true);
		v = await r.json();
	});
	async function call(path: string, payload?: unknown) {
		busy = true;
		error = '';
		try {
			const r = await fetch(base + path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(payload ?? {}) });
			const data = await r.json();
			if (!r.ok) error = data?.error?.message ?? 'Something went wrong; please try again.';
			else v = data;
			return r.ok;
		} finally {
			busy = false;
		}
	}
	async function write(e: SubmitEvent) {
		e.preventDefault();
		if (await call('/messages', { body })) body = '';
	}
	const when = (s: string) => new Date(s).toLocaleString([], { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });
</script>

<svelte:head><title>{v ? `Request #${v.number} · ${v.company}` : 'Your request'}</title></svelte:head>

<main class="min-h-dvh bg-slate-50 px-4 py-10">
	<div class="mx-auto max-w-2xl space-y-4">
		{#if missing}
			<p class="rounded-2xl bg-white p-8 text-center text-slate-600 shadow-sm">This link isn't valid any more.</p>
		{:else if v}
			{#if fresh}<p class="rounded-xl bg-emerald-50 px-4 py-3 text-emerald-800" role="status">Thank you! Your request number is #{fresh}. We've emailed you this page's link.</p>{/if}
			<div class="rounded-2xl bg-white p-6 shadow-sm">
				<p class="text-sm text-slate-500">{v.company} · Request #{v.number}{v.category ? ` · ${v.category}` : ''}</p>
				<h1 class="mt-1 text-xl font-semibold text-slate-900">{v.title}</h1>
				<p class="mt-2"><span class="rounded-full bg-slate-100 px-3 py-1 text-sm font-medium text-slate-700" data-status>{v.closed_at ? 'Closed' : v.waiting_for === 'customer' ? 'Answered' : 'We are working on it'}</span></p>
			</div>
			{#each v.messages as m (m.id)}
				<article class="rounded-2xl p-5 shadow-sm {m.by_support ? 'bg-brand-50' : 'bg-white'}" data-message>
					<p class="text-sm"><span class="font-medium text-slate-900">{m.author_name}</span>{#if m.by_support} <span class="text-slate-500">· {v.company}</span>{/if} <span class="float-right text-xs text-slate-400">{when(m.created_at)}</span></p>
					<p class="mt-2 whitespace-pre-line text-slate-800">{m.body}</p>
					{#each m.files as f (f.id)}<a class="mt-2 mr-2 inline-block rounded-lg border border-slate-200 bg-white px-2.5 py-1 text-xs text-slate-700" href="{base}/files/{f.id}">📎 {f.name}</a>{/each}
				</article>
			{/each}
			{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">{error}</p>{/if}
			{#if v.can_write}
				<form class="space-y-3 rounded-2xl bg-white p-5 shadow-sm" onsubmit={write}>
					<label class="label" for="r-body">{v.closed_at ? 'Not solved? Write to reopen your request' : 'Add a message'}</label>
					<textarea id="r-body" class="input min-h-28" required bind:value={body}></textarea>
					<div class="flex flex-wrap gap-2">
						<button class="btn btn-primary" type="submit" disabled={busy || !body.trim()}>Send</button>
						{#if !v.closed_at}<button class="btn btn-secondary" type="button" disabled={busy} onclick={() => call('/close')}>My problem is solved</button>{/if}
					</div>
				</form>
			{/if}
			{#if v.closed_at && v.marks.length}
				<section class="rounded-2xl bg-white p-5 shadow-sm" data-rate>
					<h2 class="font-medium text-slate-900">How was our answer?</h2>
					<div class="mt-3 flex flex-wrap gap-2">
						{#each v.marks as m (m.id)}<button class="btn {v.mark_id === m.id ? 'btn-primary' : 'btn-secondary'}" disabled={busy} onclick={() => call('/rate', { mark_id: m.id })}>{m.name}</button>{/each}
					</div>
					{#if v.mark_id}<p class="mt-2 text-sm text-emerald-700">Thank you for your rating.</p>{/if}
				</section>
			{/if}
		{/if}
	</div>
</main>
