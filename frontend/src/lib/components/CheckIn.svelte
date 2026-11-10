<script lang="ts">
	// Check-in in the top bar: "Check in" (with a dot while it's due), or where you're working
	// today. Opens the check-in dialog.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { locationOf, type CheckInToday } from '#lib/checkin.ts';
	import CheckInDialog from '#lib/components/CheckInDialog.svelte';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';

	let today = $state<CheckInToday | null>(null);
	let open = $state(false);
	const load = () =>
		api
			.get<CheckInToday>('/checkin/today')
			.then((t) => (today = t))
			.catch(() => (today = null));
	onMount(() => {
		load();
		const again = () => load();
		window.addEventListener('nerp:checkin', again);
		const off = realtime.on('checkin.changed', (e: { user_id?: string; settings?: boolean }) => {
			if (e?.settings || e?.user_id === session.user?.id) load();
		});
		// A new day may begin while the page is open.
		const t = setInterval(load, 15 * 60_000);
		return () => (window.removeEventListener('nerp:checkin', again), off(), clearInterval(t));
	});
	const k = $derived(today?.checkin ?? null);
</script>

{#if today?.enabled}
	{#if !k}
		<button
			class="relative btn px-3 py-1.5 text-sm {today.remind ? 'btn-secondary' : 'btn-ghost text-slate-600'}"
			onclick={() => (open = true)}
			data-checkin={today.remind ? 'due' : 'none'}
			title={today.remind ? 'Say where you are working today' : 'Check in'}
		>
			Check in
			{#if today.remind}<span class="absolute -top-0.5 -right-0.5 size-2.5 rounded-full bg-red-500 ring-2 ring-white" aria-label="due"></span>{/if}
		</button>
	{:else}
		<button class="flex items-center gap-1.5 rounded-full px-3 py-1.5 text-sm hover:bg-slate-100" onclick={() => (open = true)} data-checkin={k.status} title="Today's check-in">
			{#if k.status === 'working'}
				<span aria-hidden="true">{locationOf(k.location).icon}</span><span class="hidden max-w-32 truncate sm:inline">{k.location === 'custom' ? k.location_text : locationOf(k.location).label}</span>
			{:else}
				<span aria-hidden="true">🚫</span><span class="hidden sm:inline">Not working</span>
			{/if}
		</button>
	{/if}
{/if}

<CheckInDialog bind:open bind:today />
