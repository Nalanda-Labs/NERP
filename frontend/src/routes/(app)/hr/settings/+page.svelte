<script lang="ts">
	// HR → Settings: working days and hours, vacation allowance (company and per person),
	// holidays.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Settings = { work_weekdays: number[]; workday_hours: string; annual_vacation_days: string };
	let s = $state<Settings | null>(null);
	let holidays = $state<{ day: string; name: string }[]>([]);
	let hday = $state({ day: '', name: '' });
	onMount(async () => {
		s = (await api.get<{ settings: Settings }>('/hr/settings')).settings;
		holidays = (await api.get<{ items: typeof holidays }>('/hr/holidays')).items;
	});
	const WEEK = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			s = (await api.patch<{ settings: Settings }>('/hr/settings', { work_weekdays: s!.work_weekdays, workday_hours: Number(s!.workday_hours), annual_vacation_days: Number(s!.annual_vacation_days) })).settings;
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function addHoliday(e: SubmitEvent) {
		e.preventDefault();
		try {
			holidays = (await api.post<{ items: typeof holidays }>('/hr/holidays', hday)).items;
			hday = { day: '', name: '' };
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeHoliday(d: string) {
		holidays = (await api.del<{ items: typeof holidays }>(`/hr/holidays/${d}`)).items;
	}

	// One person's allowance.
	let person = $state<{ id: string; name: string } | null>(null);
	let year = $state(new Date().getFullYear());
	let bal = $state<{ allowance: string; used: number; requested: number; left: string; custom: boolean } | null>(null);
	let days = $state('');
	async function pick(u: { id: string; full_name?: string; first_name?: string; last_name?: string }) {
		person = { id: u.id, name: u.full_name ?? `${u.first_name} ${u.last_name}` };
		bal = await api.get(`/hr/balance?user=${u.id}&year=${year}`);
		days = bal!.custom ? String(Number(bal!.allowance)) : '';
	}
	async function setAllowance(e: SubmitEvent) {
		e.preventDefault();
		try {
			bal = await api.put(`/hr/allowances/${person!.id}/${year}`, { days: days === '' ? null : Number(days) });
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>HR settings · NERP</title></svelte:head>

<div class="max-w-3xl space-y-5">
	<h1 class="text-2xl font-semibold text-slate-900">HR settings</h1>
	{#if s}
		<form class="card space-y-4 p-6" onsubmit={save}>
			<div>
				<span class="label">Working days</span>
				<div class="flex gap-1.5" role="group" aria-label="Working days">
					{#each WEEK as w, i (w)}<button type="button" class="rounded-lg border px-3 py-1.5 text-sm {s.work_weekdays.includes(i + 1) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600'}" aria-pressed={s.work_weekdays.includes(i + 1)} onclick={() => (s!.work_weekdays = s!.work_weekdays.includes(i + 1) ? s!.work_weekdays.filter((x) => x !== i + 1) : [...s!.work_weekdays, i + 1])}>{w}</button>{/each}
				</div>
			</div>
			<div class="grid gap-4 sm:grid-cols-2">
				<div><label class="label" for="hs-h">Hours in a working day</label><input id="hs-h" class="input" type="number" min="0.5" max="24" step="0.25" bind:value={s.workday_hours} /></div>
				<div><label class="label" for="hs-v">Vacation days a year</label><input id="hs-v" class="input" type="number" min="0" max="366" step="0.5" bind:value={s.annual_vacation_days} /></div>
			</div>
			<button class="btn btn-primary">Save</button>
		</form>
	{/if}

	<section class="card space-y-3 p-6" aria-labelledby="hol-h">
		<h2 id="hol-h" class="font-medium text-slate-900">Holidays</h2>
		<p class="text-sm text-slate-500">Not working days: not expected hours, not counted as vacation.</p>
		<ul class="divide-y divide-slate-100">{#each holidays as h (h.day)}<li class="flex items-center gap-3 py-2 text-sm" data-holiday={h.name}><span class="w-32 text-slate-600">{formatDate(h.day)}</span><span class="flex-1">{h.name}</span><button class="text-slate-400 hover:text-red-600" aria-label="Remove {h.name}" onclick={() => removeHoliday(h.day)}><Icon name="trash" class="size-4" /></button></li>{/each}</ul>
		<form class="flex gap-2" onsubmit={addHoliday}><input class="input w-44" type="date" required aria-label="Holiday date" bind:value={hday.day} /><input class="input flex-1" required maxlength="100" placeholder="Name" aria-label="Holiday name" bind:value={hday.name} /><button class="btn btn-secondary">Add</button></form>
	</section>

	<section class="card space-y-3 p-6" aria-labelledby="al-h">
		<h2 id="al-h" class="font-medium text-slate-900">One person's vacation allowance</h2>
		<div class="flex gap-2"><div class="flex-1"><UserPicker onpick={pick} placeholder="Choose an employee…" /></div><input class="input w-28" type="number" aria-label="Year" bind:value={year} onchange={() => person && pick({ id: person.id, full_name: person.name })} /></div>
		{#if person && bal}
			<form class="flex flex-wrap items-end gap-3" onsubmit={setAllowance}>
				<p class="w-full text-sm text-slate-600">{person.name}, {year}: {Number(bal.allowance)} days{bal.custom ? '' : ' (company default)'}, {bal.used} taken, {bal.requested} waiting, {Number(bal.left)} left.</p>
				<div><label class="label" for="al-d">Days</label><input id="al-d" class="input w-32" type="number" min="0" max="366" step="0.5" placeholder="Default" bind:value={days} /></div>
				<button class="btn btn-secondary">Save</button>
			</form>
		{/if}
	</section>
</div>
