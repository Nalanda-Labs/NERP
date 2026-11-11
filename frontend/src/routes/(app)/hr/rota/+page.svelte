<script lang="ts">
	// Shift rota: a week of who works which shift, with late starts marked; managers change
	// it cell by cell or fill a person's weekdays; HR sets up the shifts.
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Shift = { id: string; name: string; start_min: number; end_min: number; break_minutes: number; color: string; is_active: boolean };
	type Cell = { user_id: string; day: string; shift_id: string; started_at: string | null; late: boolean; late_minutes: number | null };
	const iso = (d: Date) => d.toISOString().slice(0, 10);
	const monday = (d: Date) => { const x = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate())); x.setUTCDate(x.getUTCDate() - ((x.getUTCDay() + 6) % 7)); return x; };
	let start = $state(monday(new Date()));
	const days = $derived(Array.from({ length: 7 }, (_, i) => iso(new Date(+start + i * 86400e3))));
	let department = $state('');
	let departments = $state<{ id: string; name: string }[]>([]);
	let shifts = $state<Shift[]>([]);
	let people = $state<{ id: string; name: string; can_edit: boolean }[]>([]);
	let cells = $state<Cell[]>([]);
	const hr = $derived(session.can(PERM.HR));
	const hm = (m: number) => `${String(Math.floor(m / 60) % 24).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;

	async function load() {
		try {
			const r = await api.get<{ shifts: Shift[]; people: typeof people; cells: Cell[] }>('/hr/rota' + query({ from: days[0], to: days[6], department_id: department || undefined }));
			shifts = r.shifts;
			people = r.people;
			cells = r.cells;
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		days;
		department;
		load();
	});
	onMount(() => api.get<{ id: string; name: string }[]>('/departments').then((d) => (departments = d)).catch(() => {}));
	const cell = (u: string, d: string) => cells.find((c) => c.user_id === u && c.day === d);
	async function setCell(user_id: string, day: string, shift_id: string) {
		try {
			await api.put('/hr/rota', { entries: [{ user_id, day, shift_id: shift_id || null }] });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function fillWeek(user_id: string) {
		const s = prompt(`Shift for every weekday this week (${shifts.filter((x) => x.is_active).map((x) => x.name).join(', ')}):`);
		const shift = shifts.find((x) => x.name.toLowerCase() === s?.trim().toLowerCase());
		if (!shift) return;
		await api.put('/hr/rota', { entries: [{ user_id, from: days[0], to: days[6], weekdays: [1, 2, 3, 4, 5], shift_id: shift.id }] });
		load();
	}
	// Setting up shifts (HR).
	let form = $state({ name: '', start: '09:00', end: '17:00', break_minutes: 30, color: '#1a73e8' });
	const mins = (s: string) => { const [h, m] = s.split(':').map(Number); return h * 60 + m; };
	async function addShift(e: SubmitEvent) {
		e.preventDefault();
		const s = mins(form.start);
		let en = mins(form.end);
		if (en <= s) en += 1440;
		try {
			shifts = (await api.post<{ items: Shift[] }>('/hr/shifts', { name: form.name, start_min: s, end_min: en, break_minutes: Number(form.break_minutes), color: form.color })).items;
			form.name = '';
		} catch (err) {
			toasts.error(err);
		}
	}
	async function dropShift(s: Shift) {
		if (!confirm(`Delete the shift “${s.name}”? It goes off the rota too.`)) return;
		shifts = (await api.del<{ items: Shift[] }>(`/hr/shifts/${s.id}`)).items;
		load();
	}
</script>

<svelte:head><title>Shift rota · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="mr-2 text-2xl font-semibold text-slate-900">Shift rota</h1>
		<button class="btn btn-ghost p-2" aria-label="Previous week" onclick={() => (start = new Date(+start - 7 * 86400e3))}>‹</button>
		<button class="btn btn-ghost p-2" aria-label="Next week" onclick={() => (start = new Date(+start + 7 * 86400e3))}>›</button>
		<span class="font-medium text-slate-800">{new Date(days[0]).toLocaleDateString()} – {new Date(days[6]).toLocaleDateString()}</span>
		<select class="input ml-auto w-56" aria-label="Department" bind:value={department}><option value="">People I manage</option>{#each departments as d (d.id)}<option value={d.id}>{d.name}</option>{/each}</select>
	</div>
	<div class="card overflow-x-auto">
		<table class="w-full text-sm">
			<thead><tr class="text-left text-xs text-slate-500"><th class="px-3 py-2"></th>{#each days as d (d)}<th class="px-2 py-2">{new Date(d).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' })}</th>{/each}<th></th></tr></thead>
			<tbody class="divide-y divide-slate-100">
				{#each people as p (p.id)}
					<tr data-person={p.name}>
						<td class="px-3 py-2 font-medium whitespace-nowrap">{p.name}</td>
						{#each days as d (d)}
							{@const c = cell(p.id, d)}
							{@const s = shifts.find((x) => x.id === c?.shift_id)}
							<td class="px-1 py-1">
								{#if p.can_edit}
									<select class="input py-1 text-xs" style={s ? `border-color:${s.color}` : ''} aria-label="{p.name} on {d}" value={c?.shift_id ?? ''} onchange={(e) => setCell(p.id, d, e.currentTarget.value)}>
										<option value="">—</option>{#each shifts.filter((x) => x.is_active || x.id === c?.shift_id) as x (x.id)}<option value={x.id}>{x.name}</option>{/each}
									</select>
								{:else if s}<span class="rounded px-1.5 py-0.5 text-xs text-white" style="background:{s.color}">{s.name}</span>{/if}
								{#if c?.late}<span class="block text-[10px] text-red-600" data-late>late {c.late_minutes} min</span>{/if}
							</td>
						{/each}
						<td class="px-2">{#if p.can_edit}<button class="text-xs text-brand-600 hover:underline" onclick={() => fillWeek(p.id)}>Weekdays…</button>{/if}</td>
					</tr>
				{:else}<tr><td class="px-3 py-6 text-center text-slate-400" colspan="9">Nobody here.</td></tr>{/each}
			</tbody>
		</table>
	</div>
	<section class="card space-y-3 p-4" aria-label="Shifts">
		<h2 class="font-medium text-slate-900">Shifts</h2>
		<ul class="flex flex-wrap gap-2 text-sm">
			{#each shifts as s (s.id)}<li class="flex items-center gap-1 rounded-full px-3 py-1 text-white" style="background:{s.color}">{s.name} · {hm(s.start_min)}–{hm(s.end_min)}{#if hr}<button class="ml-1 opacity-70 hover:opacity-100" aria-label="Delete {s.name}" onclick={() => dropShift(s)}>×</button>{/if}</li>{:else}<li class="text-slate-400">No shifts yet.</li>{/each}
		</ul>
		{#if hr}
			<form class="flex flex-wrap items-end gap-2 text-sm" onsubmit={addShift}>
				<input class="input w-40" aria-label="Shift name" placeholder="Name" required maxlength="100" bind:value={form.name} />
				<label>From <input class="input w-28" type="time" bind:value={form.start} /></label>
				<label>To <input class="input w-28" type="time" bind:value={form.end} /></label>
				<label>Break (min) <input class="input w-20" type="number" min="0" max="480" bind:value={form.break_minutes} /></label>
				<input class="input h-10 w-14 p-1" type="color" aria-label="Colour" bind:value={form.color} />
				<button class="btn btn-secondary">Add shift</button>
			</form>
		{/if}
	</section>
</div>
