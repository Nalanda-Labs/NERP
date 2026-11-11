<script lang="ts">
	// The guided tour on screen: a spotlight on the part being shown and a card beside it with
	// Back, Next and Skip. Steps whose part isn't on screen (hidden, or a narrow window) are
	// left out. Esc skips; the arrow keys move.
	import { tick } from 'svelte';
	import { tours, type TourStep } from '#lib/tours.svelte.ts';

	let rect = $state<DOMRect | null>(null);
	let steps = $state<TourStep[]>([]);
	let index = $state(0);

	const visible = (sel: string) => {
		const el = document.querySelector<HTMLElement>(sel);
		if (!el) return null;
		const r = el.getBoundingClientRect();
		return r.width > 0 && r.height > 0 ? el : null;
	};
	$effect(() => {
		const c = tours.current;
		if (!c) return;
		tick().then(() => {
			steps = c.tour.steps.filter((s) => visible(s.target));
			index = 0;
			if (!steps.length) tours.finish();
			else place();
		});
	});
	function place() {
		const s = steps[index];
		const el = s && visible(s.target);
		if (!el) return void (rect = null);
		el.scrollIntoView({ block: 'nearest' });
		rect = el.getBoundingClientRect();
	}
	function go(d: number) {
		const n = index + d;
		if (n < 0) return;
		if (n >= steps.length) return void tours.finish();
		index = n;
		place();
	}
	function key(e: KeyboardEvent) {
		if (!tours.current) return;
		if (e.key === 'Escape') tours.finish();
		else if (e.key === 'ArrowRight') go(1);
		else if (e.key === 'ArrowLeft') go(-1);
	}
	// The card goes to the right of the part, or below it when there is no room.
	const card = $derived.by(() => {
		if (!rect) return '';
		const w = 300;
		const right = rect.right + 16 + w < window.innerWidth;
		const left = right ? rect.right + 16 : Math.max(12, Math.min(rect.left, window.innerWidth - w - 12));
		const top = right ? Math.max(12, Math.min(rect.top - 8, window.innerHeight - 220)) : Math.min(rect.bottom + 12, window.innerHeight - 220);
		return `left:${left}px;top:${top}px;width:${w}px`;
	});
</script>

<svelte:window onkeydown={key} onresize={place} />

{#if tours.current && rect && steps[index]}
	{@const s = steps[index]}
	<div class="pointer-events-none fixed z-[70] rounded-xl ring-4 ring-brand-500 transition-all" style="left:{rect.left - 6}px;top:{rect.top - 6}px;width:{rect.width + 12}px;height:{rect.height + 12}px;box-shadow:0 0 0 9999px rgb(15 23 42 / 0.45)" aria-hidden="true"></div>
	<div class="fixed z-[71] rounded-2xl bg-white p-4 shadow-g2" style={card} role="dialog" aria-modal="false" aria-labelledby="tour-title" data-tour-card>
		<p class="text-xs text-slate-400">{index + 1} of {steps.length}</p>
		<h2 id="tour-title" class="mt-0.5 font-semibold text-slate-900">{s.title}</h2>
		<p class="mt-1 text-sm text-slate-600">{s.text}</p>
		<div class="mt-3 flex items-center gap-2">
			<button class="text-sm text-slate-500 hover:text-slate-800" onclick={() => tours.finish()}>Skip the tour</button>
			<span class="flex-1"></span>
			{#if index > 0}<button class="btn btn-secondary px-3 py-1 text-sm" onclick={() => go(-1)}>Back</button>{/if}
			<button class="btn btn-primary px-3 py-1 text-sm" onclick={() => go(1)}>{index + 1 === steps.length ? 'Done' : 'Next'}</button>
		</div>
	</div>
{/if}
