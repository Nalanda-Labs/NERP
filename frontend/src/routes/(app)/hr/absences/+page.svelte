<script lang="ts">
	// HR → Time off: who is away (a month chart), my requests and balance, requests to approve.
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Absence = { id: string; user_id: string; user_name: string; kind: string; starts_on: string; ends_on: string; note: string | null; status: string; decided_by_name: string | null; decision_note: string | null; days?: number; balance?: Balance };
	type Balance = { year: number; allowance: string; used: number; requested: number; left: string };
	const KINDS: Record<string, [string, string]> = {
		vacation: ['Vacation', '#1a73e8'],
		sick: ['Sick leave', '#d93025'],
		personal: ['Personal day', '#9334e6'],
		business_trip: ['Business trip', '#188038'],
		other: ['Time off', '#5f6368']
	};
	const STATUS: Record<string, [string, string]> = { pending: ['Waiting', 'bg-amber-50 text-amber-800'], approved: ['Approved', 'bg-emerald-50 text-emerald-800'], rejected: ['Rejected', 'bg-red-50 text-red-700'], cancelled: ['Cancelled', 'bg-slate-100 text-slate-500'] };

	let tab = $state<'chart' | 'mine' | 'approve'>((page.url.searchParams.get('tab') as 'approve') ?? 'chart');
	let month = $state(new Date().toISOString().slice(0, 7));
	let chart = $state<{ people: { id: string; name: string }[]; items: Absence[]; holidays: string[]; work_weekdays: number[] } | null>(null);
	let mine = $state<{ items: Absence[]; balance: Balance } | null>(null);
	let waiting = $state<Absence[]>([]);

	const days = $derived.by(() => {
		const [y, m] = month.split('-').map(Number);
		const n = new Date(y, m, 0).getDate();
		return Array.from({ length: n }, (_, i) => `${month}-${String(i + 1).padStart(2, '0')}`);
	});
	async function loadChart() {
		chart = await api.get(`/hr/absences?from=${days[0]}&to=${days[days.length - 1]}`);
	}
	const loadMine = async () => (mine = await api.get('/hr/absences/mine'));
	const loadWaiting = async () => (waiting = (await api.get<{ items: Absence[] }>('/hr/absences/pending')).items);
	$effect(() => {
		days;
		loadChart();
	});
	onMount(() => {
		loadMine();
		loadWaiting();
	});
	const off = (d: string) => {
		const wd = ((new Date(d + 'T00:00').getDay() + 6) % 7) + 1;
		return !chart?.work_weekdays.includes(wd) || chart.holidays.includes(d);
	};
	const on = (person: string, d: string) => chart?.items.find((a) => a.user_id === person && a.starts_on <= d && a.ends_on >= d);
	function shift(n: number) {
		const [y, m] = month.split('-').map(Number);
		const d = new Date(y, m - 1 + n, 1);
		month = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
	}

	let reqOpen = $state(false);
	let form = $state({ kind: 'vacation', starts_on: '', ends_on: '', note: '' });
	async function request(e: SubmitEvent) {
		e.preventDefault();
		try {
			const a = await api.post<Absence>('/hr/absences', form);
			toasts.success(`Requested: ${a.days} working day${a.days === 1 ? '' : 's'}`);
			reqOpen = false;
			form = { kind: 'vacation', starts_on: '', ends_on: '', note: '' };
			loadMine();
			loadChart();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function cancel(a: Absence) {
		if (!confirm('Cancel this time off?')) return;
		try {
			mine = await api.post(`/hr/absences/${a.id}/cancel`, {});
			loadChart();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function decide(a: Absence, approve: boolean) {
		const note = approve ? '' : (prompt('Why? (shared with them)') ?? null);
		if (note === null) return;
		try {
			waiting = (await api.post<{ items: Absence[] }>(`/hr/absences/${a.id}/decide`, { approve, note: note || null })).items;
			loadChart();
		} catch (err) {
			toasts.error(err);
		}
	}
	const range = (a: Absence) => (a.starts_on === a.ends_on ? formatDate(a.starts_on) : `${formatDate(a.starts_on)} – ${formatDate(a.ends_on)}`);
</script>

<svelte:head><title>Time off · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Time off</h1>
		<button class="btn btn-primary" onclick={() => (reqOpen = true)}>Request time off</button>
	</div>
	<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" style="width:fit-content">
		{#each [['chart', 'Who is away'], ['mine', 'Mine'], ['approve', `To approve${waiting.length ? ` (${waiting.length})` : ''}`]] as [k, l] (k)}
			<button role="tab" aria-selected={tab === k} class="rounded-full px-4 py-1.5 font-medium {tab === k ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = k as typeof tab)}>{l}</button>
		{/each}
	</div>

	{#if tab === 'chart' && chart}
		<div class="flex items-center gap-2">
			<button class="btn btn-ghost p-2" aria-label="Previous month" onclick={() => shift(-1)}>‹</button>
			<span class="font-medium">{new Date(days[0] + 'T00:00').toLocaleDateString(undefined, { month: 'long', year: 'numeric' })}</span>
			<button class="btn btn-ghost p-2" aria-label="Next month" onclick={() => shift(1)}>›</button>
			<span class="ml-auto flex flex-wrap gap-3 text-xs text-slate-600">{#each Object.entries(KINDS) as [k, [l, c]] (k)}<span class="flex items-center gap-1"><span class="size-2.5 rounded-sm" style="background:{c}"></span>{l}</span>{/each}<span class="flex items-center gap-1"><span class="size-2.5 rounded-sm border border-dashed border-slate-400"></span>Waiting</span></span>
		</div>
		<div class="card overflow-x-auto">
			<table class="text-xs">
				<thead><tr><th class="sticky left-0 bg-white px-3 py-2 text-left font-medium text-slate-500">Person</th>{#each days as d (d)}<th class="w-7 py-2 text-center font-normal {off(d) ? 'text-slate-300' : 'text-slate-500'}">{Number(d.slice(8))}</th>{/each}</tr></thead>
				<tbody>
					{#each chart.people as p (p.id)}
						<tr class="border-t border-slate-50" data-chart-person={p.name}>
							<td class="sticky left-0 bg-white px-3 py-1.5 whitespace-nowrap text-slate-800">{p.name}</td>
							{#each days as d (d)}
								{@const a = on(p.id, d)}
								<td class="h-7 p-0.5 {off(d) ? 'bg-slate-50' : ''}">
									{#if a}<div class="h-full rounded-sm {a.status === 'pending' ? 'border border-dashed opacity-70' : ''}" style="background:{a.status === 'pending' ? 'transparent' : KINDS[a.kind][1]};border-color:{KINDS[a.kind][1]}" title="{KINDS[a.kind][0]}{a.status === 'pending' ? ' (waiting)' : ''}{a.note ? `: ${a.note}` : ''}" data-away={a.kind}></div>{/if}
								</td>
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{:else if tab === 'mine' && mine}
		<div class="card flex flex-wrap gap-6 p-5 text-sm" data-balance>
			<div><p class="text-xs text-slate-500">Vacation {mine.balance.year}</p><p class="text-2xl font-semibold">{Number(mine.balance.left)} <span class="text-sm font-normal text-slate-500">days left</span></p></div>
			<div><p class="text-xs text-slate-500">Allowance</p><p class="text-lg">{Number(mine.balance.allowance)}</p></div>
			<div><p class="text-xs text-slate-500">Taken</p><p class="text-lg">{mine.balance.used}</p></div>
			<div><p class="text-xs text-slate-500">Waiting</p><p class="text-lg">{mine.balance.requested}</p></div>
		</div>
		<div class="card divide-y divide-slate-50">
			{#each mine.items as a (a.id)}
				<div class="flex flex-wrap items-center gap-3 px-5 py-3 text-sm" data-request={a.kind}>
					<span class="size-2.5 rounded-sm" style="background:{KINDS[a.kind][1]}"></span>
					<span class="font-medium text-slate-900">{KINDS[a.kind][0]}</span>
					<span class="text-slate-600">{range(a)}</span>
					<span class="badge {STATUS[a.status][1]}">{STATUS[a.status][0]}</span>
					{#if a.decided_by_name}<span class="text-xs text-slate-500">by {a.decided_by_name}{a.decision_note ? `: “${a.decision_note}”` : ''}</span>{/if}
					{#if a.status === 'pending' || (a.status === 'approved' && a.starts_on > new Date().toISOString().slice(0, 10))}<button class="ml-auto text-xs text-slate-500 hover:text-red-600" onclick={() => cancel(a)}>Cancel</button>{/if}
				</div>
			{:else}
				<p class="px-5 py-8 text-center text-sm text-slate-500">No time off yet.</p>
			{/each}
		</div>
	{:else if tab === 'approve'}
		<div class="card divide-y divide-slate-50">
			{#each waiting as a (a.id)}
				<div class="flex flex-wrap items-center gap-3 px-5 py-3 text-sm" data-pending={a.user_name}>
					<span class="font-medium text-slate-900">{a.user_name}</span>
					<span>{KINDS[a.kind][0]}, {range(a)} · {a.days} working day{a.days === 1 ? '' : 's'}</span>
					{#if a.balance}<span class="text-xs text-slate-500">{Number(a.balance.left)} vacation days left</span>{/if}
					{#if a.note}<span class="w-full text-slate-600">“{a.note}”</span>{/if}
					<span class="ml-auto flex gap-2"><button class="btn btn-secondary px-3 py-1" onclick={() => decide(a, false)}>Reject</button><button class="btn btn-primary px-3 py-1" onclick={() => decide(a, true)}>Approve</button></span>
				</div>
			{:else}
				<p class="px-5 py-8 text-center text-sm text-slate-500">Nothing waiting for you.</p>
			{/each}
		</div>
	{/if}
</div>

<Modal bind:open={reqOpen} title="Request time off">
	<form id="req" class="grid gap-3 sm:grid-cols-2" onsubmit={request}>
		<div class="sm:col-span-2"><label class="label" for="rq-k">What</label><select id="rq-k" class="input" bind:value={form.kind}>{#each Object.entries(KINDS) as [k, [l]] (k)}<option value={k}>{l}</option>{/each}</select></div>
		<div><label class="label" for="rq-s">From</label><input id="rq-s" class="input" type="date" required bind:value={form.starts_on} onchange={() => (!form.ends_on || form.ends_on < form.starts_on) && (form.ends_on = form.starts_on)} /></div>
		<div><label class="label" for="rq-e">Until (including)</label><input id="rq-e" class="input" type="date" required bind:value={form.ends_on} /></div>
		<div class="sm:col-span-2"><label class="label" for="rq-n">Note <span class="font-normal text-slate-400">(optional)</span></label><textarea id="rq-n" class="input" rows="2" maxlength="1000" bind:value={form.note}></textarea></div>
		{#if mine && form.kind === 'vacation'}<p class="text-xs text-slate-500 sm:col-span-2">{Number(mine.balance.left) - mine.balance.requested} vacation days available. Weekends and holidays don't count.</p>{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (reqOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="req">Request</button>
	{/snippet}
</Modal>
