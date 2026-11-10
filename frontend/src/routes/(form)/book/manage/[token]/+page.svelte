<script lang="ts">
	// A client's booking: see it, confirm it or cancel it.
	import { onMount } from 'svelte';
	import { page } from '$app/state';

	type View = { number: number; status: string; starts_at: string; ends_at: string; timezone: string; service: string | null; with: string[]; client_name: string; price: string; can_change: boolean };
	const token = $derived(page.params.token!);
	let b = $state<View | null>(null);
	let missing = $state(false);
	let error = $state('');
	const LABEL: Record<string, string> = { booked: 'Booked', confirmed: 'Confirmed', came: 'Visited', no_show: 'Missed', cancelled: 'Cancelled', waitlist: 'On the waiting list' };
	onMount(async () => {
		const r = await fetch(`/api/public/booking/manage/${encodeURIComponent(token)}`);
		if (!r.ok) return void (missing = true);
		b = await r.json();
	});
	async function act(action: 'confirm' | 'cancel') {
		if (action === 'cancel' && !confirm('Cancel your booking?')) return;
		const r = await fetch(`/api/public/booking/manage/${encodeURIComponent(token)}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action }) });
		const data = await r.json();
		if (!r.ok) error = data?.error?.message ?? 'Something went wrong';
		else b = data;
	}
	const fmt = (s: string) => new Date(s).toLocaleString([], { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric', hour: '2-digit', minute: '2-digit' });
</script>

<svelte:head><title>Your booking</title></svelte:head>

<main class="min-h-dvh bg-slate-50 px-4 py-10">
	<div class="mx-auto max-w-md rounded-2xl bg-white p-8 shadow-sm">
		{#if missing}
			<p class="text-center text-slate-600">This link isn't valid any more.</p>
		{:else if b}
			<p class="text-sm text-slate-500">Booking #{b.number}</p>
			<h1 class="mt-1 text-xl font-semibold text-slate-900">{b.service ?? 'Your booking'}{b.with.length ? ` with ${b.with.join(', ')}` : ''}</h1>
			<p class="mt-2 text-slate-700">{fmt(b.starts_at)}</p>
			<p class="mt-3"><span class="rounded-full bg-slate-100 px-3 py-1 text-sm font-medium text-slate-700" data-status={b.status}>{LABEL[b.status] ?? b.status}</span></p>
			{#if error}<p class="mt-3 rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">{error}</p>{/if}
			{#if b.can_change}
				<div class="mt-6 flex gap-2">
					{#if b.status === 'booked'}<button class="btn btn-primary flex-1 justify-center" onclick={() => act('confirm')}>I'll be there</button>{/if}
					<button class="btn btn-secondary flex-1 justify-center" onclick={() => act('cancel')}>Cancel</button>
				</div>
			{/if}
		{/if}
	</div>
</main>
