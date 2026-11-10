<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { replaceState } from '$app/navigation';
	import { api, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { crm, money, moneyShort, optionLabel } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Kpi = { won_count: number; won_amount: string; lost_count: number; new_count: number; new_amount: string; avg_cycle_days: number | null };
	type Stage = { stage_id: string; name: string; color: string; semantics: 'process' | 'success' | 'failure'; reached: number; reached_amount: string; current: number };
	type Point = { start: string; won_count: number; won_amount: string; new_count: number };
	type Manager = { user_id: string | null; name: string | null; avatar_file_id: string | null; won_count: number; won_amount: string; lost_count: number; new_count: number; open_count: number; open_amount: string; activities_done: number; target: string | null; target_won: string };
	type Report = {
		type: string;
		currency: string;
		timezone: string;
		period: { from: string; to: string; days: number; prev_from: string; prev_to: string; bucket: 'day' | 'week' | 'month' };
		kpi: { current: Kpi; previous: Kpi; open_count: number; open_amount: string };
		funnel_category_id: string | null;
		funnel: Stage[] | null;
		dynamics: Point[];
		managers: Manager[];
		sources: { source: string; created: number; won_count: number; won_amount: string }[];
		lead_conversion: { created: number; converted: number; junk: number; open: number } | null;
		targets: { month: string; team_target: string | null; team_won: string; users: { user_id: string; name: string; target: string | null; won: string }[] };
		can_edit_targets: boolean;
	};

	// ---- filters (kept in the address so a view can be bookmarked) ----
	const PRESETS = [
		['this_month', 'This month'],
		['last_month', 'Last month'],
		['last_30', 'Last 30 days'],
		['this_quarter', 'This quarter'],
		['this_year', 'This year'],
		['last_year', 'Last year'],
		['custom', 'Custom']
	] as const;
	type Preset = (typeof PRESETS)[number][0];
	const p0 = page.url.searchParams;
	let typeCode = $state(p0.get('type') ?? 'deal');
	let preset = $state<Preset>((PRESETS.find(([k]) => k === p0.get('period'))?.[0] ?? (p0.get('from') ? 'custom' : 'this_month')) as Preset);
	let from = $state(p0.get('from') ?? '');
	let to = $state(p0.get('to') ?? '');
	let category = $state(p0.get('pipeline') ?? '');
	let people = $state<PickedUser[]>([]);
	const initialUsers = p0.get('users');

	const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
	function range(p: Preset): [string, string] {
		const now = new Date();
		const y = now.getFullYear(), m = now.getMonth();
		switch (p) {
			case 'this_month': return [iso(new Date(y, m, 1)), iso(now)];
			case 'last_month': return [iso(new Date(y, m - 1, 1)), iso(new Date(y, m, 0))];
			case 'last_30': return [iso(new Date(y, m, now.getDate() - 29)), iso(now)];
			case 'this_quarter': return [iso(new Date(y, m - (m % 3), 1)), iso(now)];
			case 'this_year': return [iso(new Date(y, 0, 1)), iso(now)];
			case 'last_year': return [iso(new Date(y - 1, 0, 1)), iso(new Date(y - 1, 11, 31))];
			default: return [from || iso(new Date(y, m, 1)), to || iso(now)];
		}
	}

	const amountTypes = $derived((crm.config?.types ?? []).filter((t) => t.has_stages && t.has_amount));
	const type = $derived(amountTypes.find((t) => t.code === typeCode));
	const pipelines = $derived(type?.categories ?? []);

	let report = $state<Report | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let seq = 0;

	async function load() {
		const [f, t] = range(preset);
		if (preset !== 'custom') [from, to] = [f, t];
		const params = { type: typeCode === 'deal' ? undefined : typeCode, period: preset === 'custom' ? undefined : preset, from: preset === 'custom' ? f : undefined, to: preset === 'custom' ? t : undefined, pipeline: category || undefined, users: people.map((p) => p.id).join(',') || undefined };
		try {
			replaceState('/crm/analytics' + query(params), {});
		} catch {
			/* router not ready */
		}
		const mine = ++seq;
		loading = true;
		try {
			const r = await api.get<Report>(`/crm/analytics/${typeCode}` + query({ from: f, to: t, category_id: category || undefined, users: params.users }));
			if (mine !== seq) return;
			report = r;
			error = null;
		} catch (e) {
			if (mine === seq) error = (e as Error).message;
		} finally {
			if (mine === seq) loading = false;
		}
	}

	onMount(async () => {
		await crm.load();
		if (!amountTypes.some((t) => t.code === typeCode)) typeCode = 'deal';
		// Default to the default pipeline when there are several (the funnel needs one).
		if (!p0.has('pipeline') && pipelines.length > 1) category = pipelines.find((c) => c.is_default)?.id ?? '';
		if (initialUsers) {
			try {
				const found = await Promise.all(initialUsers.split(',').map((id) => api.get<{ id: string; first_name: string; last_name: string; avatar_file_id: string | null }>(`/users/${id}`)));
				people = found.map((u) => ({ id: u.id, name: `${u.first_name} ${u.last_name}`.trim(), avatar_file_id: u.avatar_file_id }));
			} catch {
				/* ignore unknown people */
			}
		}
		load();
	});
	// Reload when the people filter changes (after the first load).
	let peopleKey = '';
	$effect(() => {
		const k = people.map((p) => p.id).join(',');
		if (report && k !== peopleKey) {
			peopleKey = k;
			load();
		} else peopleKey = k;
	});

	// ---- formatting ----
	const cur = $derived(report?.currency ?? 'USD');
	const fmtMoney = (v: string | number) => money(v, cur);
	const fmtShort = (v: string | number) => moneyShort(v, cur);
	const num = (v: number) => v.toLocaleString();
	const pct = (v: number | null) => (v === null || !Number.isFinite(v) ? '—' : `${(v * 100).toFixed(v < 0.1 && v > 0 ? 1 : 0)}%`);
	function delta(c: number, p: number): { text: string; dir: 'up' | 'down' | 'flat' } | null {
		if (!p && !c) return null;
		if (!p) return { text: 'new', dir: 'up' };
		const d = (c - p) / Math.abs(p);
		if (Math.abs(d) < 0.005) return { text: '0%', dir: 'flat' };
		return { text: `${d > 0 ? '+' : ''}${(d * 100).toFixed(0)}%`, dir: d > 0 ? 'up' : 'down' };
	}
	function cycleText(d: number | null) {
		if (d === null) return 'no wins yet';
		if (d < 1) return 'won within a day on average';
		return `won in ${d < 10 ? d.toFixed(1) : Math.round(d)} days on average`;
	}
	const winRate = (k: Kpi) => (k.won_count + k.lost_count ? k.won_count / (k.won_count + k.lost_count) : null);
	const avgWon = (k: Kpi) => (k.won_count ? Number(k.won_amount) / k.won_count : 0);
	const fmtDay = (s: string, opts: Intl.DateTimeFormatOptions) => new Date(s + 'T00:00:00').toLocaleDateString(undefined, opts);
	function bucketLabel(s: string, long = false) {
		const b = report?.period.bucket;
		if (b === 'month') return fmtDay(s, long ? { month: 'long', year: 'numeric' } : { month: 'short' });
		if (b === 'week') return (long ? 'Week of ' : '') + fmtDay(s, { month: 'short', day: 'numeric' });
		return fmtDay(s, long ? { weekday: 'short', month: 'short', day: 'numeric' } : { day: 'numeric' });
	}
	const periodText = $derived(report ? `${fmtDay(report.period.from, { month: 'short', day: 'numeric', year: 'numeric' })} – ${fmtDay(report.period.to, { month: 'short', day: 'numeric', year: 'numeric' })}` : '');
	const prevText = $derived(report ? `vs ${fmtDay(report.period.prev_from, { month: 'short', day: 'numeric' })} – ${fmtDay(report.period.prev_to, { month: 'short', day: 'numeric', year: 'numeric' })}` : '');

	// ---- KPI tiles ----
	const tiles = $derived.by(() => {
		if (!report) return [];
		const c = report.kpi.current, p = report.kpi.previous;
		const wr = winRate(c), wrp = winRate(p);
		return [
			{ key: 'won', label: 'Won', value: fmtMoney(c.won_amount), sub: `${num(c.won_count)} ${c.won_count === 1 ? 'deal' : 'deals'}`, d: delta(Number(c.won_amount), Number(p.won_amount)) },
			{ key: 'rate', label: 'Win rate', value: pct(wr), sub: `${num(c.won_count)} won · ${num(c.lost_count)} lost`, d: wr !== null && wrp !== null ? delta(wr, wrp) : null },
			{ key: 'avg', label: 'Average won', value: c.won_count ? fmtMoney(avgWon(c)) : '—', sub: cycleText(c.avg_cycle_days), d: c.won_count && p.won_count ? delta(avgWon(c), avgWon(p)) : null },
			{ key: 'new', label: 'New', value: num(c.new_count), sub: fmtMoney(c.new_amount), d: delta(c.new_count, p.new_count) },
			{ key: 'open', label: 'Open now', value: fmtMoney(report.kpi.open_amount), sub: `${num(report.kpi.open_count)} in progress`, d: null }
		];
	});

	// ---- sales over time (column chart) ----
	let metric = $state<'won_amount' | 'won_count' | 'new_count'>('won_amount');
	const METRICS = [['won_amount', 'Won amount'], ['won_count', 'Won count'], ['new_count', 'New']] as const;
	let chartW = $state(640);
	const chartH = 220, padL = 56, padR = 8, padT = 12, padB = 28;
	const values = $derived((report?.dynamics ?? []).map((p) => Number(p[metric])));
	function niceMax(v: number) {
		if (v <= 0) return metric === 'won_amount' ? 100 : 4;
		const pow = 10 ** Math.floor(Math.log10(v));
		const n = v / pow;
		const step = n <= 1 ? 1 : n <= 2 ? 2 : n <= 2.5 ? 2.5 : n <= 5 ? 5 : 10;
		const m = step * pow;
		return metric === 'won_amount' ? m : Math.max(4, Math.ceil(m));
	}
	const yMax = $derived(niceMax(Math.max(0, ...values)));
	const ticks = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => f * yMax).filter((t) => metric === 'won_amount' || Number.isInteger(t)));
	const plotW = $derived(Math.max(10, chartW - padL - padR));
	const band = $derived(values.length ? plotW / values.length : plotW);
	const barW = $derived(Math.max(2, Math.min(24, band * 0.6)));
	const yOf = (v: number) => padT + (chartH - padT - padB) * (1 - v / yMax);
	const labelEvery = $derived(Math.max(1, Math.ceil(48 / band)));
	let hover = $state<number | null>(null);
	const fmtMetric = (v: number) => (metric === 'won_amount' ? fmtMoney(v) : num(v));
	const fmtTick = (v: number) => (metric === 'won_amount' ? fmtShort(v) : num(v));
	function barPath(x: number, y: number, w: number, h: number) {
		if (h <= 0) return '';
		const r = Math.min(4, w / 2, h);
		const b = y + h;
		return `M${x},${b} L${x},${y + r} Q${x},${y} ${x + r},${y} L${x + w - r},${y} Q${x + w},${y} ${x + w},${y + r} L${x + w},${b} Z`;
	}
	let showTable = $state(false);

	// ---- funnel ----
	const openStages = $derived((report?.funnel ?? []).filter((s) => s.semantics === 'process'));
	const wonStage = $derived((report?.funnel ?? []).filter((s) => s.semantics === 'success'));
	const lostStages = $derived((report?.funnel ?? []).filter((s) => s.semantics === 'failure'));
	const funnelTop = $derived(Math.max(1, openStages[0]?.reached ?? 0, ...openStages.map((s) => s.reached)));

	// ---- sources ----
	const srcMax = $derived(Math.max(1, ...(report?.sources ?? []).map((s) => s.created)));

	// ---- lead conversion ----
	const leadParts = $derived.by(() => {
		const l = report?.lead_conversion;
		if (!l || !l.created) return [];
		return [
			{ key: 'converted', label: 'Converted', n: l.converted, color: 'var(--series-1)' },
			{ key: 'open', label: 'Still open', n: l.open, color: 'var(--series-2)' },
			{ key: 'junk', label: 'Junk', n: l.junk, color: 'var(--series-3)' }
		];
	});

	// ---- targets ----
	const targetPct = (won: string | number, target: string | number | null) => (target && Number(target) > 0 ? Number(won) / Number(target) : null);
	const monthName = $derived(report ? fmtDay(report.targets.month, { month: 'long', year: 'numeric' }) : '');
	let editingTargets = $state(false);
	let tMonth = $state('');
	let tTeam = $state<string | number>('');
	let tUsers = $state<{ user_id: string; name: string; avatar_file_id: string | null; target: string | number }[]>([]);
	let tSaving = $state(false);
	async function openTargets(month?: string) {
		tMonth = month ?? report?.targets.month.slice(0, 7) ?? iso(new Date()).slice(0, 7);
		try {
			const r = await api.get<{ team_target: string | null; users: { user_id: string; name: string; avatar_file_id: string | null; target: string | null }[] }>(`/crm/analytics/${typeCode}/targets` + query({ month: tMonth }));
			tTeam = r.team_target ? String(Number(r.team_target)) : '';
			tUsers = r.users.map((u) => ({ ...u, target: u.target ? String(Number(u.target)) : '' }));
			editingTargets = true;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function saveTargets(e: SubmitEvent) {
		e.preventDefault();
		tSaving = true;
		try {
			const n = (v: unknown) => (v === null || v === undefined || String(v).trim() === '' ? null : String(v).trim());
			await api.put(`/crm/analytics/${typeCode}/targets`, { month: tMonth, team_target: n(tTeam), users: tUsers.filter((u) => n(u.target) !== null).map((u) => ({ user_id: u.user_id, target: n(u.target) })) });
			toasts.success('Targets saved');
			editingTargets = false;
			load();
		} catch (err) {
			toasts.error(err);
		} finally {
			tSaving = false;
		}
	}
</script>

<svelte:head><title>Analytics · NERP</title></svelte:head>

<div class="viz-root space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Sales analytics</h1>
		{#if report}<span class="text-sm text-slate-500">{periodText}</span>{/if}
		{#if loading && report}<span class="text-xs text-slate-400">Updating…</span>{/if}
	</div>

	<!-- Filters: one row above the charts -->
	<div class="card flex flex-wrap items-end gap-3 p-4">
		{#if amountTypes.length > 1}
			<div>
				<label class="label" for="an-type">Records</label>
				<select id="an-type" class="input" bind:value={typeCode} onchange={() => ((category = pipelines.length > 1 ? (pipelines.find((c) => c.is_default)?.id ?? '') : ''), load())}>
					{#each amountTypes as t (t.code)}<option value={t.code}>{t.name_plural}</option>{/each}
				</select>
			</div>
		{/if}
		<div>
			<label class="label" for="an-period">Period</label>
			<select id="an-period" class="input" bind:value={preset} onchange={load}>
				{#each PRESETS as [k, l] (k)}<option value={k}>{l}</option>{/each}
			</select>
		</div>
		{#if preset === 'custom'}
			<div><label class="label" for="an-from">From</label><input id="an-from" type="date" class="input" bind:value={from} max={to || undefined} onchange={load} /></div>
			<div><label class="label" for="an-to">To</label><input id="an-to" type="date" class="input" bind:value={to} min={from || undefined} onchange={load} /></div>
		{/if}
		{#if pipelines.length > 1}
			<div>
				<label class="label" for="an-pipe">Pipeline</label>
				<select id="an-pipe" class="input" bind:value={category} onchange={load}>
					<option value="">All pipelines</option>
					{#each pipelines as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
				</select>
			</div>
		{/if}
		<div class="min-w-64 flex-1">
			<span class="label">Managers</span>
			<MultiUserPicker bind:value={people} placeholder="Everyone — add people to narrow" />
		</div>
	</div>

	{#if error}
		<p class="rounded-lg bg-red-50 px-4 py-3 text-sm text-red-800" role="alert">{error}</p>
	{:else if !report}
		<p class="py-16 text-center text-sm text-slate-500">Loading…</p>
	{:else}
		<!-- Headline figures -->
		<section aria-label="Headline figures" class="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
			{#each tiles as t (t.key)}
				<div class="card p-4" data-tile={t.key}>
					<p class="text-xs font-medium text-slate-500">{t.label}</p>
					<p class="mt-1 text-2xl font-semibold text-slate-900 tabular-nums">{t.value}</p>
					<p class="mt-0.5 text-xs text-slate-500">{t.sub}</p>
					{#if t.d}
						<p class="mt-2 flex items-center gap-1 text-xs text-slate-600" title={prevText}>
							<span aria-hidden="true" class={t.d.dir === 'up' ? 'text-emerald-700' : t.d.dir === 'down' ? 'text-red-700' : 'text-slate-400'}>{t.d.dir === 'up' ? '▲' : t.d.dir === 'down' ? '▼' : '■'}</span>
							<span class="tabular-nums">{t.d.text}</span><span class="text-slate-400">vs previous period</span>
						</p>
					{/if}
				</div>
			{/each}
		</section>

		<div class="grid items-start gap-5 xl:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
			<!-- Sales over time -->
			<section class="card p-5" aria-labelledby="dyn-h">
				<div class="mb-3 flex flex-wrap items-center gap-2">
					<h2 id="dyn-h" class="font-medium text-slate-900">Sales over time</h2>
					<span class="text-xs text-slate-500">by {report.period.bucket}</span>
					<div class="ml-auto flex rounded-full bg-slate-100 p-0.5 text-xs" role="tablist" aria-label="Measure">
						{#each METRICS as [k, l] (k)}
							<button role="tab" aria-selected={metric === k} class="rounded-full px-3 py-1 {metric === k ? 'bg-white font-medium text-slate-900 shadow-sm' : 'text-slate-600'}" onclick={() => (metric = k)}>{l}</button>
						{/each}
					</div>
					<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => (showTable = !showTable)}>{showTable ? 'Chart' : 'Table'}</button>
				</div>
				{#if showTable}
					<div class="max-h-64 overflow-auto">
						<table class="min-w-full text-sm">
							<thead class="text-left text-xs text-slate-500"><tr><th class="py-1.5">Period</th><th class="py-1.5 text-right">Won amount</th><th class="py-1.5 text-right">Won</th><th class="py-1.5 text-right">New</th></tr></thead>
							<tbody class="divide-y divide-slate-50 tabular-nums">
								{#each report.dynamics as p (p.start)}<tr><td class="py-1.5">{bucketLabel(p.start, true)}</td><td class="py-1.5 text-right">{fmtMoney(p.won_amount)}</td><td class="py-1.5 text-right">{num(p.won_count)}</td><td class="py-1.5 text-right">{num(p.new_count)}</td></tr>{/each}
							</tbody>
						</table>
					</div>
				{:else}
					<div class="relative" bind:clientWidth={chartW}>
						<svg width={chartW} height={chartH} role="img" aria-label="{METRICS.find((m) => m[0] === metric)?.[1]} by {report.period.bucket}">
							{#each ticks as t (t)}
								<line x1={padL} x2={chartW - padR} y1={yOf(t)} y2={yOf(t)} stroke="var(--grid)" stroke-width="1" />
								<text x={padL - 8} y={yOf(t)} dy="0.32em" text-anchor="end" class="fill-[var(--text-muted)] text-[11px] tabular-nums">{fmtTick(t)}</text>
							{/each}
							{#each report.dynamics as p, i (p.start)}
								{@const v = values[i] ?? 0}
								{@const x = padL + i * band + (band - barW) / 2}
								{@const h = (chartH - padT - padB) * (v / yMax)}
								<path d={barPath(x, yOf(v), barW, h)} fill="var(--series-1)" opacity={hover === null || hover === i ? 1 : 0.45} />
								{#if i % labelEvery === 0}
									<text x={padL + i * band + band / 2} y={chartH - 8} text-anchor="middle" class="fill-[var(--text-muted)] text-[11px]">{bucketLabel(p.start)}</text>
								{/if}
								<!-- Hit target: the whole band -->
								<rect x={padL + i * band} y={padT} width={band} height={chartH - padT - padB} fill="transparent" role="presentation" onmouseenter={() => (hover = i)} onmouseleave={() => (hover = null)} />
							{/each}
							<line x1={padL} x2={chartW - padR} y1={yOf(0)} y2={yOf(0)} stroke="var(--axis)" stroke-width="1" />
						</svg>
						{#if hover !== null && report.dynamics[hover]}
							{@const p = report.dynamics[hover]}
							{@const left = Math.min(Math.max(padL + hover * band + band / 2, 90), chartW - 90)}
							<div class="pointer-events-none absolute top-0 z-10 w-44 -translate-x-1/2 rounded-lg bg-white p-2.5 text-xs shadow-g2" style="left:{left}px" role="tooltip">
								<p class="mb-1 font-medium text-slate-900">{bucketLabel(p.start, true)}</p>
								<p class="flex justify-between gap-2"><span class="text-slate-500">Won amount</span><span class="tabular-nums">{fmtMoney(p.won_amount)}</span></p>
								<p class="flex justify-between gap-2"><span class="text-slate-500">Won</span><span class="tabular-nums">{num(p.won_count)}</span></p>
								<p class="flex justify-between gap-2"><span class="text-slate-500">New</span><span class="tabular-nums">{num(p.new_count)}</span></p>
							</div>
						{/if}
					</div>
				{/if}
			</section>

			<!-- Funnel -->
			<section class="card p-5" aria-labelledby="fun-h">
				<div class="mb-1 flex items-baseline gap-2">
					<h2 id="fun-h" class="font-medium text-slate-900">Funnel</h2>
					{#if report.funnel}<span class="text-xs text-slate-500">{type?.name_plural ?? 'Records'} created in the period, by how far they got</span>{/if}
				</div>
				{#if !report.funnel}
					<p class="py-10 text-center text-sm text-slate-500">Choose one pipeline to see its funnel.</p>
				{:else if !openStages.length}
					<p class="py-10 text-center text-sm text-slate-500">This pipeline has no stages.</p>
				{:else}
					<ul class="mt-3 max-h-[22rem] space-y-2.5 overflow-y-auto pr-1">
						{#each openStages as s, i (s.stage_id)}
							{@const prev = i ? openStages[i - 1]!.reached : null}
							<li title="{s.name}: {num(s.reached)} reached ({fmtMoney(s.reached_amount)}), {num(s.current)} there now">
								<div class="mb-1 flex items-baseline justify-between gap-2 text-sm">
									<span class="flex min-w-0 items-center gap-1.5"><span class="size-2 shrink-0 rounded-full" style="background:{s.color}" aria-hidden="true"></span><span class="truncate text-slate-800">{s.name}</span></span>
									<span class="shrink-0 text-slate-900 tabular-nums">{num(s.reached)}{#if prev}<span class="ml-1.5 text-xs text-slate-500">{pct(prev ? s.reached / prev : null)} of previous</span>{/if}</span>
								</div>
								<div class="h-3 rounded-full bg-slate-100"><div class="h-3 rounded-full" style="width:{(s.reached / funnelTop) * 100}%;background:var(--series-1)"></div></div>
							</li>
						{/each}
					</ul>
					<div class="mt-4 grid grid-cols-2 gap-3 border-t border-slate-100 pt-3 text-sm">
						<div>
							<p class="text-xs text-slate-500">Won</p>
							<p class="font-medium text-slate-900 tabular-nums">{num(wonStage.reduce((a, s) => a + s.reached, 0))} <span class="text-xs font-normal text-slate-500">{pct(openStages[0]?.reached ? wonStage.reduce((a, s) => a + s.reached, 0) / openStages[0].reached : null)} of all</span></p>
						</div>
						<div>
							<p class="text-xs text-slate-500">Lost</p>
							<p class="font-medium text-slate-900 tabular-nums">{num(lostStages.reduce((a, s) => a + s.reached, 0))}{#if lostStages.length > 1}<span class="text-xs font-normal text-slate-500"> · {lostStages.map((s) => `${s.name} ${s.reached}`).join(', ')}</span>{/if}</p>
						</div>
					</div>
				{/if}
			</section>
		</div>

		<!-- Managers and targets -->
		<section class="card" aria-labelledby="mgr-h">
			<div class="flex flex-wrap items-center gap-3 border-b border-slate-100 px-5 py-3">
				<h2 id="mgr-h" class="font-medium text-slate-900">Managers</h2>
				<span class="text-xs text-slate-500">Targets are for {monthName}</span>
				{#if report.targets.team_target}
					{@const tp = targetPct(report.targets.team_won, report.targets.team_target) ?? 0}
					<div class="flex min-w-60 items-center gap-2 text-xs text-slate-600" data-team-target>
						<span>Team</span>
						<div class="h-2 w-28 rounded-full bg-slate-100" role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(tp * 100)} aria-label="Team target"><div class="h-2 rounded-full" style="width:{Math.min(1, tp) * 100}%;background:var(--series-1)"></div></div>
						<span class="tabular-nums">{fmtShort(report.targets.team_won)} of {fmtShort(report.targets.team_target)} · {pct(tp)}</span>
					</div>
				{/if}
				{#if report.can_edit_targets}<button class="btn btn-ghost ml-auto text-sm" onclick={() => openTargets()}><Icon name="edit" class="size-4" /> Set targets</button>{/if}
			</div>
			<div class="overflow-x-auto">
				<table class="min-w-full text-sm">
					<thead class="text-left text-xs font-medium text-slate-500">
						<tr>
							<th class="px-5 py-2.5">#</th><th class="px-3 py-2.5">Manager</th><th class="px-3 py-2.5 text-right">Won</th><th class="px-3 py-2.5 text-right">Deals won</th><th class="px-3 py-2.5 text-right">Win rate</th><th class="px-3 py-2.5 text-right">New</th><th class="px-3 py-2.5 text-right">Open now</th><th class="px-3 py-2.5 text-right">Activities done</th><th class="px-5 py-2.5">Target ({fmtDay(report.targets.month, { month: 'short' })})</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-50 tabular-nums">
						{#each report.managers as m, i (m.user_id ?? 'none')}
							{@const rate = m.won_count + m.lost_count ? m.won_count / (m.won_count + m.lost_count) : null}
							{@const tp = targetPct(m.target_won, m.target)}
							<tr>
								<td class="px-5 py-2.5 text-slate-400">{i + 1}</td>
								<td class="px-3 py-2.5"><span class="flex items-center gap-2 whitespace-nowrap">{#if m.user_id}<Avatar name={m.name ?? '?'} fileId={m.avatar_file_id} size="xs" />{/if}<span class="text-slate-800">{m.name ?? 'Unassigned'}</span></span></td>
								<td class="px-3 py-2.5 text-right font-medium text-slate-900">{fmtMoney(m.won_amount)}</td>
								<td class="px-3 py-2.5 text-right">{num(m.won_count)}</td>
								<td class="px-3 py-2.5 text-right">{pct(rate)}</td>
								<td class="px-3 py-2.5 text-right">{num(m.new_count)}</td>
								<td class="px-3 py-2.5 text-right">{num(m.open_count)} <span class="text-xs text-slate-500">{fmtShort(m.open_amount)}</span></td>
								<td class="px-3 py-2.5 text-right">{num(m.activities_done)}</td>
								<td class="px-5 py-2.5">
									{#if tp !== null}
										<div class="flex items-center gap-2 whitespace-nowrap text-xs">
											<div class="h-2 w-24 rounded-full bg-slate-100" role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(tp * 100)} aria-label="Target for {m.name}"><div class="h-2 rounded-full" style="width:{Math.min(1, tp) * 100}%;background:var(--series-1)"></div></div>
											<span>{pct(tp)} of {fmtShort(m.target!)}</span>
										</div>
									{:else}<span class="text-xs text-slate-400">—</span>{/if}
								</td>
							</tr>
						{:else}
							<tr><td colspan="9" class="px-5 py-10 text-center text-slate-500">Nothing in this period.</td></tr>
						{/each}
					</tbody>
				</table>
			</div>
		</section>

		<div class="grid items-start gap-5 lg:grid-cols-2">
			<!-- Sources -->
			<section class="card p-5" aria-labelledby="src-h">
				<h2 id="src-h" class="font-medium text-slate-900">Sources</h2>
				<p class="mb-3 text-xs text-slate-500">Created in the period, and how many are won by now</p>
				{#if report.sources.length}
					<ul class="space-y-2.5">
						{#each report.sources as s (s.source)}
							<li title="{optionLabel(s.source)}: {num(s.created)} created, {num(s.won_count)} won ({fmtMoney(s.won_amount)})">
								<div class="mb-1 flex justify-between gap-2 text-sm"><span class="text-slate-800">{s.source === 'none' ? 'Not set' : optionLabel(s.source)}</span><span class="tabular-nums text-slate-900">{num(s.created)} <span class="text-xs text-slate-500">· {num(s.won_count)} won · {pct(s.won_count / s.created)}</span></span></div>
								<div class="h-3 rounded-full bg-slate-100"><div class="h-3 rounded-full" style="width:{(s.created / srcMax) * 100}%;background:var(--series-1)"></div></div>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="py-8 text-center text-sm text-slate-500">Nothing created in this period.</p>
				{/if}
			</section>

			<!-- Lead conversion -->
			<section class="card p-5" aria-labelledby="lead-h">
				<h2 id="lead-h" class="font-medium text-slate-900">Lead conversion</h2>
				{#if report.lead_conversion === null}
					<p class="py-8 text-center text-sm text-slate-500">{report.type === 'deal' ? 'You can’t see leads.' : 'Lead conversion is shown for deals.'}</p>
				{:else if !report.lead_conversion.created}
					<p class="py-8 text-center text-sm text-slate-500">No leads created in this period.</p>
				{:else}
					{@const l = report.lead_conversion}
					<p class="mb-3 text-xs text-slate-500">{num(l.created)} leads created in the period</p>
					<p class="text-2xl font-semibold text-slate-900 tabular-nums">{pct(l.converted / l.created)} <span class="text-sm font-normal text-slate-500">converted</span></p>
					<div class="mt-3 flex h-7 gap-0.5" role="img" aria-label="Converted {l.converted}, still open {l.open}, junk {l.junk}">
						{#each leadParts.filter((p) => p.n > 0) as p (p.key)}
							<div class="rounded" style="flex:{p.n} 1 0;background:{p.color}" title="{p.label}: {num(p.n)} ({pct(p.n / l.created)})"></div>
						{/each}
					</div>
					<ul class="mt-3 flex flex-wrap gap-x-5 gap-y-1 text-xs text-slate-600">
						{#each leadParts as p (p.key)}
							<li class="flex items-center gap-1.5"><span class="size-2.5 rounded-sm" style="background:{p.color}" aria-hidden="true"></span>{p.label} <span class="tabular-nums text-slate-900">{num(p.n)}</span><span class="tabular-nums text-slate-500">{pct(p.n / l.created)}</span></li>
						{/each}
					</ul>
				{/if}
			</section>
		</div>

		<p class="text-xs text-slate-400">
			Amounts are in {cur}, converted at the rates in CRM settings. Days follow your time zone ({report.timezone}). Won and lost count by closing date; new and the funnel count by creation date. Only records you can view are included.
		</p>
	{/if}
</div>

<Modal bind:open={editingTargets} title="Sales targets">
	<form id="targets" class="space-y-4" onsubmit={saveTargets}>
		<div class="grid gap-3 sm:grid-cols-2">
			<div>
				<label class="label" for="t-month">Month</label>
				<input id="t-month" type="month" class="input" required bind:value={tMonth} onchange={() => openTargets(tMonth)} />
			</div>
			<div>
				<label class="label" for="t-team">Team target ({cur})</label>
				<input id="t-team" type="number" min="0" step="any" class="input" placeholder="No target" bind:value={tTeam} />
			</div>
		</div>
		<div>
			<p class="label">Per manager ({cur})</p>
			<ul class="max-h-80 divide-y divide-slate-50 overflow-auto">
				{#each tUsers as u (u.user_id)}
					<li class="flex items-center gap-3 py-1.5">
						<Avatar name={u.name} fileId={u.avatar_file_id} size="xs" />
						<label class="flex-1 truncate text-sm text-slate-800" for="t-{u.user_id}">{u.name}</label>
						<input id="t-{u.user_id}" type="number" min="0" step="any" class="input w-40 py-1.5 text-right" placeholder="—" bind:value={u.target} />
					</li>
				{/each}
			</ul>
		</div>
		<p class="text-xs text-slate-500">Progress counts {type?.name_plural?.toLowerCase() ?? 'records'} won in that month. Leave a field empty for no target.</p>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (editingTargets = false)}>Cancel</button>
		<button class="btn btn-primary" form="targets" disabled={tSaving}>{tSaving ? 'Saving…' : 'Save targets'}</button>
	{/snippet}
</Modal>

