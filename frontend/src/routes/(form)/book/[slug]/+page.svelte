<script lang="ts">
	// A self-booking form: choose a service (and who), a day and a time, then leave your details.
	import { onMount } from 'svelte';
	import { page } from '$app/state';

	type Svc = { id: string; name: string; description: string | null; duration_minutes: number; price: string; is_extra: boolean };
	type Res = { id: string; name: string; kind: string; description: string | null; timezone: string };
	type Form = { name: string; intro: string | null; ask_phone: boolean; phone_required: boolean; choose_resource: boolean; waitlist: boolean; currency: string; services: Svc[]; resources: Res[] };
	type Slot = { start: string; resource_ids: string[] };

	const slug = $derived(page.params.slug!);
	let form = $state<Form | null>(null);
	let missing = $state(false);
	let service = $state('');
	let extras = $state<string[]>([]);
	let resource = $state('');
	const today = new Date();
	let day = $state(new Date(today.getFullYear(), today.getMonth(), today.getDate()));
	let slots = $state<Slot[] | null>(null);
	let chosen = $state<Slot | null>(null);
	let details = $state({ name: '', email: '', phone: '', note: '', website: '' });
	let busy = $state(false);
	let error = $state('');
	let done = $state<{ number: number; status: string; starts_at: string; manage_url: string; thanks: string | null } | null>(null);
	const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;

	onMount(async () => {
		const r = await fetch(`/api/public/booking/${encodeURIComponent(slug)}`);
		if (!r.ok) return void (missing = true);
		form = await r.json();
		const main = form!.services.filter((s) => !s.is_extra);
		if (main.length === 1) service = main[0]!.id;
	});
	async function loadSlots() {
		slots = null;
		chosen = null;
		if (!service) return;
		const q = new URLSearchParams({ service_id: service, date: iso(day) });
		if (resource) q.set('resource_id', resource);
		const r = await fetch(`/api/public/booking/${encodeURIComponent(slug)}/slots?${q}`);
		slots = r.ok ? (await r.json()).slots : [];
	}
	$effect(() => {
		service;
		resource;
		day;
		loadSlots();
	});
	async function book(e: SubmitEvent, waitlist = false) {
		e.preventDefault();
		busy = true;
		error = '';
		const start = waitlist ? new Date(`${iso(day)}T${waitTime}:00`).toISOString() : chosen!.start;
		try {
			const r = await fetch(`/api/public/booking/${encodeURIComponent(slug)}`, {
				method: 'POST',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({ service_id: service, extra_ids: extras, starts_at: start, resource_id: resource || (waitlist ? null : chosen!.resource_ids[0]), name: details.name, email: details.email, phone: details.phone || null, note: details.note || null, website: details.website, waitlist })
			});
			const data = await r.json();
			if (!r.ok) {
				error = data?.error?.message ?? 'Something went wrong';
				if (r.status === 409) loadSlots();
				return;
			}
			done = data;
		} finally {
			busy = false;
		}
	}
	let waitOpen = $state(false);
	let waitTime = $state('10:00');
	const svc = $derived(form?.services.find((s) => s.id === service));
	const shift = (n: number) => {
		const d = new Date(day);
		d.setDate(d.getDate() + n);
		if (d >= new Date(today.getFullYear(), today.getMonth(), today.getDate())) day = d;
	};
	const time = (s: string) => new Date(s).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	const total = $derived(form ? (Number(svc?.price ?? 0) + extras.reduce((n, x) => n + Number(form!.services.find((s) => s.id === x)?.price ?? 0), 0)).toFixed(2) : '0');
</script>

<svelte:head><title>{form?.name ?? 'Book'}</title></svelte:head>

<main class="min-h-dvh bg-slate-50 px-4 py-10">
	<div class="mx-auto max-w-xl space-y-5">
		{#if missing}
			<div class="rounded-2xl bg-white p-8 text-center text-slate-600 shadow-sm">This booking page isn't available.</div>
		{:else if done}
			<div class="space-y-3 rounded-2xl bg-white p-8 text-center shadow-sm" data-done>
				<p class="text-4xl">{done.status === 'waitlist' ? '⏳' : '✅'}</p>
				<h1 class="text-xl font-semibold text-slate-900">{done.status === 'waitlist' ? "You're on the waiting list" : "You're booked"}</h1>
				<p class="text-slate-600">{new Date(done.starts_at).toLocaleString([], { weekday: 'long', day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' })} · booking #{done.number}</p>
				{#if done.thanks}<p class="text-slate-600">{done.thanks}</p>{/if}
				<a class="inline-block text-sm text-brand-600 hover:underline" href={done.manage_url}>See, confirm or cancel your booking</a>
			</div>
		{:else if form}
			<div>
				<h1 class="text-2xl font-semibold text-slate-900">{form.name}</h1>
				{#if form.intro}<p class="mt-1 whitespace-pre-line text-slate-600">{form.intro}</p>{/if}
			</div>
			<section class="space-y-3 rounded-2xl bg-white p-5 shadow-sm">
				<label class="block text-sm font-medium text-slate-700" for="bk-s">What would you like?</label>
				<select id="bk-s" class="input" bind:value={service}>
					<option value="" disabled>Choose…</option>
					{#each form.services.filter((s) => !s.is_extra) as s (s.id)}<option value={s.id}>{s.name} · {s.duration_minutes >= 1440 ? `${s.duration_minutes / 1440} day(s)` : `${s.duration_minutes} min`} · {s.price} {form.currency}</option>{/each}
				</select>
				{#if form.services.some((s) => s.is_extra)}
					<div class="flex flex-wrap gap-3 text-sm" role="group" aria-label="Extras">
						{#each form.services.filter((s) => s.is_extra) as x (x.id)}<label class="flex items-center gap-1.5"><input type="checkbox" value={x.id} bind:group={extras} /> {x.name} (+{x.price})</label>{/each}
					</div>
				{/if}
				{#if form.choose_resource && form.resources.length > 1}
					<label class="block text-sm font-medium text-slate-700" for="bk-r">With</label>
					<select id="bk-r" class="input" bind:value={resource}><option value="">Anyone free</option>{#each form.resources as r (r.id)}<option value={r.id}>{r.name}</option>{/each}</select>
				{/if}
			</section>
			{#if service}
				<section class="space-y-3 rounded-2xl bg-white p-5 shadow-sm" aria-label="Times">
					<div class="flex items-center gap-2">
						<button class="btn btn-ghost px-2" aria-label="Previous day" onclick={() => shift(-1)}>‹</button>
						<p class="flex-1 text-center font-medium text-slate-900" data-day>{day.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long' })}</p>
						<button class="btn btn-ghost px-2" aria-label="Next day" onclick={() => shift(1)}>›</button>
					</div>
					{#if slots === null}
						<p class="text-center text-sm text-slate-400">Looking…</p>
					{:else if slots.length}
						<div class="grid grid-cols-3 gap-2 sm:grid-cols-4">
							{#each slots as s (s.start)}
								<button class="rounded-lg border px-2 py-2 text-sm {chosen?.start === s.start ? 'border-brand-600 bg-brand-600 text-white' : 'border-slate-200 hover:border-brand-400'}" onclick={() => (chosen = s)} data-slot={time(s.start)}>{time(s.start)}</button>
							{/each}
						</div>
					{:else}
						<p class="text-center text-sm text-slate-500">Nothing free this day.{#if form.waitlist} <button class="text-brand-600 hover:underline" onclick={() => (waitOpen = true)}>Join the waiting list</button>{/if}</p>
					{/if}
				</section>
			{/if}
			{#if chosen || waitOpen}
				<form class="space-y-3 rounded-2xl bg-white p-5 shadow-sm" onsubmit={(e) => book(e, !chosen)} aria-label="Your details">
					{#if !chosen}<div><label class="block text-sm font-medium text-slate-700" for="bk-w">Around what time?</label><input id="bk-w" type="time" class="input w-40" bind:value={waitTime} /></div>{/if}
					<div><label class="block text-sm font-medium text-slate-700" for="bk-n">Your name</label><input id="bk-n" class="input" required maxlength="200" bind:value={details.name} /></div>
					<div><label class="block text-sm font-medium text-slate-700" for="bk-e">Email</label><input id="bk-e" type="email" class="input" required bind:value={details.email} /></div>
					{#if form.ask_phone}<div><label class="block text-sm font-medium text-slate-700" for="bk-p">Phone</label><input id="bk-p" type="tel" class="input" required={form.phone_required} bind:value={details.phone} /></div>{/if}
					<div><label class="block text-sm font-medium text-slate-700" for="bk-no">Anything we should know?</label><textarea id="bk-no" class="input" rows="2" bind:value={details.note}></textarea></div>
					<input class="hidden" tabindex="-1" autocomplete="off" aria-hidden="true" name="website" bind:value={details.website} />
					{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">{error}</p>{/if}
					<button class="btn btn-primary w-full justify-center" disabled={busy}>{chosen ? `Book ${time(chosen.start)} · ${total} ${form.currency}` : 'Join the waiting list'}</button>
				</form>
			{/if}
		{/if}
	</div>
</main>
