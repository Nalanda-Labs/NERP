<script lang="ts">
	import { session } from '#lib/session.svelte.ts';
	import type { Recurrence } from '#lib/tasks.ts';

	let { value = $bindable<Recurrence | null>(null) }: { value?: Recurrence | null } = $props();

	const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
	const timezones: string[] = Intl.supportedValuesOf?.('timeZone') ?? ['UTC'];
	const localDate = () => new Date().toLocaleDateString('en-CA'); // YYYY-MM-DD

	function enable(freq: Recurrence['freq'] | '') {
		if (!freq) {
			value = null;
			return;
		}
		const today = new Date();
		value = {
			freq,
			interval: value?.interval ?? 1,
			weekdays: value?.weekdays?.length ? value.weekdays : [((today.getDay() + 6) % 7) + 1],
			month_day: value?.month_day ?? today.getDate(),
			time: value?.time ?? '09:00',
			timezone: value?.timezone ?? (session.user?.timezone && session.user.timezone !== 'UTC' ? session.user.timezone : Intl.DateTimeFormat().resolvedOptions().timeZone),
			starts_on: value?.starts_on ?? localDate(),
			ends_on: value?.ends_on ?? null,
			max_runs: value?.max_runs ?? null
		};
	}

	let ending = $derived<'never' | 'date' | 'count'>(value?.ends_on ? 'date' : value?.max_runs ? 'count' : 'never');
	function setEnding(e: 'never' | 'date' | 'count') {
		if (!value) return;
		value.ends_on = e === 'date' ? (value.ends_on ?? localDate()) : null;
		value.max_runs = e === 'count' ? (value.max_runs ?? 10) : null;
	}
	const unit = $derived(value ? { daily: 'day', weekly: 'week', monthly: 'month' }[value.freq] : '');
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2 text-sm">
		<label class="label mb-0" for="rec-freq">Repeat</label>
		<select id="rec-freq" class="input w-36" value={value?.freq ?? ''} onchange={(e) => enable(e.currentTarget.value as Recurrence['freq'] | '')}>
			<option value="">Never</option>
			<option value="daily">Daily</option>
			<option value="weekly">Weekly</option>
			<option value="monthly">Monthly</option>
		</select>
		{#if value}
			<span class="text-slate-500">every</span>
			<input type="number" min="1" max="366" class="input w-20" bind:value={value.interval} aria-label="Interval" />
			<span class="text-slate-500">{unit}{value.interval === 1 ? '' : 's'}</span>
		{/if}
	</div>

	{#if value}
		{#if value.freq === 'weekly'}
			<div class="flex flex-wrap gap-1.5">
				{#each WEEKDAYS as d, i (d)}
					{@const n = i + 1}
					<button
						type="button"
						class="rounded-lg border px-3 py-1.5 text-sm font-medium {value.weekdays.includes(n) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600 hover:bg-slate-50'}"
						onclick={() => (value!.weekdays = value!.weekdays.includes(n) ? value!.weekdays.filter((x) => x !== n) : [...value!.weekdays, n].sort())}
					>
						{d}
					</button>
				{/each}
			</div>
		{:else if value.freq === 'monthly'}
			<div class="flex items-center gap-2 text-sm">
				<span class="text-slate-500">on day</span>
				<input type="number" min="1" max="31" class="input w-20" bind:value={value.month_day} aria-label="Day of month" />
				<span class="text-xs text-slate-400">(31 = last day of shorter months)</span>
			</div>
		{/if}

		<div class="grid gap-3 sm:grid-cols-3">
			<div>
				<label class="label" for="rec-time">At</label>
				<input id="rec-time" type="time" class="input" bind:value={value.time} />
			</div>
			<div class="sm:col-span-2">
				<label class="label" for="rec-tz">Time zone</label>
				<select id="rec-tz" class="input" bind:value={value.timezone}>
					{#if !timezones.includes(value.timezone)}<option>{value.timezone}</option>{/if}
					{#each timezones as tz (tz)}<option>{tz}</option>{/each}
				</select>
			</div>
			<div>
				<label class="label" for="rec-start">Starting</label>
				<input id="rec-start" type="date" class="input" bind:value={value.starts_on} />
			</div>
			<div>
				<label class="label" for="rec-end">Ends</label>
				<select id="rec-end" class="input" value={ending} onchange={(e) => setEnding(e.currentTarget.value as typeof ending)}>
					<option value="never">Never</option>
					<option value="date">On a date</option>
					<option value="count">After a number of times</option>
				</select>
			</div>
			<div>
				{#if ending === 'date'}
					<label class="label" for="rec-until">Last day</label>
					<input id="rec-until" type="date" class="input" bind:value={value.ends_on} />
				{:else if ending === 'count'}
					<label class="label" for="rec-count">Times</label>
					<input id="rec-count" type="number" min="1" class="input" bind:value={value.max_runs} />
				{/if}
			</div>
		</div>
	{/if}
</div>
