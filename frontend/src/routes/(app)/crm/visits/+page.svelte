<script lang="ts">
	// Field visits: a person's day in route order — add visits, plan the day, check in and out
	// (with the browser's location). The mobile app uses the same server side later.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import CrmPicker from '#lib/components/crm/CrmPicker.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';

	type Visit = { id: string; title: string; subject: string; link: string; order: number | null; starts_at: string; ends_at: string; duration_minutes: number; fixed_time: boolean; done: boolean; address: string | null; lat: number | null; lng: number | null; leg_km: number | null; leg_minutes: number | null; checked_in_at: string | null; in_distance_m: number | null; checked_out_at: string | null; notes: string | null };
	type Day = { visits: Visit[]; total_km: number; travel_minutes: number; start: { lat: number; lng: number; label: string | null } | null };

	let date = $state(new Date().toISOString().slice(0, 10));
	let who = $state<{ id: string; name: string }>({ id: session.user!.id, name: session.user!.full_name });
	let day = $state<Day | null>(null);
	let adding = $state<{ id: string; title: string } | null>(null);
	let time = $state('');
	let duration = $state(30);
	let notes = $state<Record<string, string>>({});
	const mine = $derived(who.id === session.user!.id);

	async function load() {
		try {
			day = await api.get<Day>(`/field/day?date=${date}${mine ? '' : `&user=${who.id}`}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	async function plan() {
		try {
			day = await api.post<Day>('/field/day/plan', { date, user: who.id });
			toasts.success('The day is planned');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function add() {
		if (!adding) return;
		try {
			await api.post('/field/visits', { item_id: adding.id, day: date, time: time || null, duration_minutes: Number(duration), responsible_id: who.id });
			adding = null;
			time = '';
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	const here = () =>
		new Promise<{ lat?: number; lng?: number; accuracy_m?: number }>((res) => {
			if (!navigator.geolocation) return res({});
			navigator.geolocation.getCurrentPosition((p) => res({ lat: p.coords.latitude, lng: p.coords.longitude, accuracy_m: p.coords.accuracy }), () => res({}), { timeout: 8000 });
		});
	async function checkIn(v: Visit) {
		try {
			const r = await api.post<{ far: boolean; distance_m: number | null }>(`/field/visits/${v.id}/check-in`, await here());
			if (r.far) toasts.error(new Error(`Checked in ${r.distance_m} m from the address`));
			else toasts.success('Checked in');
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function checkOut(v: Visit) {
		try {
			await api.post(`/field/visits/${v.id}/check-out`, { ...(await here()), notes: notes[v.id] ?? '' });
			toasts.success('Visit logged');
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	const hm = (d: string) => new Date(d).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	// A small sketch of the route (no map tiles needed).
	const sketch = $derived.by(() => {
		const pts = (day?.visits ?? []).filter((v) => v.lat !== null && v.lng !== null).map((v) => ({ x: v.lng!, y: v.lat!, label: String(v.order ?? '') }));
		if (day?.start) pts.unshift({ x: day.start.lng, y: day.start.lat, label: '⌂' });
		if (pts.length < 2) return null;
		const xs = pts.map((p) => p.x), ys = pts.map((p) => p.y);
		const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
		const sx = (x: number) => 20 + ((x - x0) / (x1 - x0 || 1)) * 560, sy = (y: number) => 140 - ((y - y0) / (y1 - y0 || 1)) * 120;
		return pts.map((p) => ({ x: sx(p.x), y: sy(p.y), label: p.label }));
	});
</script>

<svelte:head><title>Field visits · NERP</title></svelte:head>

<div class="max-w-4xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Field visits</h1>
		<input type="date" class="input w-44" bind:value={date} onchange={load} aria-label="Day" />
		<div class="w-56"><UserPicker placeholder={who.name} onpick={(u) => ((who = { id: u.id, name: u.full_name }), load())} /></div>
		<button class="btn btn-primary ml-auto" onclick={plan} disabled={!day?.visits.length} data-plan-day>Plan the day</button>
	</div>

	{#if day}
		<p class="text-sm text-slate-600" data-totals>{day.visits.length} visits · {day.total_km} km · about {day.travel_minutes} min on the road{day.start?.label ? ` · from ${day.start.label}` : ''}</p>
		{#if sketch}
			<svg viewBox="0 0 600 160" class="card w-full" role="img" aria-label="Route sketch">
				<polyline points={sketch.map((p) => `${p.x},${p.y}`).join(' ')} fill="none" stroke="currentColor" class="text-brand-500" stroke-width="2" />
				{#each sketch as p, i (i)}<circle cx={p.x} cy={p.y} r="9" class="fill-white stroke-brand-600" stroke-width="2" /><text x={p.x} y={p.y + 4} text-anchor="middle" class="fill-slate-800 text-[10px]">{p.label}</text>{/each}
			</svg>
		{/if}
		<section class="card divide-y divide-slate-100 text-sm" aria-label="Visits">
			{#each day.visits as v (v.id)}
				<div class="flex flex-wrap items-start gap-3 px-5 py-3" data-visit={v.title}>
					<span class="w-14 shrink-0 font-semibold tabular-nums">{hm(v.starts_at)}</span>
					<div class="min-w-0 flex-1">
						<p><a class="font-medium text-brand-700 hover:underline" href={v.link}>{v.title}</a> <span class="text-xs text-slate-500">{v.duration_minutes} min{v.fixed_time ? ' · fixed time' : ''}</span></p>
						<p class="text-xs text-slate-500">{v.address ?? 'No address'}{v.leg_km !== null ? ` · ${v.leg_km} km, ${v.leg_minutes} min drive` : ''}</p>
						{#if v.checked_in_at}<p class="text-xs {v.in_distance_m && v.in_distance_m > 500 ? 'text-red-700' : 'text-emerald-700'}">In {hm(v.checked_in_at)}{v.in_distance_m !== null ? ` (${v.in_distance_m} m from the address)` : ''}{v.checked_out_at ? ` · out ${hm(v.checked_out_at)}` : ''}</p>{/if}
						{#if v.notes}<p class="text-xs text-slate-600">{v.notes}</p>{/if}
					</div>
					{#if mine && !v.done}
						{#if !v.checked_in_at}<button class="btn btn-secondary" onclick={() => checkIn(v)} data-check-in>Check in</button>
						{:else}
							<input class="input w-56" placeholder="Notes" bind:value={notes[v.id]} aria-label="Notes" />
							<button class="btn btn-primary" onclick={() => checkOut(v)} data-check-out>Check out</button>
						{/if}
					{/if}
				</div>
			{:else}<p class="px-5 py-4 text-slate-500">No visits this day.</p>{/each}
		</section>
		<section class="card space-y-3 p-5 text-sm" aria-label="Add a visit">
			<h2 class="font-semibold text-slate-900">Add a visit</h2>
			{#if adding}
				<p>To <b>{adding.title}</b> <button class="text-slate-400" onclick={() => (adding = null)}>×</button></p>
				<div class="flex flex-wrap items-center gap-2">
					<label>At <input type="time" class="input w-32" bind:value={time} aria-label="Time" /></label>
					<span class="text-xs text-slate-500">(empty: any time — the plan sets it)</span>
					<label>for <input type="number" min="5" max="720" class="input w-24" bind:value={duration} aria-label="Minutes" /> min</label>
					<button class="btn btn-primary" onclick={add} data-add-visit>Add</button>
				</div>
			{:else}
				<CrmPicker types={['company', 'contact', 'deal', 'lead']} placeholder="Find a client or deal…" onpick={(h) => (adding = { id: h.id, title: h.title })} />
			{/if}
		</section>
	{/if}
</div>
