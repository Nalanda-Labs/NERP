<script lang="ts">
	// An open deal's health: its score, grade and the reasons, compared with the pipeline's
	// won deals.
	import { api } from '#lib/api.ts';

	let { itemId }: { itemId: string } = $props();
	type Health = { score: number | null; grade: 'healthy' | 'at_risk' | 'poor' | null; reasons: { kind: string; penalty: number; text: string }[] };
	type Step = { step: string; text: string; done: boolean; won_with: number; won_without: number; deals_with: number; usual_day: number };
	let h = $state<Health | null>(null);
	let steps = $state<Step[]>([]);
	$effect(() => {
		itemId;
		api.get<Health>(`/crm/items/${itemId}/health`).then((x) => (h = x)).catch(() => (h = null));
		api.get<{ items: Step[] }>(`/crm/items/${itemId}/next-steps`).then((x) => (steps = x.items)).catch(() => (steps = []));
	});
	const LABEL = { healthy: 'Healthy', at_risk: 'At risk', poor: 'Poor' };
	const COLOR = { healthy: 'bg-emerald-50 text-emerald-800', at_risk: 'bg-amber-50 text-amber-800', poor: 'bg-red-50 text-red-800' };
</script>

{#if h && h.score !== null && h.grade}
	<section class="card p-4 text-sm" aria-label="Deal health" data-health={h.grade}>
		<div class="flex items-center gap-2">
			<h2 class="font-semibold text-slate-900">Health</h2>
			<span class="rounded-full px-2 py-0.5 text-xs font-medium {COLOR[h.grade]}">{LABEL[h.grade]}</span>
			<span class="ml-auto text-lg font-semibold tabular-nums" data-score>{h.score}</span>
		</div>
		{#if h.reasons.length}
			<ul class="mt-2 space-y-1 text-slate-600">
				{#each h.reasons as r (r.kind)}<li class="flex gap-2"><span class="w-8 shrink-0 text-right text-xs text-red-600 tabular-nums">−{r.penalty}</span><span>{r.text}</span></li>{/each}
			</ul>
		{:else}<p class="mt-1 text-slate-500">On track: recent contact, the usual pace, a next step planned.</p>{/if}
		{#if steps.length}
			<h3 class="mt-3 text-xs font-semibold tracking-wide text-slate-500 uppercase">What worked at this stage</h3>
			<ul class="mt-1 space-y-1" data-next-steps>
				{#each steps as st (st.step)}
					<li class="flex gap-2" data-step={st.step}>
						<span class="w-4 shrink-0 {st.done ? 'text-emerald-600' : 'text-slate-300'}">{st.done ? '✓' : '○'}</span>
						<span><span class="font-medium {st.done ? 'text-slate-500' : 'text-slate-800'}">{st.text}</span>
							<span class="block text-xs text-slate-500">{st.won_with}% won with it, {st.won_without}% without ({st.deals_with} deals){st.usual_day > 0 ? `; usually by day ${st.usual_day}` : ''}</span></span>
					</li>
				{/each}
			</ul>
		{/if}
	</section>
{/if}
