<script lang="ts">
	// An invitation sent by email: see the meeting and accept, decline or say maybe.
	import { onMount } from 'svelte';
	import { page } from '$app/state';

	type View = {
		title: string; description: string | null; location: string | null; starts_at: string; ends_at: string; all_day: boolean; timezone: string;
		repeat: string | null; next: string | null; organizer: string; company: string; cancelled?: boolean;
		guest: { email: string; name: string | null; status: string };
	};
	const token = $derived(page.params.token!);
	let view = $state<View | null>(null);
	let missing = $state(false);
	let busy = $state(false);
	let error = $state('');

	onMount(async () => {
		const r = await fetch(`/api/public/calendar/rsvp/${encodeURIComponent(token)}`);
		if (!r.ok) return void (missing = true);
		view = await r.json();
	});
	async function answer(status: string) {
		busy = true;
		error = '';
		try {
			const r = await fetch(`/api/public/calendar/rsvp/${encodeURIComponent(token)}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ status }) });
			const data = await r.json();
			if (!r.ok) error = data?.error?.message ?? 'Something went wrong';
			else view = data;
		} finally {
			busy = false;
		}
	}
	const when = (v: View) => {
		const s = new Date(v.next ?? v.starts_at);
		const e = new Date(+s + (+new Date(v.ends_at) - +new Date(v.starts_at)));
		if (v.all_day) return s.toLocaleDateString([], { timeZone: 'UTC', weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
		return `${s.toLocaleString([], { weekday: 'long', day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' })} – ${e.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
	};
	const LABEL: Record<string, string> = { accepted: "You're going", declined: "You're not going", tentative: 'You might go', pending: "You haven't answered yet" };
</script>

<svelte:head><title>{view?.title ?? 'Invitation'}</title></svelte:head>

<main class="min-h-dvh bg-slate-50 px-4 py-10">
	<div class="mx-auto max-w-lg">
		{#if missing}
			<div class="rounded-2xl bg-white p-8 text-center text-slate-600 shadow-sm">This invitation isn't available any more.</div>
		{:else if view?.cancelled}
			<div class="space-y-2 rounded-2xl bg-white p-8 text-center shadow-sm" data-cancelled>
				<h1 class="text-xl font-semibold text-slate-900">Cancelled</h1>
				<p class="text-slate-600">“{view.title}” is cancelled. {view.organizer} has been told.</p>
			</div>
		{:else if view}
			<div class="space-y-4 rounded-2xl bg-white p-6 shadow-sm">
				<div>
					<p class="text-sm text-slate-500">{view.company} · from {view.organizer}</p>
					<h1 class="text-2xl font-semibold text-slate-900">{view.title}</h1>
				</div>
				<dl class="space-y-1 text-sm text-slate-700">
					<div><dt class="sr-only">When</dt><dd>🗓 {when(view)}{#if view.repeat} <span class="text-slate-500">({view.repeat})</span>{/if}</dd></div>
					{#if view.location}<div><dt class="sr-only">Where</dt><dd>📍 {view.location}</dd></div>{/if}
				</dl>
				{#if view.description}<p class="whitespace-pre-line text-sm text-slate-600">{view.description}</p>{/if}
				<p class="font-medium text-slate-800" data-status={view.guest.status}>{LABEL[view.guest.status]}</p>
				<div class="flex flex-wrap gap-2" role="group" aria-label="Your answer">
					{#each [['accepted', 'Going'], ['tentative', 'Maybe'], ['declined', 'Not going']] as [v, l] (v)}
						<button class="btn {view.guest.status === v ? 'btn-primary' : 'btn-secondary'}" aria-pressed={view.guest.status === v} disabled={busy} onclick={() => answer(v)}>{l}</button>
					{/each}
				</div>
				{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">{error}</p>{/if}
				<a class="inline-block text-sm text-brand-600 hover:underline" href="/api/public/calendar/rsvp/{token}/invite.ics">Add to my calendar (.ics)</a>
			</div>
		{/if}
	</div>
</main>
