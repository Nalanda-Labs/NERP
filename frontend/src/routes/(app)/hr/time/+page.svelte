<script lang="ts">
	// HR → Working time: my month, and (for managers) the team's, with corrections.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { api } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type Day = { id: string; day: string; started_at: string; ended_at: string | null; note: string | null; edit_reason: string | null; edited_by_name: string | null; worked_minutes: number; break_minutes: number };
	type Sheet = { user_id: string; name: string; month: string; days: Day[]; worked_minutes: number; expected_minutes: number; absence_days: number; can_edit: boolean };
	type Row = { user_id: string; name: string; worked_minutes: number; expected_minutes: number; days: number; absence_days: number };

	const thisMonth = new Date().toISOString().slice(0, 7);
	let month = $state(page.url.searchParams.get('month') ?? thisMonth);
	const who = $derived(page.url.searchParams.get('user'));
	let sheet = $state<Sheet | null>(null);
	let team = $state<Row[]>([]);
	let tab = $state<'me' | 'team'>(page.url.searchParams.has('user') ? 'team' : 'me');

	async function load() {
		try {
			sheet = await api.get<Sheet>(`/hr/timesheet?month=${month}${who ? `&user=${who}` : ''}`);
		} catch (e) {
			toasts.error(e);
		}
		team = (await api.get<{ items: Row[] }>(`/hr/team?month=${month}`).catch(() => ({ items: [] }))).items;
	}
	$effect(() => {
		month;
		who;
		load();
	});
	const hm = (m: number) => `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`;
	const time = (iso: string | null) => (iso ? new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '—');
	function shift(n: number) {
		const [y, m] = month.split('-').map(Number);
		const d = new Date(y, m - 1 + n, 1);
		month = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
	}
	const open = (id: string | null) => goto(id ? `/hr/time?user=${id}&month=${month}` : `/hr/time?month=${month}`, { replace: true });

	// Corrections.
	let editOpen = $state(false);
	let editing = $state<Day | null>(null);
	let form = $state({ day: '', start: '', end: '', reason: '' });
	const local = (iso: string | null) => (iso ? new Date(new Date(iso).getTime() - new Date(iso).getTimezoneOffset() * 60000).toISOString().slice(0, 16) : '');
	function edit(d: Day | null) {
		editing = d;
		form = d ? { day: d.day, start: local(d.started_at), end: local(d.ended_at), reason: '' } : { day: '', start: '', end: '', reason: '' };
		editOpen = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		const body = { user_id: sheet!.user_id, day: editing ? undefined : form.start.slice(0, 10), started_at: new Date(form.start).toISOString(), ended_at: form.end ? new Date(form.end).toISOString() : null, reason: form.reason };
		try {
			sheet = { ...(editing ? await api.patch<Sheet>(`/hr/workdays/${editing.id}`, body) : await api.post<Sheet>('/hr/workdays', body)), can_edit: true };
			editOpen = false;
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>Working time · NERP</title></svelte:head>

<div class="max-w-5xl space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Working time</h1>
		<button class="btn btn-ghost p-2" aria-label="Previous month" onclick={() => shift(-1)}>‹</button>
		<input class="input w-40" type="month" aria-label="Month" bind:value={month} />
		<button class="btn btn-ghost p-2" aria-label="Next month" onclick={() => shift(1)}>›</button>
	</div>
	{#if team.length}
		<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" style="width:fit-content">
			<button role="tab" aria-selected={tab === 'me'} class="rounded-full px-4 py-1.5 font-medium {tab === 'me' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => ((tab = 'me'), open(null))}>{who ? sheet?.name : 'Me'}</button>
			<button role="tab" aria-selected={tab === 'team'} class="rounded-full px-4 py-1.5 font-medium {tab === 'team' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = 'team')}>My team</button>
		</div>
	{/if}

	{#if tab === 'team'}
		<div class="card overflow-x-auto">
			<table class="min-w-full text-sm">
				<thead class="border-b border-slate-100 text-left text-xs text-slate-500"><tr><th class="px-4 py-3">Person</th><th class="px-4 py-3 text-right">Days</th><th class="px-4 py-3 text-right">Worked</th><th class="px-4 py-3 text-right">Expected</th><th class="px-4 py-3 text-right">Away</th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each team as r (r.user_id)}
						<tr class="cursor-pointer hover:bg-slate-50" data-person={r.name} onclick={() => ((tab = 'me'), open(r.user_id))}>
							<td class="px-4 py-2.5 font-medium text-slate-900">{r.name}</td>
							<td class="px-4 py-2.5 text-right tabular-nums">{r.days}</td>
							<td class="px-4 py-2.5 text-right tabular-nums {r.worked_minutes < r.expected_minutes ? 'text-amber-800' : ''}">{hm(r.worked_minutes)}</td>
							<td class="px-4 py-2.5 text-right text-slate-500 tabular-nums">{hm(r.expected_minutes)}</td>
							<td class="px-4 py-2.5 text-right text-slate-500 tabular-nums">{r.absence_days ? `${r.absence_days} d` : ''}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{:else if sheet}
		<div class="grid gap-4 sm:grid-cols-3">
			<div class="card p-4"><p class="text-xs text-slate-500">Worked</p><p class="text-2xl font-semibold tabular-nums" data-worked>{hm(sheet.worked_minutes)}</p></div>
			<div class="card p-4"><p class="text-xs text-slate-500">Expected</p><p class="text-2xl font-semibold text-slate-600 tabular-nums">{hm(sheet.expected_minutes)}</p></div>
			<div class="card p-4"><p class="text-xs text-slate-500">Time off (working days)</p><p class="text-2xl font-semibold text-slate-600 tabular-nums">{sheet.absence_days}</p></div>
		</div>
		<div class="card overflow-x-auto">
			<div class="flex items-center border-b border-slate-100 px-4 py-3"><h2 class="flex-1 font-medium">{sheet.name}</h2>{#if sheet.can_edit}<button class="btn btn-secondary px-3 py-1 text-sm" onclick={() => edit(null)}>Add a day</button>{/if}</div>
			<table class="min-w-full text-sm">
				<thead class="text-left text-xs text-slate-500"><tr><th class="px-4 py-2">Day</th><th class="px-4 py-2">Started</th><th class="px-4 py-2">Ended</th><th class="px-4 py-2 text-right">Breaks</th><th class="px-4 py-2 text-right">Worked</th><th class="px-4 py-2">Notes</th><th></th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each sheet.days as d (d.id)}
						<tr data-day={d.day}>
							<td class="px-4 py-2 whitespace-nowrap">{new Date(d.day + 'T00:00').toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })}</td>
							<td class="px-4 py-2 tabular-nums">{time(d.started_at)}</td>
							<td class="px-4 py-2 tabular-nums">{d.ended_at ? time(d.ended_at) : 'still working'}</td>
							<td class="px-4 py-2 text-right text-slate-500 tabular-nums">{d.break_minutes ? `${d.break_minutes} min` : ''}</td>
							<td class="px-4 py-2 text-right font-medium tabular-nums">{hm(d.worked_minutes)}</td>
							<td class="px-4 py-2 text-xs text-slate-500">{[d.note, d.edit_reason ? `Corrected by ${d.edited_by_name}: ${d.edit_reason}` : null].filter(Boolean).join(' · ')}</td>
							<td class="px-2">{#if sheet.can_edit}<button class="text-xs text-brand-600 hover:underline" onclick={() => edit(d)}>Correct</button>{/if}</td>
						</tr>
					{:else}
						<tr><td colspan="7" class="px-4 py-10 text-center text-slate-500">No days recorded this month. Use “Start day” at the top when you begin work.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<Modal bind:open={editOpen} title={editing ? 'Correct a day' : 'Add a day'}>
	<form id="wd" class="space-y-3" onsubmit={save}>
		<div class="grid gap-3 sm:grid-cols-2">
			<div><label class="label" for="wd-s">Started</label><input id="wd-s" class="input" type="datetime-local" required bind:value={form.start} /></div>
			<div><label class="label" for="wd-e">Ended</label><input id="wd-e" class="input" type="datetime-local" bind:value={form.end} /></div>
		</div>
		<div><label class="label" for="wd-r">Why</label><input id="wd-r" class="input" required maxlength="500" placeholder="Forgot to end the day" bind:value={form.reason} /></div>
		<p class="text-xs text-slate-500">The reason is kept with the day and in the audit log.</p>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (editOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="wd">Save</button>
	{/snippet}
</Modal>
