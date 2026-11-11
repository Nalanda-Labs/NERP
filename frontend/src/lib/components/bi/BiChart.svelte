<script lang="ts">
	// One chart's figures: a single figure (with the period before), a table, or an ECharts
	// chart. Columns g0/g1 are the groupings and m0… the figures: a second grouping becomes
	// the series, otherwise each figure is one.
	import { onDestroy } from 'svelte';
	import * as echarts from 'echarts/core';
	import { BarChart, FunnelChart, LineChart, PieChart } from 'echarts/charts';
	import { GridComponent, LegendComponent, TooltipComponent } from 'echarts/components';
	import { CanvasRenderer } from 'echarts/renderers';
	import { show, type Result } from '#lib/bi.ts';

	echarts.use([
		BarChart,
		LineChart,
		PieChart,
		FunnelChart,
		GridComponent,
		LegendComponent,
		TooltipComponent,
		CanvasRenderer
	]);

	let { chart, result }: { chart: string; result: Result } = $props();
	let el: HTMLDivElement | undefined = $state();
	let inst: echarts.ECharts | null = null;
	let ro: ResizeObserver | null = null;

	const groups = $derived(result.columns.filter((c) => c.key.startsWith('g')));
	const measures = $derived(result.columns.filter((c) => c.key.startsWith('m')));
	const idx = (key: string) => result.columns.findIndex((c) => c.key === key);
	const label = (v: unknown, i: number) => show(v, result.columns[i].type);

	const single = $derived.by(() => {
		if (chart !== 'number' || !measures.length) return null;
		const i = idx(measures[0].key);
		const now = Number(result.rows[0]?.[i] ?? 0);
		const before = result.previous ? Number(result.previous.rows[0]?.[i] ?? 0) : null;
		const change =
			before === null
				? null
				: before === 0
					? now === 0
						? 0
						: null
					: ((now - before) / Math.abs(before)) * 100;
		return {
			text: label(result.rows[0]?.[i], i),
			label: measures[0].label,
			before: before === null ? null : label(before, i),
			change,
			more: measures
				.slice(1)
				.map((m) => ({ label: m.label, text: label(result.rows[0]?.[idx(m.key)], idx(m.key)) }))
		};
	});

	function option(): echarts.EChartsCoreOption {
		const rows = result.rows;
		const tooltip = {
			trigger: chart === 'pie' || chart === 'funnel' ? 'item' : 'axis',
			confine: true
		};
		if (chart === 'pie' || chart === 'funnel') {
			const m = idx(measures[0].key);
			const data = rows.map((r) => ({
				name: groups.length
					? groups.map((g) => label(r[idx(g.key)], idx(g.key))).join(' · ')
					: measures[0].label,
				value: Number(r[m] ?? 0)
			}));
			return {
				tooltip,
				legend: { type: 'scroll', bottom: 0 },
				series: [
					chart === 'pie'
						? {
								type: 'pie',
								radius: ['40%', '70%'],
								center: ['50%', '45%'],
								data,
								label: { show: data.length <= 8 }
							}
						: {
								type: 'funnel',
								left: '10%',
								width: '80%',
								top: 10,
								bottom: 40,
								sort: 'descending',
								data,
								label: { position: 'inside' }
							}
				]
			};
		}
		// Categories and series.
		let cats: string[];
		let series: { name: string; data: (number | null)[] }[];
		if (!groups.length) {
			cats = measures.map((m) => m.label);
			series = [{ name: '', data: measures.map((m) => Number(rows[0]?.[idx(m.key)] ?? 0)) }];
		} else if (groups.length === 2) {
			const [a, b] = [idx('g0'), idx('g1')];
			const m = idx(measures[0].key);
			cats = [...new Set(rows.map((r) => label(r[a], a)))];
			const names = [...new Set(rows.map((r) => label(r[b], b)))];
			series = names.map((n) => ({
				name: n,
				data: cats.map((c) => {
					const r = rows.find((r) => label(r[a], a) === c && label(r[b], b) === n);
					return r ? Number(r[m] ?? 0) : null;
				})
			}));
		} else {
			const a = idx('g0');
			cats = rows.map((r) => label(r[a], a));
			series = measures.map((m) => ({
				name: m.label,
				data: rows.map((r) => (r[idx(m.key)] === null ? null : Number(r[idx(m.key)])))
			}));
		}
		const horizontal = chart === 'hbar';
		const type = chart === 'line' || chart === 'area' ? 'line' : 'bar';
		const cat = {
			type: 'category',
			data: horizontal ? [...cats].reverse() : cats,
			axisLabel: { hideOverlap: true }
		};
		const val = { type: 'value' };
		return {
			tooltip,
			legend: series.length > 1 ? { type: 'scroll', bottom: 0 } : undefined,
			grid: { left: 8, right: 16, top: 16, bottom: series.length > 1 ? 36 : 8, containLabel: true },
			xAxis: horizontal ? val : cat,
			yAxis: horizontal ? cat : val,
			series: series.map((s) => ({
				name: s.name,
				type,
				data: horizontal ? [...s.data].reverse() : s.data,
				smooth: type === 'line',
				areaStyle: chart === 'area' ? {} : undefined,
				stack: chart === 'area' && series.length > 1 ? 'all' : undefined,
				barMaxWidth: 48
			}))
		};
	}

	$effect(() => {
		if (!el || chart === 'number' || chart === 'table') return;
		if (inst && inst.getDom() !== el) {
			ro?.disconnect();
			inst.dispose();
			inst = null;
		}
		if (!inst) {
			inst = echarts.init(el);
			ro = new ResizeObserver(() => inst?.resize());
			ro.observe(el);
		}
		inst.setOption(option(), true);
	});
	onDestroy(() => {
		ro?.disconnect();
		inst?.dispose();
	});
</script>

{#if chart === 'number'}
	{#if single}
		<div class="flex h-full flex-col justify-center gap-1 px-1" data-figure>
			<p class="text-3xl font-semibold text-slate-900 tabular-nums">{single.text}</p>
			<p class="text-xs text-slate-500">{single.label}</p>
			{#if single.before !== null}
				<p
					class="text-xs {single.change === null || single.change === 0
						? 'text-slate-500'
						: single.change > 0
							? 'text-emerald-700'
							: 'text-red-700'}"
					data-change
				>
					{single.change === null
						? ''
						: `${single.change > 0 ? '▲' : single.change < 0 ? '▼' : ''} ${Math.abs(single.change).toFixed(1)}% · `}before:
					{single.before}
				</p>
			{/if}
			{#each single.more as m (m.label)}<p class="text-sm text-slate-600">
					<span class="font-medium tabular-nums">{m.text}</span>
					{m.label}
				</p>{/each}
		</div>
	{/if}
{:else if chart === 'table'}
	<div class="h-full overflow-auto">
		<table class="w-full text-sm">
			<thead class="sticky top-0 bg-white"
				><tr
					>{#each result.columns as c (c.key)}<th
							class="border-b border-slate-200 px-2 py-1 text-left font-medium text-slate-600 {c.type ===
							'number'
								? 'text-right'
								: ''}">{c.label}</th
						>{/each}</tr
				></thead
			>
			<tbody>
				{#each result.rows as r, i (i)}
					<tr class="border-b border-slate-50"
						>{#each result.columns as c, j (c.key)}<td
								class="px-2 py-1 {c.type === 'number' ? 'text-right tabular-nums' : ''}"
								>{show(r[j], c.type)}</td
							>{/each}</tr
					>
				{:else}<tr
						><td class="px-2 py-3 text-slate-400" colspan={result.columns.length}>No rows.</td></tr
					>{/each}
			</tbody>
		</table>
	</div>
{:else if !result.rows.length}
	<p class="flex h-full items-center justify-center text-sm text-slate-400">
		Nothing for this period.
	</p>
{:else}
	<div bind:this={el} class="h-full w-full" data-echart></div>
{/if}
