<script lang="ts">
	// HR → Check-in: today's check-in, a department's day (who works where, who isn't and why,
	// who hasn't checked in) and month, my own check-ins, and the company settings.
	import { onMount, untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { ABSENCE, locationOf, mapLink, summary, type CheckIn, type CheckInToday } from '#lib/checkin.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import CheckInDialog from '#lib/components/CheckInDialog.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Dept = { id: string; name: string; parent_id: string | null };
	type Person = { user_id: string; name: string; avatar_file_id: string | null; position: string | null; checkin: CheckIn | null; absence: string | null };
	type DayView = { department: { id: string; name: string }; day: string; people: Person[]; totals: { people: number; working: number; not_working: number; no_checkin: number } };
	type MonthRow = { user_id: string; name: string; avatar_file_id: string | null; position: string | null; working: number; not_working: number; days: { day: string; status: string; location: string | null }[] };
	type MonthView = { department: { id: string; name: string }; month: string; working_days: number; people: MonthRow[] };

	const ymd = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
	let today = $state<CheckInToday | null>(null);
	let depts = $state<Dept[]>([]);
	let dept = $state('');
	let tab = $state<'day' | 'month' | 'mine'>('day');
	let day = $state(ymd(new Date()));
	let month = $state(ymd(new Date()).slice(0, 7));
	let dayView = $state<DayView | null>(null);
	let monthView = $state<MonthView | null>(null);
	let mine = $state<{ items: CheckIn[]; working_days: number } | null>(null);
	let dialogOpen = $state(false);

	async function loadToday() {
		today = await api.get<CheckInToday>('/checkin/today');
	}
	async function start() {
		try {
			await loadToday();
			depts = (await api.get<{ items: Dept[] }>('/checkin/departments')).items;
			const remembered = today?.prefs.department_id;
			dept = depts.find((d) => d.id === remembered)?.id ?? depts[0]?.id ?? '';
			if (!depts.length) tab = 'mine';
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(() => {
		start();
		return realtime.on('checkin.changed', () => (loadToday(), load()));
	});

	async function load() {
		try {
			if (tab === 'day' && dept) dayView = await api.get<DayView>(`/checkin/departments/${dept}/day?day=${day}`);
			if (tab === 'month' && dept) monthView = await api.get<MonthView>(`/checkin/departments/${dept}/month?month=${month}`);
			if (tab === 'mine') mine = await api.get<{ items: CheckIn[]; working_days: number }>(`/checkin/history?month=${month}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		tab;
		dept;
		day;
		month;
		untrack(load);
	});
	// Remember the department looked at.
	function pick(id: string) {
		dept = id;
		api.patch('/checkin/prefs', { department_id: id }).catch(() => {});
	}
	function shiftDay(n: number) {
		const [y, m, d] = day.split('-').map(Number);
		day = ymd(new Date(y, m - 1, d + n));
	}
	function shiftMonth(n: number) {
		const [y, m] = month.split('-').map(Number);
		month = ymd(new Date(y, m - 1 + n, 1)).slice(0, 7);
	}
	// After the dialog changed today's check-in, the top bar catches up.
	$effect(() => {
		if (!dialogOpen && today) untrack(() => window.dispatchEvent(new CustomEvent('nerp:checkin')));
	});

	const groups = $derived.by(() => {
		const p = dayView?.people ?? [];
		return [
			{ key: 'working', title: 'Working', people: p.filter((x) => x.checkin?.status === 'working') },
			{ key: 'not_working', title: 'Not working', people: p.filter((x) => x.checkin && x.checkin.status !== 'working') },
			{ key: 'away', title: 'On time off', people: p.filter((x) => !x.checkin && x.absence) },
			{ key: 'none', title: 'Not checked in', people: p.filter((x) => !x.checkin && !x.absence) }
		].filter((g) => g.people.length);
	});
	const monthDays = $derived.by(() => {
		const [y, m] = month.split('-').map(Number);
		const n = new Date(y, m, 0).getDate();
		return Array.from({ length: n }, (_, i) => ({ day: `${month}-${String(i + 1).padStart(2, '0')}`, n: i + 1, weekend: [0, 6].includes(new Date(y, m - 1, i + 1).getDay()) }));
	});
	const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

	// Settings (HR managers).
	let settingsOpen = $state(false);
	let settings = $state({ enabled: true, geo_enabled: true });
	async function saveSettings() {
		try {
			settings = await api.patch('/checkin/settings', settings);
			settingsOpen = false;
			await loadToday();
			window.dispatchEvent(new CustomEvent('nerp:checkin'));
		} catch (e) {
			toasts.error(e);
		}
	}
	async function setStartWorkday(on: boolean) {
		try {
			today = await api.patch<CheckInToday>('/checkin/prefs', { start_workday: on });
		} catch (e) {
			toasts.error(e);
		}
	}
	async function unmute() {
		today = await api.patch<CheckInToday>('/checkin/prefs', { mute: 'off' });
		window.dispatchEvent(new CustomEvent('nerp:checkin'));
	}
</script>

<svelte:head><title>Check-in · NERP</title></svelte:head>

<div class="max-w-6xl space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Check-in</h1>
		{#if today?.can_manage}<button class="btn btn-ghost text-sm" onclick={() => ((settings = { enabled: today!.enabled, geo_enabled: today!.geo_enabled }), (settingsOpen = true))}>Settings</button>{/if}
	</div>

	{#if today}
		<div class="card flex flex-wrap items-center gap-4 p-4" data-today>
			{#if !today.enabled}
				<p class="flex-1 text-sm text-slate-600">Check-in is turned off for the company.</p>
			{:else if today.checkin}
				<div class="flex-1">
					<p class="text-xs text-slate-500">Today</p>
					<p class="text-lg font-medium text-slate-900" data-my-summary>{summary(today.checkin)}</p>
				</div>
				<button class="btn btn-secondary" onclick={() => (dialogOpen = true)}>Open</button>
			{:else}
				<div class="flex-1">
					<p class="text-lg font-medium text-slate-900">Where are you working today?</p>
					<p class="text-sm text-slate-500">
						{#if !today.working_day}Not a working day for you, but you can still check in.{:else if today.muted}Reminders are off. <button class="text-brand-600 hover:underline" onclick={unmute}>Turn them back on</button>{:else}Your team sees it, and you can tell a chat.{/if}
					</p>
				</div>
				<button class="btn btn-primary" onclick={() => (dialogOpen = true)}>Check in</button>
			{/if}
			{#if today.enabled}
				<label class="flex w-full items-center gap-2 border-t border-slate-100 pt-3 text-sm text-slate-600">
					<input type="checkbox" checked={today.prefs.start_workday} onchange={(e) => setStartWorkday(e.currentTarget.checked)} /> Start my work day when I check in as working
				</label>
			{/if}
		</div>
	{/if}

	<div class="flex flex-wrap items-center gap-3">
		<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist">
			{#if depts.length}
				<button role="tab" aria-selected={tab === 'day'} class="rounded-full px-4 py-1.5 font-medium {tab === 'day' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = 'day')}>Department</button>
				<button role="tab" aria-selected={tab === 'month'} class="rounded-full px-4 py-1.5 font-medium {tab === 'month' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = 'month')}>Month</button>
			{/if}
			<button role="tab" aria-selected={tab === 'mine'} class="rounded-full px-4 py-1.5 font-medium {tab === 'mine' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = 'mine')}>My check-ins</button>
		</div>
		<div class="flex-1"></div>
		{#if tab !== 'mine' && depts.length > 1}
			<select class="input w-56" aria-label="Department" value={dept} onchange={(e) => pick(e.currentTarget.value)}>
				{#each depts as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
			</select>
		{/if}
		{#if tab === 'day'}
			<button class="btn btn-ghost px-2" aria-label="Previous day" onclick={() => shiftDay(-1)}>‹</button>
			<input class="input w-40" type="date" aria-label="Day" value={day} onchange={(e) => e.currentTarget.value && (day = e.currentTarget.value)} />
			<button class="btn btn-ghost px-2" aria-label="Next day" onclick={() => shiftDay(1)}>›</button>
		{:else}
			<button class="btn btn-ghost px-2" aria-label="Previous month" onclick={() => shiftMonth(-1)}>‹</button>
			<input class="input w-40" type="month" aria-label="Month" value={month} onchange={(e) => e.currentTarget.value && (month = e.currentTarget.value)} />
			<button class="btn btn-ghost px-2" aria-label="Next month" onclick={() => shiftMonth(1)}>›</button>
		{/if}
	</div>

	{#if tab === 'day' && dayView}
		<div class="grid gap-3 sm:grid-cols-4">
			<div class="card p-4"><p class="text-xs text-slate-500">People</p><p class="text-2xl font-semibold tabular-nums">{dayView.totals.people}</p></div>
			<div class="card p-4"><p class="text-xs text-slate-500">Working</p><p class="text-2xl font-semibold text-emerald-700 tabular-nums" data-total="working">{dayView.totals.working}</p></div>
			<div class="card p-4"><p class="text-xs text-slate-500">Not working</p><p class="text-2xl font-semibold text-amber-700 tabular-nums" data-total="not_working">{dayView.totals.not_working}</p></div>
			<div class="card p-4"><p class="text-xs text-slate-500">Not checked in</p><p class="text-2xl font-semibold text-slate-500 tabular-nums" data-total="none">{dayView.totals.no_checkin}</p></div>
		</div>
		{#each groups as g (g.key)}
			<section class="card" data-group={g.key}>
				<h2 class="border-b border-slate-100 px-4 py-3 font-medium">{g.title} <span class="text-slate-400">{g.people.length}</span></h2>
				<ul class="divide-y divide-slate-50">
					{#each g.people as p (p.user_id)}
						<li class="flex items-start gap-3 px-4 py-3" data-person={p.name}>
							<Avatar name={p.name} fileId={p.avatar_file_id} size="sm" />
							<div class="min-w-0 flex-1">
								<p class="font-medium text-slate-900">{p.name} {#if p.position}<span class="text-sm font-normal text-slate-500">· {p.position}</span>{/if}</p>
								{#if p.checkin}
									<p class="text-sm {p.checkin.status === 'working' ? 'text-slate-700' : 'text-amber-800'}" data-status={p.checkin.status}>
										{#if p.checkin.status === 'working'}{locationOf(p.checkin.location).icon}{/if} {summary(p.checkin)}
									</p>
									{#if p.checkin.message}<p class="text-sm whitespace-pre-line text-slate-500">{p.checkin.message}</p>{/if}
									<p class="text-xs text-slate-400">
										{time(p.checkin.created_at)}
										{#if p.checkin.latitude != null && p.checkin.longitude != null} · <a class="text-brand-600 hover:underline" target="_blank" rel="noopener" href={mapLink(p.checkin.latitude, p.checkin.longitude)}>📍 {p.checkin.address ?? 'on the map'}</a>{:else if p.checkin.address} · 📍 {p.checkin.address}{/if}
									</p>
								{:else if p.absence}
									<p class="text-sm text-slate-500">{ABSENCE[p.absence] ?? 'Away'}</p>
								{/if}
							</div>
						</li>
					{/each}
				</ul>
			</section>
		{:else}
			<p class="card px-4 py-10 text-center text-slate-500">Nobody in this department yet.</p>
		{/each}
	{:else if tab === 'month' && monthView}
		<div class="card overflow-x-auto">
			<table class="min-w-full text-xs">
				<thead class="border-b border-slate-100 text-slate-500">
					<tr>
						<th class="sticky left-0 bg-white px-3 py-2 text-left">Person</th>
						{#each monthDays as d (d.day)}<th class="w-6 px-0.5 py-2 text-center font-normal {d.weekend ? 'text-slate-300' : ''}">{d.n}</th>{/each}
						<th class="px-3 py-2 text-right" title="Days checked in as working">Working</th>
						<th class="px-3 py-2 text-right">Not</th>
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-50">
					{#each monthView.people as p (p.user_id)}
						<tr data-person={p.name}>
							<td class="sticky left-0 bg-white px-3 py-2 text-sm font-medium whitespace-nowrap text-slate-900">{p.name}</td>
							{#each monthDays as d (d.day)}
								{@const k = p.days.find((x) => x.day === d.day)}
								<td class="px-0.5 py-2 text-center {d.weekend ? 'bg-slate-50' : ''}">
									{#if k}<span
											class="inline-block size-4 rounded {k.status === 'working' ? 'bg-emerald-400' : 'bg-amber-300'}"
											title="{d.day}: {k.status === 'working' ? locationOf(k.location as never).full : 'Not working'}"
											data-cell={d.day}
										></span>{/if}
								</td>
							{/each}
							<td class="px-3 py-2 text-right text-sm tabular-nums" data-working>{p.working}</td>
							<td class="px-3 py-2 text-right text-sm text-slate-500 tabular-nums">{p.not_working || ''}</td>
						</tr>
					{/each}
				</tbody>
			</table>
			<p class="border-t border-slate-100 px-4 py-2 text-xs text-slate-500">
				<span class="mr-1 inline-block size-2.5 rounded bg-emerald-400"></span>working · <span class="mr-1 ml-2 inline-block size-2.5 rounded bg-amber-300"></span>not working · {monthView.working_days} working days this month
			</p>
		</div>
	{:else if tab === 'mine' && mine}
		<div class="card">
			<h2 class="border-b border-slate-100 px-4 py-3 font-medium">Working {mine.working_days} {mine.working_days === 1 ? 'day' : 'days'} this month</h2>
			<ul class="divide-y divide-slate-50">
				{#each mine.items as k (k.id)}
					<li class="flex gap-4 px-4 py-2.5 text-sm" data-mine={k.day}>
						<span class="w-28 text-slate-500">{new Date(k.day + 'T00:00').toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })}</span>
						<span class="flex-1 {k.status === 'working' ? '' : 'text-amber-800'}">{k.status === 'working' ? locationOf(k.location).icon : '🚫'} {summary(k)}{k.message ? ` — ${k.message}` : ''}</span>
						<span class="text-xs text-slate-400">{k.chat_title ?? ''}</span>
					</li>
				{:else}
					<li class="px-4 py-10 text-center text-slate-500">No check-ins this month.</li>
				{/each}
			</ul>
		</div>
	{/if}
</div>

<CheckInDialog bind:open={dialogOpen} bind:today />

<Modal bind:open={settingsOpen} title="Check-in settings">
	<div class="space-y-3">
		<label class="flex items-start gap-2 text-sm"><input type="checkbox" class="mt-0.5" bind:checked={settings.enabled} /> <span>Check-in is on<span class="block text-xs text-slate-500">People say each day where they work, or that they aren't working.</span></span></label>
		<label class="flex items-start gap-2 text-sm"><input type="checkbox" class="mt-0.5" bind:checked={settings.geo_enabled} /> <span>People may share where they are<span class="block text-xs text-slate-500">Their browser's location and an address, seen by their managers and the chat they tell.</span></span></label>
	</div>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (settingsOpen = false)}>Cancel</button>
		<button class="btn btn-primary" onclick={saveSettings}>Save</button>
	{/snippet}
</Modal>
