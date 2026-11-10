<script lang="ts">
	// Booking: a day's schedule, one row per resource; the waiting list beside it.
	import { replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount, untrack } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import BookingDialog from '#lib/components/booking/BookingDialog.svelte';
	import { BOOKING_STATUS, TILE, type Booking, type Resource, type Service } from '#lib/booking.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';

	let resources = $state<Resource[]>([]);
	let services = $state<Service[]>([]);
	let bookings = $state<Booking[]>([]);
	let waiting = $state<Booking[]>([]);
	// The day shown is kept in the address (?day=YYYY-MM-DD), so a reload or a shared link stays on it.
	const ymd = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
	const fromYmd = (v: string | null) => {
		const d = v && /^\d{4}-\d{2}-\d{2}$/.test(v) ? new Date(v + 'T00:00:00') : null;
		return d && !isNaN(+d) ? d : new Date(new Date().setHours(0, 0, 0, 0));
	};
	let day = $state(fromYmd(page.url.searchParams.get('day')));
	let showCancelled = $state(false);

	// The hours shown: from the earliest opening to the latest closing (local time), 8–20 at least.
	const span = $derived.by(() => {
		let from = 8 * 60, to = 20 * 60;
		for (const r of resources) for (const h of r.hours) (from = Math.min(from, h.start_min)), (to = Math.max(to, h.end_min));
		return { from: Math.floor(from / 60) * 60, to: Math.ceil(to / 60) * 60 };
	});
	const hours = $derived(Array.from({ length: (span.to - span.from) / 60 }, (_, i) => span.from / 60 + i));
	const pct = (m: number) => ((m - span.from) / (span.to - span.from)) * 100;
	const minOf = (iso: string) => {
		const d = new Date(iso);
		const dayStart = +day;
		return Math.max(span.from, Math.min(span.to, Math.round((+d - dayStart) / 60e3)));
	};

	async function load() {
		const to = new Date(day.getFullYear(), day.getMonth(), day.getDate() + 1);
		const [b, w] = await Promise.all([
			api.get<{ items: Booking[] }>('/booking/bookings' + query({ from: day.toISOString(), to: to.toISOString() })),
			api.get<{ items: Booking[] }>('/booking/waitlist')
		]);
		bookings = b.items;
		waiting = w.items;
	}
	let mounted = false;
	onMount(() => {
		mounted = true;
		start();
		return realtime.on('booking.changed', () => load());
	});
	async function start() {
		const [r, s] = await Promise.all([api.get<{ items: Resource[] }>('/booking/resources'), api.get<{ items: Service[] }>('/booking/services')]);
		resources = r.items.filter((x) => x.is_active);
		services = s.items;
		const want = page.url.searchParams.get('booking');
		if (want) {
			try {
				const b = await api.get<Booking>(`/booking/bookings/${want}`);
				day = new Date(new Date(b.starts_at).setHours(0, 0, 0, 0));
				openBooking(b);
			} catch {
				/* gone */
			}
		}
	}
	$effect(() => {
		const d = ymd(day);
		untrack(() => {
			load();
			// After mount only: the router isn't ready before then. Untracked, so updating the address
			// doesn't re-run this.
			if (!mounted || page.url.searchParams.get('day') === d) return;
			const url = new URL(page.url.href);
			url.searchParams.set('day', d);
			replaceState(url, {});
		});
	});
	// By calendar day, not 24 hours, so a daylight-saving change doesn't drift off midnight.
	const shift = (n: number) => (day = new Date(day.getFullYear(), day.getMonth(), day.getDate() + n));

	// ----- the dialog -----
	let dialogOpen = $state(false);
	let current = $state<Booking | null>(null);
	let newStart = $state<Date | null>(null);
	let newResource = $state<string | null>(null);
	function openBooking(b: Booking) {
		current = b;
		dialogOpen = true;
	}
	function newAt(e: MouseEvent, r: Resource) {
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		const m = span.from + ((e.clientX - rect.left) / rect.width) * (span.to - span.from);
		const snapped = Math.floor(m / 15) * 15;
		current = null;
		newStart = new Date(+day + snapped * 60e3);
		newResource = r.id;
		dialogOpen = true;
	}
	function newBooking() {
		current = null;
		newStart = null;
		newResource = null;
		dialogOpen = true;
	}
	const rowOf = (r: Resource) => bookings.filter((b) => b.resource_ids.includes(r.id) && (showCancelled || b.status !== 'cancelled'));
	const weekday = $derived(((day.getDay() + 6) % 7) + 1);
	const isToday = $derived(new Date().toDateString() === day.toDateString());
	const nowMin = $derived(new Date().getHours() * 60 + new Date().getMinutes());
</script>

<svelte:head><title>Booking · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Booking</h1>
		<div class="flex items-center gap-1">
			<button class="btn btn-ghost px-2" aria-label="Previous day" onclick={() => shift(-1)}>‹</button>
			<input type="date" class="input w-40" aria-label="Day" value={ymd(day)} onchange={(e) => e.currentTarget.value && (day = fromYmd(e.currentTarget.value))} />
			<button class="btn btn-ghost px-2" aria-label="Next day" onclick={() => shift(1)}>›</button>
			<button class="btn btn-ghost text-sm" onclick={() => (day = new Date(new Date().setHours(0, 0, 0, 0)))}>Today</button>
		</div>
		<span class="text-sm text-slate-500" data-day-label>{day.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })}</span>
		<label class="ml-auto flex items-center gap-2 text-sm text-slate-600"><input type="checkbox" class="rounded border-slate-300" bind:checked={showCancelled} /> Show cancelled</label>
		{#if session.can(PERM.BOOKING)}<a class="btn btn-secondary" href="/booking/settings"><Icon name="settings" class="size-4" /> Set up</a>{/if}
		<button class="btn btn-primary" onclick={newBooking}><Icon name="plus" class="size-4" /> New booking</button>
	</div>

	{#if !resources.length}
		<div class="card p-10 text-center text-sm text-slate-500">Nothing to book yet.{#if session.can(PERM.BOOKING)} <a class="text-brand-600 hover:underline" href="/booking/settings">Add people, rooms or equipment</a>.{/if}</div>
	{:else}
		<div class="grid gap-4 xl:grid-cols-[1fr_18rem]">
			<div class="card overflow-x-auto" aria-label="Schedule">
				<div class="min-w-[48rem]">
					<div class="flex border-b border-slate-100 text-xs text-slate-400">
						<div class="w-44 shrink-0"></div>
						<div class="relative h-7 flex-1">
							{#each hours as h (h)}<span class="absolute top-1.5 -translate-x-1/2" style="left: {pct(h * 60)}%">{String(h).padStart(2, '0')}:00</span>{/each}
						</div>
					</div>
					{#each resources as r (r.id)}
						{@const open = r.hours.filter((h) => h.weekday === weekday)}
						{@const off = r.days_off.some((d) => d.day === ymd(day))}
						<div class="flex border-b border-slate-100 last:border-0" data-resource={r.name}>
							<div class="w-44 shrink-0 px-3 py-3">
								<p class="truncate text-sm font-medium text-slate-900">{r.name}</p>
								<p class="text-xs text-slate-500">{off ? 'Day off' : open.length ? open.map((h) => `${String(Math.floor(h.start_min / 60)).padStart(2, '0')}:${String(h.start_min % 60).padStart(2, '0')}–${String(Math.floor(h.end_min / 60)).padStart(2, '0')}:${String(h.end_min % 60).padStart(2, '0')}`).join(', ') : 'Closed'}</p>
							</div>
							<!-- svelte-ignore a11y_click_events_have_key_events -->
							<!-- svelte-ignore a11y_no_static_element_interactions -->
							<div class="relative h-16 flex-1 cursor-copy bg-slate-50" onclick={(e) => newAt(e, r)} title="Click to book">
								{#if !off}
									{#each open as h (h.start_min)}<div class="absolute inset-y-0 bg-white" style="left: {pct(h.start_min)}%; width: {pct(h.end_min) - pct(h.start_min)}%"></div>{/each}
								{/if}
								{#each hours as h (h)}<div class="absolute inset-y-0 border-l border-slate-100" style="left: {pct(h * 60)}%"></div>{/each}
								{#if isToday && nowMin > span.from && nowMin < span.to}<div class="absolute inset-y-0 z-10 w-px bg-red-400" style="left: {pct(nowMin)}%"></div>{/if}
								{#each rowOf(r) as b (b.id)}
									{@const s = minOf(b.starts_at)}
									{@const e = minOf(b.ends_at)}
									<button
										class="absolute inset-y-1.5 z-20 overflow-hidden rounded-md border px-1.5 text-left text-xs {TILE[r.color] ?? TILE.sky} {b.status === 'cancelled' ? 'opacity-40 line-through' : ''} {b.status === 'came' ? 'opacity-70' : ''}"
										style="left: {pct(s)}%; width: max({pct(e) - pct(s)}%, 1.5rem)"
										onclick={(ev) => (ev.stopPropagation(), openBooking(b))}
										title="{b.client_name}{b.service_name ? ` — ${b.service_name}` : ''}"
										data-tile={b.client_name}
									>
										<span class="block truncate font-medium">{b.client_name}</span>
										<span class="block truncate opacity-80">{b.service_name ?? ''} {b.status !== 'booked' ? `· ${BOOKING_STATUS[b.status].label}` : ''}</span>
									</button>
								{/each}
							</div>
						</div>
					{/each}
				</div>
			</div>
			<aside class="card self-start" aria-label="Waiting list">
				<h2 class="border-b border-slate-100 px-4 py-2.5 text-sm font-semibold text-slate-800">Waiting list <span class="font-normal text-slate-400">{waiting.length}</span></h2>
				<ul class="divide-y divide-slate-100">
					{#each waiting as w (w.id)}
						<li><button class="w-full px-4 py-2.5 text-left hover:bg-slate-50" onclick={() => openBooking(w)} data-waiting={w.client_name}>
							<span class="block text-sm font-medium text-slate-900">{w.client_name}</span>
							<span class="block text-xs text-slate-500">{new Date(w.starts_at).toLocaleString([], { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' })}{w.service_name ? ` · ${w.service_name}` : ''}</span>
						</button></li>
					{:else}
						<li class="px-4 py-4 text-center text-xs text-slate-400">Nobody is waiting.</li>
					{/each}
				</ul>
			</aside>
		</div>
	{/if}
</div>

<BookingDialog bind:open={dialogOpen} booking={current} start={newStart} resourceId={newResource} {resources} {services} onchanged={load} />
