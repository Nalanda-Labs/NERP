<script lang="ts">
	// A slot link: pick a free time with the people behind it, then leave your name and email.
	// Times are shown in the visitor's own time zone.
	import { onMount } from 'svelte';
	import { page } from '$app/state';

	type Info = { title: string; description: string | null; location: string | null; duration_minutes: number; timezone: string; days_ahead: number; company: string | null; people: string[] };
	type Day = { date: string; slots: string[] };

	const slug = $derived(page.params.slug!);
	let info = $state<Info | null>(null);
	let missing = $state(false);
	const pad = (n: number) => String(n).padStart(2, '0');
	const iso = (d: Date) => `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
	const todayUtc = new Date(Date.UTC(new Date().getUTCFullYear(), new Date().getUTCMonth(), new Date().getUTCDate()));
	let weekStart = $state(todayUtc);
	let days = $state<Day[] | null>(null);
	let chosen = $state<string | null>(null);
	let details = $state({ name: '', email: '', note: '', website: '' });
	let busy = $state(false);
	let error = $state('');
	let done = $state<{ starts_at: string; ends_at: string; manage_url: string } | null>(null);
	const visitorTz = Intl.DateTimeFormat().resolvedOptions().timeZone;

	onMount(async () => {
		const r = await fetch(`/api/public/calendar/meet/${encodeURIComponent(slug)}`);
		if (!r.ok) return void (missing = true);
		info = await r.json();
	});
	// A week of days (in the link's days; the visitor sees each time in their own zone).
	async function load() {
		days = null;
		chosen = null;
		const to = new Date(+weekStart + 6 * 86400e3);
		const r = await fetch(`/api/public/calendar/meet/${encodeURIComponent(slug)}/slots?from=${iso(weekStart)}&to=${iso(to)}`);
		days = r.ok ? (await r.json()).days : [];
	}
	$effect(() => {
		weekStart;
		if (info) load();
	});
	// Slots grouped by the visitor's local day.
	const byDay = $derived.by(() => {
		const map = new Map<string, string[]>();
		for (const d of days ?? []) for (const s of d.slots) {
			const k = new Date(s).toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long' });
			map.set(k, [...(map.get(k) ?? []), s]);
		}
		return [...map.entries()];
	});
	function shift(n: number) {
		const d = new Date(+weekStart + n * 7 * 86400e3);
		if (d >= todayUtc) weekStart = d;
	}
	const time = (s: string) => new Date(s).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	async function book(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			const r = await fetch(`/api/public/calendar/meet/${encodeURIComponent(slug)}`, {
				method: 'POST',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({ starts_at: chosen, name: details.name, email: details.email, note: details.note || null, website: details.website })
			});
			const data = await r.json();
			if (!r.ok) {
				error = data?.error?.message ?? 'Something went wrong';
				if (r.status === 409) load();
				return;
			}
			done = data;
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>{info?.title ?? 'Book a time'}</title></svelte:head>

<main class="min-h-dvh bg-slate-50 px-4 py-10">
	<div class="mx-auto max-w-xl space-y-5">
		{#if missing}
			<div class="rounded-2xl bg-white p-8 text-center text-slate-600 shadow-sm">This page isn't available.</div>
		{:else if done}
			<div class="space-y-3 rounded-2xl bg-white p-8 text-center shadow-sm" data-done>
				<p class="text-4xl">✅</p>
				<h1 class="text-xl font-semibold text-slate-900">You're booked</h1>
				<p class="text-slate-600">{new Date(done.starts_at).toLocaleString([], { weekday: 'long', day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' })} – {time(done.ends_at)}</p>
				<p class="text-sm text-slate-500">An invitation is on its way to your email.</p>
				<a class="inline-block text-sm text-brand-600 hover:underline" href={done.manage_url}>See or cancel it</a>
			</div>
		{:else if info}
			<div>
				{#if info.company}<p class="text-sm text-slate-500">{info.company}</p>{/if}
				<h1 class="text-2xl font-semibold text-slate-900">{info.title}</h1>
				<p class="mt-1 text-slate-600">{info.duration_minutes} min with {info.people.join(', ')}{info.location ? ` · ${info.location}` : ''}</p>
				{#if info.description}<p class="mt-2 whitespace-pre-line text-slate-600">{info.description}</p>{/if}
			</div>
			<section class="space-y-3 rounded-2xl bg-white p-5 shadow-sm" aria-label="Times">
				<div class="flex items-center gap-2">
					<button class="btn btn-ghost px-2" aria-label="Earlier" disabled={+weekStart <= +todayUtc} onclick={() => shift(-1)}>‹</button>
					<p class="flex-1 text-center text-sm text-slate-500">Times in {visitorTz.replace(/_/g, ' ')}</p>
					<button class="btn btn-ghost px-2" aria-label="Later" onclick={() => shift(1)}>›</button>
				</div>
				{#if days === null}
					<p class="text-center text-sm text-slate-400">Looking…</p>
				{:else if byDay.length}
					{#each byDay as [label, slots] (label)}
						<div>
							<p class="mb-1.5 text-sm font-medium text-slate-800">{label}</p>
							<div class="grid grid-cols-3 gap-2 sm:grid-cols-5">
								{#each slots as s (s)}
									<button class="rounded-lg border px-2 py-2 text-sm {chosen === s ? 'border-brand-600 bg-brand-600 text-white' : 'border-slate-200 hover:border-brand-400'}" aria-pressed={chosen === s} onclick={() => (chosen = s)} data-slot={s}>{time(s)}</button>
								{/each}
							</div>
						</div>
					{/each}
				{:else}
					<p class="text-center text-sm text-slate-500">Nothing free this week. Try the next one.</p>
				{/if}
			</section>
			{#if chosen}
				<form class="space-y-3 rounded-2xl bg-white p-5 shadow-sm" onsubmit={book} aria-label="Your details">
					<div><label class="block text-sm font-medium text-slate-700" for="mt-n">Your name</label><input id="mt-n" class="input" required maxlength="200" bind:value={details.name} /></div>
					<div><label class="block text-sm font-medium text-slate-700" for="mt-e">Email</label><input id="mt-e" type="email" class="input" required bind:value={details.email} /></div>
					<div><label class="block text-sm font-medium text-slate-700" for="mt-no">What would you like to talk about?</label><textarea id="mt-no" class="input" rows="2" maxlength="2000" bind:value={details.note}></textarea></div>
					<input class="hidden" tabindex="-1" autocomplete="off" aria-hidden="true" name="website" bind:value={details.website} />
					{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">{error}</p>{/if}
					<button class="btn btn-primary w-full justify-center" disabled={busy}>Book {new Date(chosen).toLocaleString([], { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' })}</button>
				</form>
			{/if}
		{/if}
	</div>
</main>
