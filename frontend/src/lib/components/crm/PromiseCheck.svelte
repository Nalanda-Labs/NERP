<script lang="ts">
	// Promise checker: can the deal be delivered by a date — stock (with receipts and other
	// deals' needs) and the free working time of the people doing the work.
	import { api } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';

	let {
		itemId,
		closeDate,
		assigned,
		onDate
	}: { itemId: string; closeDate: string | null; assigned: PickedUser | null; /** After "Promise this date" saved a new close date. */ onDate?: () => void } = $props();

	type Verdict = 'ok' | 'risky' | 'no';
	type Good = { product_id: string; name: string; needed: string; on_hand?: string; committed?: string; free?: string; status: Verdict | 'unknown'; text: string };
	type Person = { id: string; name: string; working_days: number; hours: number; meeting_hours: number; task_hours: number; tasks_without_estimate: number; free_hours: number; away_on_the_day: boolean };
	type Result = { date: string; verdict: Verdict; earliest: string | null; reasons: string[]; goods: Good[]; work: { needed_hours: number; free_hours: number; status: Verdict }; people: Person[] };

	let open = $state(false);
	let date = $state('');
	let people = $state<PickedUser[]>([]);
	let r = $state<Result | null>(null);
	let busy = $state(false);

	function start() {
		open = true;
		date = closeDate ?? '';
		people = assigned ? [assigned] : [];
		if (date) run();
	}
	async function run() {
		if (!date) return;
		busy = true;
		try {
			const q = new URLSearchParams({ date });
			if (people.length) q.set('users', people.map((p) => p.id).join(','));
			r = await api.get<Result>(`/crm/items/${itemId}/promise?${q}`);
		} catch (e) {
			r = null;
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	async function promise(d: string) {
		try {
			await api.patch(`/crm/deal/items/${itemId}`, { close_date: d });
			toasts.success(`Close date set to ${formatDate(d)}`);
			date = d;
			await run();
			onDate?.();
		} catch (e) {
			toasts.error(e);
		}
	}
	const WORD: Record<Verdict, string> = { ok: 'Can be done', risky: 'Risky', no: 'Not by then' };
	const TONE: Record<string, string> = { ok: 'bg-emerald-50 text-emerald-800', risky: 'bg-amber-50 text-amber-800', no: 'bg-red-50 text-red-800', unknown: 'bg-slate-100 text-slate-600' };
</script>

<section class="card p-4 text-sm" aria-label="Promise checker">
	<div class="flex items-center gap-2">
		<h2 class="font-semibold text-slate-900">Can we deliver by…?</h2>
		{#if !open}<button class="ml-auto text-brand-700 hover:underline" onclick={start} data-promise-open>Check a date</button>{/if}
	</div>
	{#if open}
		<div class="mt-2 space-y-2">
			<div class="flex flex-wrap items-center gap-2">
				<input type="date" class="input w-44" bind:value={date} aria-label="Delivery date" onchange={run} />
				<button class="btn btn-secondary" disabled={!date || busy} onclick={run}>{busy ? 'Checking…' : 'Check'}</button>
			</div>
			<div>
				<p class="mb-1 text-xs text-slate-500">Who does the work</p>
				<MultiUserPicker bind:value={people} placeholder="Add people…" onchange={run} />
			</div>
		</div>
		{#if r}
			<div class="mt-3 space-y-2" data-promise={r.verdict}>
				<p class="rounded-lg px-3 py-2 font-medium {TONE[r.verdict]}">
					{WORD[r.verdict]} on {formatDate(r.date)}{#if r.verdict !== 'ok' && r.earliest && r.earliest !== r.date}; the earliest that works is <b data-earliest={r.earliest}>{formatDate(r.earliest)}</b>{/if}
				</p>
				{#if r.verdict !== 'ok' && r.earliest && r.earliest !== r.date}<button class="text-brand-700 hover:underline" onclick={() => promise(r!.earliest!)} data-promise-earliest>Make {formatDate(r.earliest)} the close date</button>
				{:else if r.verdict === 'ok' && r.date !== closeDate}<button class="text-brand-700 hover:underline" onclick={() => promise(r!.date)} data-promise-this>Make it the close date</button>{/if}
				{#if r.reasons.length}<ul class="list-disc space-y-0.5 pl-5 text-slate-700" data-reasons>{#each r.reasons as x, i (i)}<li>{x}</li>{/each}</ul>{/if}
				{#if r.goods.length}
					<h3 class="pt-1 text-xs font-semibold tracking-wide text-slate-500 uppercase">Goods</h3>
					<ul class="space-y-1">
						{#each r.goods as g (g.product_id)}
							<li class="flex gap-2" data-good={g.name}><span class="h-fit rounded px-1.5 text-xs {TONE[g.status]}">{g.status === 'unknown' ? '?' : g.status === 'ok' ? '✓' : '!'}</span><span><b class="font-medium">{Number(g.needed)} × {g.name}</b> <span class="text-slate-500">— {g.text}</span></span></li>
						{/each}
					</ul>
				{/if}
				{#if r.work.needed_hours > 0 || r.people.length}
					<h3 class="pt-1 text-xs font-semibold tracking-wide text-slate-500 uppercase">Work{r.work.needed_hours > 0 ? `: ${r.work.needed_hours} h needed, ${r.work.free_hours} h free` : ''}</h3>
					<ul class="space-y-1 text-slate-600">
						{#each r.people as p (p.id)}
							<li data-person={p.name}><b class="font-medium text-slate-800">{p.name}</b>: {p.working_days} working days, {p.hours} h, less {p.meeting_hours} h meetings and {p.task_hours} h tasks due → <b>{p.free_hours} h free</b>{p.tasks_without_estimate ? ` (${p.tasks_without_estimate} tasks have no estimate)` : ''}{p.away_on_the_day ? ' · away on the day' : ''}</li>
						{/each}
					</ul>
				{/if}
			</div>
		{/if}
	{/if}
</section>
