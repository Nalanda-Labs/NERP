<script lang="ts">
	// Making or changing a chart: its dataset, what it groups by and counts (or which columns a
	// table lists), its own conditions, the date column the dashboard's period applies to, and
	// sorting — with a live preview.
	import { api } from '#lib/api.ts';
	import {
		AGGS,
		complete,
		BUCKETS,
		CHARTS,
		type Dataset,
		type Filter,
		type Period,
		type Result,
		type Spec,
		type Widget
	} from '#lib/bi.ts';
	import Modal from '#lib/components/Modal.svelte';
	import BiChart from './BiChart.svelte';
	import FilterList from './FilterList.svelte';

	let {
		open = $bindable(false),
		datasets,
		widget,
		period,
		onsave
	}: {
		open: boolean;
		datasets: Dataset[];
		widget: Widget | null;
		period?: Period;
		onsave: (w: {
			title: string;
			chart: string;
			w: number;
			h: number;
			spec: Spec;
		}) => Promise<void>;
	} = $props();

	let title = $state('');
	let chart = $state('bar');
	let dataset = $state('');
	let groups = $state<{ field: string; bucket?: string }[]>([]);
	let measures = $state<{ agg: string; field?: string }[]>([]);
	let rowsMode = $state(false);
	let columns = $state<string[]>([]);
	let filters = $state<Filter[]>([]);
	let dateField = $state('');
	let sort = $state('');
	let limit = $state<number | null>(null);
	let compare = $state(false);
	let size = $state({ w: 6, h: 2 });
	let preview = $state<Result | null>(null);
	let previewError = $state('');
	let busy = $state(false);

	const ds = $derived(datasets.find((d) => d.key === dataset));
	const cols = $derived(ds?.columns ?? []);
	const dated = $derived(cols.filter((c) => c.type === 'date' || c.type === 'time'));
	const isDate = (k: string) => dated.some((c) => c.key === k);
	const groupsets = $derived([...new Set(datasets.map((d) => d.group))]);

	// Fill the form when it opens.
	let filledFor = '';
	$effect(() => {
		if (!open) {
			filledFor = '';
			return;
		}
		const key = widget?.id ?? 'new';
		if (filledFor === key) return;
		filledFor = key;
		const s: Spec = widget?.spec ?? { dataset: datasets[0]?.key ?? '' };
		title = widget?.title ?? '';
		chart = widget?.chart ?? 'bar';
		dataset = s.dataset;
		groups = (s.groups ?? []).map((g) => ({ ...g }));
		measures = (s.measures ?? []).map((m) => ({ ...m }));
		columns = [...(s.columns ?? [])];
		rowsMode = !!s.columns?.length;
		filters = (s.filters ?? []).map((f) => ({ ...f }));
		dateField = s.date_field ?? '';
		sort = s.sort ? `${s.sort.by}:${s.sort.desc ? 'desc' : 'asc'}` : '';
		limit = s.limit ?? null;
		compare = !!s.compare;
		size = { w: widget?.w ?? 6, h: widget?.h ?? 2 };
		if (!widget) pickDataset(dataset);
	});

	function pickDataset(key: string) {
		dataset = key;
		const d = datasets.find((x) => x.key === key);
		const firstText = d?.columns.find((c) => c.type === 'text');
		groups = chart === 'number' ? [] : firstText ? [{ field: firstText.key }] : [];
		measures = [{ agg: 'count' }];
		columns = (d?.columns ?? []).slice(0, 5).map((c) => c.key);
		filters = [];
		dateField = d?.columns.find((c) => c.type === 'date' || c.type === 'time')?.key ?? '';
		sort = '';
	}
	function pickChart(c: string) {
		chart = c;
		if (c === 'number') groups = [];
		if (c !== 'table') rowsMode = false;
		if (c === 'number') size = { w: 3, h: 1 };
	}

	const spec = $derived<Spec>({
		dataset,
		groups:
			rowsMode || chart === 'number'
				? []
				: groups
						.filter((g) => g.field)
						.map((g) =>
							isDate(g.field) ? { field: g.field, bucket: g.bucket ?? 'month' } : { field: g.field }
						),
		measures: rowsMode
			? []
			: measures.map((m) =>
					m.agg === 'count' ? { agg: 'count' } : { agg: m.agg, field: m.field ?? '' }
				),
		columns: rowsMode ? columns : [],
		filters: complete($state.snapshot(filters)),
		date_field: dateField || null,
		sort: sort ? { by: sort.split(':')[0], desc: sort.endsWith(':desc') } : null,
		limit: limit || null,
		compare: chart === 'number' && compare
	});

	let timer: ReturnType<typeof setTimeout>;
	$effect(() => {
		if (!open || !dataset) return;
		const body = { spec: $state.snapshot(spec), chart, period };
		clearTimeout(timer);
		timer = setTimeout(async () => {
			try {
				preview = await api.post<Result>('/bi/query', body);
				previewError = '';
			} catch (e) {
				preview = null;
				previewError = (e as Error).message;
			}
		}, 400);
	});

	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			await onsave({ title, chart, w: size.w, h: size.h, spec: $state.snapshot(spec) });
			open = false;
		} finally {
			busy = false;
		}
	}
	const sortOptions = $derived<[string, string][]>(
		rowsMode
			? cols
					.filter((c) => columns.includes(c.key))
					.flatMap((c): [string, string][] => [
						[`${c.key}:asc`, `${c.label} ↑`],
						[`${c.key}:desc`, `${c.label} ↓`]
					])
			: [
					...(groups[0]?.field
						? ([
								['g0:asc', 'First grouping ↑'],
								['g0:desc', 'First grouping ↓']
							] as [string, string][])
						: []),
					...measures.flatMap((_, i): [string, string][] => [
						[`m${i}:desc`, `Figure ${i + 1} ↓`],
						[`m${i}:asc`, `Figure ${i + 1} ↑`]
					])
				]
	);
</script>

<Modal bind:open title={widget ? 'Change the chart' : 'New chart'} size="lg">
	<form id="bi-widget" class="grid gap-4 text-sm lg:grid-cols-2" onsubmit={save}>
		<div class="space-y-3">
			<div>
				<label class="label" for="bw-title">Title</label><input
					id="bw-title"
					class="input"
					required
					maxlength="200"
					bind:value={title}
				/>
			</div>
			<div>
				<span class="label">Kind</span>
				<div class="flex flex-wrap gap-1" role="group" aria-label="Kind of chart">
					{#each CHARTS as [k, l] (k)}<button
							type="button"
							class="rounded-full border px-2.5 py-1 text-xs {chart === k
								? 'text-brand-800 border-brand-500 bg-brand-50'
								: 'border-slate-200 text-slate-600'}"
							aria-pressed={chart === k}
							onclick={() => pickChart(k)}>{l}</button
						>{/each}
				</div>
			</div>
			<div>
				<label class="label" for="bw-ds">Data</label>
				<select
					id="bw-ds"
					class="input"
					value={dataset}
					onchange={(e) => pickDataset(e.currentTarget.value)}
				>
					{#each groupsets as g (g)}<optgroup label={g}
							>{#each datasets.filter((d) => d.group === g) as d (d.key)}<option value={d.key}
									>{d.name}</option
								>{/each}</optgroup
						>{/each}
				</select>
			</div>
			{#if chart === 'table'}
				<label class="flex items-center gap-2"
					><input type="checkbox" class="rounded border-slate-300" bind:checked={rowsMode} /> List rows
					(instead of grouped figures)</label
				>
			{/if}
			{#if rowsMode}
				<fieldset>
					<legend class="label">Columns</legend>
					<div class="grid max-h-40 grid-cols-2 gap-1 overflow-y-auto">
						{#each cols as c (c.key)}<label class="flex items-center gap-1.5"
								><input
									type="checkbox"
									class="rounded border-slate-300"
									value={c.key}
									bind:group={columns}
								/>
								{c.label}</label
							>{/each}
					</div>
				</fieldset>
			{:else}
				{#if chart !== 'number'}
					<div class="space-y-1">
						<span class="label">Group by</span>
						{#each groups as g, i (i)}
							<div class="flex gap-1">
								<select class="input py-1" aria-label="Group by" bind:value={groups[i].field}
									>{#each cols as c (c.key)}<option value={c.key}>{c.label}</option>{/each}</select
								>
								{#if isDate(g.field)}<select
										class="input w-36 py-1"
										aria-label="Date step"
										value={g.bucket ?? 'month'}
										onchange={(e) => (groups[i].bucket = e.currentTarget.value)}
										>{#each BUCKETS as [k, l] (k)}<option value={k}>{l}</option>{/each}</select
									>{/if}
								<button
									type="button"
									class="px-1 text-slate-400 hover:text-red-600"
									aria-label="Remove grouping"
									onclick={() => (groups = groups.filter((_, j) => j !== i))}>×</button
								>
							</div>
						{/each}
						{#if groups.length < 2}<button
								type="button"
								class="text-brand-600 hover:underline"
								onclick={() => (groups = [...groups, { field: cols[0]?.key ?? '' }])}
								>+ {groups.length ? 'Split each into series by' : 'Group by a column'}</button
							>{/if}
					</div>
				{/if}
				<div class="space-y-1">
					<span class="label">Figures</span>
					{#each measures as m, i (i)}
						<div class="flex gap-1">
							<select class="input py-1" aria-label="Figure" bind:value={measures[i].agg}
								>{#each AGGS as [k, l] (k)}<option value={k}>{l}</option>{/each}</select
							>
							{#if m.agg !== 'count'}
								<select class="input py-1" aria-label="Of column" bind:value={measures[i].field}>
									<option value="">Pick a column…</option>
									{#each cols.filter( (c) => (m.agg === 'sum' || m.agg === 'avg' ? c.type === 'number' : m.agg === 'count_distinct' ? true : c.type !== 'text' && c.type !== 'bool') ) as c (c.key)}<option
											value={c.key}>{c.label}</option
										>{/each}
								</select>
							{/if}
							{#if measures.length > 1}<button
									type="button"
									class="px-1 text-slate-400 hover:text-red-600"
									aria-label="Remove figure"
									onclick={() => (measures = measures.filter((_, j) => j !== i))}>×</button
								>{/if}
						</div>
					{/each}
					{#if measures.length < 6 && groups.length < 2}<button
							type="button"
							class="text-brand-600 hover:underline"
							onclick={() => (measures = [...measures, { agg: 'count' }])}>+ Another figure</button
						>{/if}
				</div>
			{/if}
			<FilterList {cols} bind:filters />
			<div class="grid grid-cols-2 gap-2">
				<div>
					<label class="label" for="bw-date">The dashboard's period applies to</label>
					<select id="bw-date" class="input" bind:value={dateField}
						><option value="">— (all rows)</option>{#each dated as c (c.key)}<option value={c.key}
								>{c.label}</option
							>{/each}</select
					>
				</div>
				<div>
					<label class="label" for="bw-sort">Sort</label>
					<select id="bw-sort" class="input" bind:value={sort}
						><option value="">Automatic</option>{#each sortOptions as [k, l] (k)}<option value={k}
								>{l}</option
							>{/each}</select
					>
				</div>
				<div>
					<label class="label" for="bw-limit">At most rows</label><input
						id="bw-limit"
						class="input"
						type="number"
						min="1"
						max="1000"
						placeholder="200"
						bind:value={limit}
					/>
				</div>
				{#if chart === 'number'}<label class="mt-6 flex items-center gap-2"
						><input type="checkbox" class="rounded border-slate-300" bind:checked={compare} /> Compare
						with the period before</label
					>{/if}
			</div>
		</div>
		<div class="flex min-h-72 flex-col rounded-lg border border-slate-200 p-3" aria-label="Preview">
			<p class="mb-2 text-xs font-medium tracking-wide text-slate-400 uppercase">Preview</p>
			<div class="min-h-0 flex-1">
				{#if previewError}<p class="text-red-700" data-preview-error>{previewError}</p>
				{:else if preview}<BiChart {chart} result={preview} />{/if}
			</div>
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="bi-widget" disabled={busy || !dataset}
			>{widget ? 'Save' : 'Add'}</button
		>
	{/snippet}
</Modal>
