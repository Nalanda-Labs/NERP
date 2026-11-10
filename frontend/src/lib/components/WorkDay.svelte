<script lang="ts">
	// The work day in the top bar: start, pause, resume, end, with the time worked today.
	import { onDestroy, onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Today = { state: 'none' | 'working' | 'paused' | 'ended'; worked_minutes?: number; started_at?: string; paused_since?: string | null };
	let t = $state<Today | null>(null);
	let open = $state(false);
	let loadedAt = $state(Date.now());
	let now = $state(Date.now());
	const load = async () => {
		try {
			t = await api.get<Today>('/hr/workday');
			loadedAt = Date.now();
		} catch {
			t = null;
		}
	};
	onMount(() => {
		load();
		// Check-in can start the day.
		window.addEventListener('nerp:workday', load);
		return () => window.removeEventListener('nerp:workday', load);
	});
	const tick = setInterval(() => (now = Date.now()), 30000);
	onDestroy(() => clearInterval(tick));
	/** Minutes worked, counting on while working. */
	const minutes = $derived((t?.worked_minutes ?? 0) + (t?.state === 'working' ? Math.floor((now - loadedAt) / 60000) : 0));
	const hm = (m: number) => `${Math.floor(m / 60)}:${String(m % 60).padStart(2, '0')}`;
	async function act(a: string) {
		try {
			t = await api.post<Today>(`/hr/workday/${a}`, {});
			loadedAt = Date.now();
			now = Date.now();
			open = false;
		} catch (e) {
			toasts.error(e);
		}
	}
	const LABEL = { none: 'Start day', working: 'Working', paused: 'On a break', ended: 'Day ended' };
	const DOT = { none: 'bg-slate-300', working: 'bg-emerald-500', paused: 'bg-amber-500', ended: 'bg-slate-400' };
</script>

{#if t}
	<div class="relative">
		{#if t.state === 'none'}
			<button class="btn btn-secondary px-3 py-1.5 text-sm" onclick={() => act('start')} data-workday="none">Start day</button>
		{:else}
			<button class="flex items-center gap-2 rounded-full px-3 py-1.5 text-sm hover:bg-slate-100" aria-haspopup="menu" aria-expanded={open} onclick={() => (open = !open)} data-workday={t.state}>
				<span class="size-2 rounded-full {DOT[t.state]}"></span>{LABEL[t.state]} <span class="text-slate-500 tabular-nums">{hm(minutes)}</span>
			</button>
			{#if open}
				<div class="absolute right-0 z-30 mt-1 w-48 rounded-xl bg-white p-1 shadow-g2" role="menu">
					{#if t.state === 'working'}
						<button class="block w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-slate-50" role="menuitem" onclick={() => act('pause')}>Take a break</button>
						<button class="block w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-slate-50" role="menuitem" onclick={() => act('end')}>End the day</button>
					{:else if t.state === 'paused'}
						<button class="block w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-slate-50" role="menuitem" onclick={() => act('resume')}>Back to work</button>
						<button class="block w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-slate-50" role="menuitem" onclick={() => act('end')}>End the day</button>
					{:else}
						<button class="block w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-slate-50" role="menuitem" onclick={() => act('start')}>Start again</button>
					{/if}
					<a class="block rounded-lg px-3 py-2 text-sm text-slate-500 hover:bg-slate-50" href="/hr/time" onclick={() => (open = false)}>My timesheet</a>
				</div>
			{/if}
		{/if}
	</div>
{/if}
