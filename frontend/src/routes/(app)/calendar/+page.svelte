<script lang="ts">
	// Calendar: month and week views of my events (and invitations), optionally colleagues'
	// calendars, my task deadlines and CRM calls and meetings.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import EventEditor from '#lib/components/EventEditor.svelte';
	import CalendarSharing from '#lib/components/calendar/CalendarSharing.svelte';
	import SlotLinks from '#lib/components/calendar/SlotLinks.svelte';
	import ConnectedCalendars from '#lib/components/calendar/ConnectedCalendars.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Entry = {
		kind: 'event' | 'task' | 'activity' | 'booking';
		id: string;
		user_id: string;
		title: string;
		start: string;
		end: string;
		all_day: boolean;
		occurrence: string | null;
		recurring: boolean;
		location: string | null;
		color: string | null;
		busy: boolean;
		owner_id: string | null;
		my_status: string | null;
		link: string | null;
	};
	type Person = { id: string; name: string; avatar_file_id: string | null };

	const KEY = 'calendar.view';
	let mode = $state<'month' | 'week'>((() => { try { return (localStorage.getItem(KEY) as 'month' | 'week') ?? 'month'; } catch { return 'month'; } })());
	let cursor = $state(new Date());
	let entries = $state<Entry[]>([]);
	// Colleagues whose calendars are closed to me.
	let closed = $state<string[]>([]);
	// Colleagues who shared their calendar with me (or my departments).
	let sharedWithMe = $state<(Person & { level: string })[]>([]);
	let sharingOpen = $state(false);
	let linksOpen = $state(page.url.searchParams.get('slots') === '1');
	let accountsOpen = $state(false);
	let extras = $state(true);
	let colleagues = $state<Person[]>([]);
	// Colleagues' colours; mine is the brand colour.
	const PALETTE = ['#188038', '#e37400', '#9334e6', '#12828c', '#d93025', '#5f6368'];
	const colorOf = (e: Entry) => e.color ?? (e.kind === 'task' ? '#5f6368' : e.kind === 'activity' ? '#e37400' : e.user_id === session.user?.id ? '#1a73e8' : PALETTE[colleagues.findIndex((c) => c.id === e.user_id) % PALETTE.length]);

	const startOfWeek = (d: Date) => { const x = new Date(d.getFullYear(), d.getMonth(), d.getDate()); x.setDate(x.getDate() - ((x.getDay() + 6) % 7)); return x; };
	const addDays = (d: Date, n: number) => { const x = new Date(d); x.setDate(x.getDate() + n); return x; };
	const sameDay = (a: Date, b: Date) => a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
	const days = $derived.by(() => {
		if (mode === 'week') return Array.from({ length: 7 }, (_, i) => addDays(startOfWeek(cursor), i));
		const first = startOfWeek(new Date(cursor.getFullYear(), cursor.getMonth(), 1));
		return Array.from({ length: 42 }, (_, i) => addDays(first, i));
	});
	const title = $derived(mode === 'month' ? cursor.toLocaleDateString(undefined, { month: 'long', year: 'numeric' }) : `${days[0].toLocaleDateString(undefined, { day: 'numeric', month: 'short' })} – ${days[6].toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })}`);

	let seq = 0;
	async function load() {
		const mine = ++seq;
		const from = days[0], to = addDays(days[days.length - 1], 1);
		try {
			const r = await api.get<{ items: Entry[]; closed: string[] }>('/calendar' + query({ from: from.toISOString(), to: to.toISOString(), users: colleagues.map((c) => c.id).join(',') || undefined, extras: extras || undefined }));
			if (mine === seq) {
				entries = r.items;
				closed = r.closed ?? [];
			}
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		days;
		colleagues;
		extras;
		load();
	});
	function setMode(m: 'month' | 'week') {
		mode = m;
		try { localStorage.setItem(KEY, m); } catch { /* not remembered */ }
	}
	function move(n: number) {
		cursor = mode === 'month' ? new Date(cursor.getFullYear(), cursor.getMonth() + n, 1) : addDays(cursor, 7 * n);
	}

	/** Entries on a day (all-day ones span their days; timed ones show on their start day). */
	function on(d: Date) {
		return entries.filter((e) => {
			if (e.all_day) {
				const s = new Date(e.start.slice(0, 10) + 'T00:00'), en = new Date(e.end.slice(0, 10) + 'T00:00');
				return d >= s && d < en;
			}
			return sameDay(new Date(e.start), d);
		});
	}
	const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

	// The editor.
	let editorOpen = $state(false);
	let editing = $state<{ id: string | null; occurrence: string | null; at: Date | null }>({ id: null, occurrence: null, at: null });
	function openEntry(e: Entry) {
		if (e.link) return goto(e.link);
		if (e.busy) return;
		editing = { id: e.id, occurrence: e.occurrence, at: null };
		editorOpen = true;
	}
	function newAt(at: Date) {
		editing = { id: null, occurrence: null, at };
		editorOpen = true;
	}
	onMount(() => {
		api.get<{ items: (Person & { level: string })[] }>('/calendar/shared-with-me').then((r) => (sharedWithMe = r.items)).catch(() => {});
		// Back from Google or Microsoft sign-in.
		const connected = page.url.searchParams.get('connected'), failed = page.url.searchParams.get('error');
		if (connected || failed) {
			connected ? toasts.success(`Connected ${connected}; syncing now`) : toasts.error(failed);
			accountsOpen = true;
			goto('/calendar', { replaceState: true });
		}
		const ev = page.url.searchParams.get('event');
		if (ev) {
			editing = { id: ev, occurrence: null, at: null };
			editorOpen = true;
		}
	});

	// Subscribing other apps.
	let feedOpen = $state(false);
	let feed = $state<{ active: boolean; url?: string; webcal?: string } | null>(null);
	async function openFeed() {
		feed = await api.get('/calendar/feed');
		feedOpen = true;
	}
	async function newFeed() {
		if (feed?.active && !confirm('Make a new link? The current one stops working in apps that use it.')) return;
		feed = await api.post('/calendar/feed', {});
	}
	async function stopFeed() {
		feed = await api.del('/calendar/feed');
	}

	const HOUR = 44;
	let scroller: HTMLElement | undefined = $state();
	$effect(() => {
		if (mode === 'week' && scroller) scroller.scrollTop = 8 * HOUR;
	});
	function top(iso: string) {
		const d = new Date(iso);
		return (d.getHours() + d.getMinutes() / 60) * HOUR;
	}
	function height(e: Entry) {
		return Math.max(20, ((new Date(e.end).getTime() - new Date(e.start).getTime()) / 3600000) * HOUR);
	}
</script>

<svelte:head><title>Calendar · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="mr-2 text-2xl font-semibold text-slate-900">Calendar</h1>
		<button class="btn btn-secondary" onclick={() => (cursor = new Date())}>Today</button>
		<button class="btn btn-ghost p-2" aria-label="Previous" onclick={() => move(-1)}>‹</button>
		<button class="btn btn-ghost p-2" aria-label="Next" onclick={() => move(1)}>›</button>
		<span class="text-lg font-medium text-slate-800" data-range>{title}</span>
		<div class="ml-auto flex flex-wrap items-center gap-2">
			<label class="flex items-center gap-2 text-sm text-slate-600"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={extras} /> Tasks and CRM</label>
			<div class="flex rounded-full bg-slate-100 p-0.5" role="group" aria-label="View">
				{#each [['month', 'Month'], ['week', 'Week']] as [m, l] (m)}<button class="rounded-full px-3 py-1 text-sm font-medium {mode === m ? 'bg-white shadow-sm' : 'text-slate-600'}" aria-pressed={mode === m} onclick={() => setMode(m as 'month' | 'week')}>{l}</button>{/each}
			</div>
			<button class="btn btn-secondary" onclick={() => (sharingOpen = true)}>Sharing</button>
			<button class="btn btn-secondary" onclick={() => (linksOpen = true)}>Booking links</button>
			<button class="btn btn-secondary" onclick={() => (accountsOpen = true)}>Connected calendars</button>
			<button class="btn btn-secondary" onclick={openFeed}><Icon name="calendar" class="size-4" /> Subscribe</button>
			<button class="btn btn-primary" onclick={() => newAt(new Date(Math.ceil(Date.now() / 1800000) * 1800000))}><Icon name="plus" class="size-4" /> New event</button>
		</div>
	</div>
	<div class="max-w-xl space-y-1.5">
		<MultiUserPicker bind:value={colleagues} exclude={session.user ? [session.user.id] : []} placeholder="Show colleagues' calendars…" />
		{#if sharedWithMe.some((p) => !colleagues.some((c) => c.id === p.id))}
			<p class="flex flex-wrap items-center gap-1.5 text-xs text-slate-500">Shared with you:
				{#each sharedWithMe.filter((p) => !colleagues.some((c) => c.id === p.id)) as p (p.id)}
					<button class="rounded-full border border-slate-200 px-2 py-0.5 text-slate-700 hover:border-brand-400" onclick={() => (colleagues = [...colleagues, p])}>+ {p.name}</button>
				{/each}
			</p>
		{/if}
		{#if closed.length}<p class="text-xs text-amber-700" data-closed>{colleagues.filter((c) => closed.includes(c.id)).map((c) => c.name).join(', ')} {closed.length === 1 ? "doesn't" : "don't"} share their calendar with you.</p>{/if}
	</div>

	{#if mode === 'month'}
		<div class="card overflow-hidden">
			<div class="grid grid-cols-7 border-b border-slate-100 text-center text-xs font-medium text-slate-500">
				{#each ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as w (w)}<div class="py-2">{w}</div>{/each}
			</div>
			<div class="grid grid-cols-7">
				{#each days as d (d.getTime())}
					{@const list = on(d)}
					<div class="min-h-28 border-r border-b border-slate-100 p-1 {d.getMonth() !== cursor.getMonth() ? 'bg-slate-50/60' : ''}" data-day={d.getDate()}>
						<button class="mb-0.5 flex size-6 items-center justify-center rounded-full text-xs {sameDay(d, new Date()) ? 'bg-brand-600 font-semibold text-white' : 'text-slate-600 hover:bg-slate-100'}" aria-label="New event on {d.toDateString()}" onclick={() => newAt(new Date(d.getFullYear(), d.getMonth(), d.getDate(), 9))}>{d.getDate()}</button>
						{#each list.slice(0, 4) as e (e.kind + e.id + e.start + e.user_id)}
							<button class="mb-0.5 block w-full truncate rounded px-1.5 py-0.5 text-left text-xs {e.all_day ? 'text-white' : 'text-slate-800 hover:bg-slate-100'} {e.my_status === 'declined' ? 'line-through opacity-60' : ''}" style={e.all_day ? `background:${colorOf(e)}` : ''} data-entry={e.title} onclick={() => openEntry(e)}>
								{#if !e.all_day}<span class="mr-1 inline-block size-2 rounded-full" style="background:{colorOf(e)}"></span><span class="text-slate-500">{time(e.start)}</span>{/if}
								{e.title}{#if e.my_status === 'pending'} <span class="text-amber-700">?</span>{/if}
							</button>
						{/each}
						{#if list.length > 4}<button class="px-1.5 text-xs text-slate-500 hover:underline" onclick={() => ((cursor = d), setMode('week'))}>+{list.length - 4} more</button>{/if}
					</div>
				{/each}
			</div>
		</div>
	{:else}
		<div class="card overflow-hidden">
			<div class="grid grid-cols-[3.5rem_repeat(7,minmax(0,1fr))] border-b border-slate-100">
				<div></div>
				{#each days as d (d.getTime())}
					<div class="border-l border-slate-100 px-1 py-2 text-center">
						<p class="text-xs text-slate-500">{d.toLocaleDateString(undefined, { weekday: 'short' })}</p>
						<p class="text-lg {sameDay(d, new Date()) ? 'font-semibold text-brand-700' : 'text-slate-800'}">{d.getDate()}</p>
						{#each on(d).filter((e) => e.all_day) as e (e.kind + e.id + e.start + e.user_id)}
							<button class="mt-0.5 block w-full truncate rounded px-1 text-left text-xs text-white" style="background:{colorOf(e)}" data-entry={e.title} onclick={() => openEntry(e)}>{e.title}</button>
						{/each}
					</div>
				{/each}
			</div>
			<div bind:this={scroller} class="relative max-h-[65vh] overflow-y-auto">
				<div class="grid grid-cols-[3.5rem_repeat(7,minmax(0,1fr))]" style="height:{24 * HOUR}px">
					<div class="relative">{#each Array(24) as _, h (h)}<span class="absolute right-2 -translate-y-1/2 text-[10px] text-slate-400" style="top:{h * HOUR}px">{h ? `${String(h).padStart(2, '0')}:00` : ''}</span>{/each}</div>
					{#each days as d (d.getTime())}
						<div class="relative border-l border-slate-100" role="presentation" ondblclick={(ev) => { const y = ev.offsetY; newAt(new Date(d.getFullYear(), d.getMonth(), d.getDate(), Math.floor(y / HOUR), Math.round(((y % HOUR) / HOUR) * 2) * 30)); }}>
							{#each Array(24) as _, h (h)}<div class="absolute inset-x-0 border-t border-slate-50" style="top:{h * HOUR}px"></div>{/each}
							{#each on(d).filter((e) => !e.all_day) as e (e.kind + e.id + e.start + e.user_id)}
								<button class="absolute inset-x-0.5 overflow-hidden rounded-md px-1.5 py-0.5 text-left text-xs text-white shadow-sm {e.my_status === 'declined' ? 'opacity-50' : ''}" style="top:{top(e.start)}px;height:{height(e)}px;background:{colorOf(e)}" data-entry={e.title} onclick={() => openEntry(e)}>
									<span class="font-medium">{e.title}</span> <span class="opacity-80">{time(e.start)}</span>
								</button>
							{/each}
						</div>
					{/each}
				</div>
			</div>
		</div>
	{/if}
</div>

<EventEditor bind:open={editorOpen} eventId={editing.id} occurrence={editing.occurrence} startAt={editing.at} onchanged={load} />
<CalendarSharing bind:open={sharingOpen} />
<SlotLinks bind:open={linksOpen} />
<ConnectedCalendars bind:open={accountsOpen} onchanged={load} />

<Modal bind:open={feedOpen} title="Use your calendar in other apps">
	<div class="space-y-3 text-sm text-slate-600">
		<div class="space-y-1 rounded-xl bg-slate-50 p-3" data-caldav>
			<p class="font-medium text-slate-800">Two-way sync (CalDAV)</p>
			<p>Apple Calendar, Thunderbird, DAVx5 on Android and other CalDAV apps can show and change your events. Use the server <code class="rounded bg-white px-1 text-slate-800">{typeof location !== 'undefined' ? location.origin : ''}/dav/</code>, your email, and an <a class="text-brand-600 hover:underline" href="/profile">app password from your profile</a>.</p>
		</div>
		<p class="font-medium text-slate-800">Read-only link</p>
		<p>Add your NERP calendar to Google Calendar, Outlook or your phone with a private link. It shows your events and CRM calls and meetings, and refreshes on its own (how often depends on the app).</p>
		{#if feed?.url}
			<div class="space-y-1"><p class="font-medium text-slate-800">Your link (shown once; keep it private):</p><code class="block rounded-lg bg-slate-50 px-3 py-2 text-xs break-all text-slate-800" data-feed>{feed.url}</code></div>
			<p>Google Calendar: Other calendars → + → From URL. Outlook: Add calendar → Subscribe from web. Apple: <a class="text-brand-600 hover:underline" href={feed.webcal}>open in Calendar</a>.</p>
		{:else if feed?.active}
			<p>You have a link already. For security it isn't shown again; make a new one to see it (the old one stops working).</p>
		{/if}
	</div>
	{#snippet footer()}
		{#if feed?.active}<button class="btn btn-ghost mr-auto hover:text-red-600" onclick={stopFeed}>Turn the link off</button>{/if}
		<button class="btn btn-secondary" onclick={() => (feedOpen = false)}>Close</button>
		<button class="btn btn-primary" onclick={newFeed}>{feed?.active ? 'Make a new link' : 'Make my link'}</button>
	{/snippet}
</Modal>
