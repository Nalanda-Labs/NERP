<script lang="ts">
	// A dashboard's charts on a 12-column grid (one column on phones). While editing, each
	// chart can be changed, removed, moved and resized.
	import type { Figures, Widget } from '#lib/bi.ts';
	import BiChart from './BiChart.svelte';

	let {
		widgets,
		figures,
		editing = false,
		onedit,
		onremove,
		onmove,
		onresize
	}: {
		widgets: Widget[];
		figures: Record<string, Figures> | null;
		editing?: boolean;
		onedit?: (w: Widget) => void;
		onremove?: (w: Widget) => void;
		onmove?: (w: Widget, by: number) => void;
		onresize?: (w: Widget, dw: number, dh: number) => void;
	} = $props();
</script>

<div class="grid grid-cols-12 gap-3">
	{#each widgets as w, i (w.id)}
		{@const f = figures?.[w.id]}
		<section
			class="bi-cell card flex flex-col p-3"
			style="--w: {w.w}; height: {w.h * 130 + 44}px"
			aria-label={w.title}
			data-widget={w.title}
		>
			<div class="mb-1 flex items-center gap-1">
				<h2 class="flex-1 truncate text-sm font-medium text-slate-700">{w.title}</h2>
				{#if editing}
					<button
						class="rounded px-1 text-slate-400 hover:text-slate-700 disabled:opacity-30"
						aria-label="Move {w.title} earlier"
						disabled={i === 0}
						onclick={() => onmove?.(w, -1)}>←</button
					>
					<button
						class="rounded px-1 text-slate-400 hover:text-slate-700 disabled:opacity-30"
						aria-label="Move {w.title} later"
						disabled={i === widgets.length - 1}
						onclick={() => onmove?.(w, 1)}>→</button
					>
					<button
						class="rounded px-1 text-slate-400 hover:text-slate-700"
						aria-label="Narrower"
						title="Narrower"
						onclick={() => onresize?.(w, -1, 0)}>⇤</button
					>
					<button
						class="rounded px-1 text-slate-400 hover:text-slate-700"
						aria-label="Wider"
						title="Wider"
						onclick={() => onresize?.(w, 1, 0)}>⇥</button
					>
					<button
						class="rounded px-1 text-slate-400 hover:text-slate-700"
						aria-label="Shorter"
						title="Shorter"
						onclick={() => onresize?.(w, 0, -1)}>⤒</button
					>
					<button
						class="rounded px-1 text-slate-400 hover:text-slate-700"
						aria-label="Taller"
						title="Taller"
						onclick={() => onresize?.(w, 0, 1)}>⤓</button
					>
					<button
						class="rounded px-1 text-slate-500 hover:text-brand-700"
						aria-label="Change {w.title}"
						onclick={() => onedit?.(w)}>✎</button
					>
					<button
						class="rounded px-1 text-slate-400 hover:text-red-600"
						aria-label="Remove {w.title}"
						onclick={() => onremove?.(w)}>×</button
					>
				{/if}
			</div>
			<div class="min-h-0 flex-1">
				{#if !f}
					<p class="text-sm text-slate-400">Loading…</p>
				{:else if f.error}
					<p class="text-sm text-red-700" data-widget-error>{f.error}</p>
				{:else if f.result}
					<BiChart chart={w.chart} result={f.result} />
				{/if}
			</div>
		</section>
	{:else}
		<p class="col-span-12 text-sm text-slate-500">No charts yet.</p>
	{/each}
</div>

<style>
	.bi-cell {
		grid-column: span 12 / span 12;
	}
	@media (min-width: 768px) {
		.bi-cell {
			grid-column: span var(--w) / span var(--w);
		}
	}
</style>
