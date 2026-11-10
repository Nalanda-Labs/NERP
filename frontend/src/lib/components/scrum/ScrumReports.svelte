<script lang="ts">
	import { api, query } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { fmtPoints, type Sprint } from '#lib/tasks.ts';

	let { projectId }: { projectId: string } = $props();

	type Day = { date: string; ideal: number; remaining: number | null };
	type Burndown = { committed: number; scope_added: number; scope_removed: number; days: Day[] };
	type Velocity = { sprints: { id: string; name: string; committed: number; completed: number }[]; average: number };

	let sprints = $state<Sprint[]>([]);
	let chosen = $state('');
	let burn = $state<Burndown | null>(null);
	let velocity = $state<Velocity | null>(null);
	let hover = $state<number | null>(null);

	async function load() {
		const [s, v] = await Promise.all([
			api.get<Sprint[]>(`/projects/${projectId}/sprints`),
			api.get<Velocity>(`/projects/${projectId}/velocity`)
		]);
		sprints = s.filter((x) => x.status !== 'planned');
		velocity = v;
		if (!sprints.some((x) => x.id === chosen)) chosen = sprints[0]?.id ?? '';
	}
	$effect(() => {
		projectId;
		load();
	});
	$effect(() => {
		if (!chosen) {
			burn = null;
			return;
		}
		api.get<Burndown>(`/sprints/${chosen}/burndown`).then((b) => (burn = b));
	});

	// ----- burndown geometry -----
	const W = 640, H = 240, L = 36, R = 12, T = 12, B = 28;
	const maxY = $derived(burn ? Math.max(1, burn.committed + burn.scope_added, ...burn.days.map((d) => d.remaining ?? 0)) : 1);
	const x = (i: number) => L + (burn && burn.days.length > 1 ? (i * (W - L - R)) / (burn.days.length - 1) : 0);
	const y = (v: number) => T + (H - T - B) * (1 - v / maxY);
	const ticks = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => Math.round(maxY * f)));
	const idealPath = $derived(burn ? burn.days.map((d, i) => `${i ? 'L' : 'M'}${x(i)},${y(d.ideal)}`).join(' ') : '');
	const realPath = $derived(
		burn
			? burn.days
					.map((d, i) => [d, i] as const)
					.filter(([d]) => d.remaining !== null)
					.map(([d, i], k) => `${k ? 'L' : 'M'}${x(i)},${y(d.remaining!)}`)
					.join(' ')
			: ''
	);
	function onMove(e: PointerEvent) {
		if (!burn) return;
		const svg = e.currentTarget as SVGSVGElement;
		const px = ((e.clientX - svg.getBoundingClientRect().left) / svg.getBoundingClientRect().width) * W;
		const step = (W - L - R) / Math.max(1, burn.days.length - 1);
		hover = Math.min(burn.days.length - 1, Math.max(0, Math.round((px - L) / step)));
	}

	// ----- velocity geometry -----
	const VH = 200;
	const vMax = $derived(velocity ? Math.max(1, ...velocity.sprints.flatMap((s) => [s.committed, s.completed])) : 1);
	const vy = (v: number) => T + (VH - T - B) * (1 - v / vMax);
</script>

<div class="grid gap-5 xl:grid-cols-2">
	<section class="card p-4" aria-label="Burndown">
		<div class="mb-3 flex flex-wrap items-center gap-3">
			<h2 class="text-sm font-semibold text-slate-900">Burndown</h2>
			{#if sprints.length}
				<select class="input ml-auto w-48" aria-label="Sprint" bind:value={chosen}>
					{#each sprints as s (s.id)}<option value={s.id}>{s.name}{s.status === 'active' ? ' (active)' : ''}</option>{/each}
				</select>
			{/if}
		</div>
		{#if !burn}
			<p class="py-10 text-center text-sm text-slate-400">The burndown appears once a sprint has started.</p>
		{:else}
			<div class="mb-2 flex flex-wrap gap-4 text-xs text-slate-600">
				<span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-brand-600"></span>Points left</span>
				<span class="flex items-center gap-1.5"><span class="h-0 w-4 border-t-2 border-dashed border-slate-400"></span>Ideal</span>
				<span class="ml-auto" data-burn-summary>Committed {fmtPoints(burn.committed)}{#if burn.scope_added} · +{fmtPoints(burn.scope_added)} added{/if}{#if burn.scope_removed} · −{fmtPoints(burn.scope_removed)} removed{/if}</span>
			</div>
			<div class="relative">
				<svg viewBox="0 0 {W} {H}" class="w-full" role="img" aria-label="Burndown chart" onpointermove={onMove} onpointerleave={() => (hover = null)}>
					{#each ticks as t (t)}
						<line x1={L} x2={W - R} y1={y(t)} y2={y(t)} class="stroke-slate-100" />
						<text x={L - 6} y={y(t) + 4} text-anchor="end" class="fill-slate-400 text-[12px]">{t}</text>
					{/each}
					{#each burn.days as d, i (d.date)}
						{#if burn.days.length <= 16 || i % Math.ceil(burn.days.length / 8) === 0}
							<text x={x(i)} y={H - 8} text-anchor="middle" class="fill-slate-400 text-[12px]">{d.date.slice(5)}</text>
						{/if}
					{/each}
					<path d={idealPath} fill="none" class="stroke-slate-400" stroke-width="2" stroke-dasharray="5 4" />
					<path d={realPath} fill="none" class="stroke-brand-600" stroke-width="2" stroke-linejoin="round" />
					{#if hover !== null}
						<line x1={x(hover)} x2={x(hover)} y1={T} y2={H - B} class="stroke-slate-300" />
						{#if burn.days[hover]!.remaining !== null}<circle cx={x(hover)} cy={y(burn.days[hover]!.remaining!)} r="4" class="fill-brand-600 stroke-white" stroke-width="2" />{/if}
					{/if}
				</svg>
				{#if hover !== null}
					{@const d = burn.days[hover]!}
					<div class="pointer-events-none absolute top-0 rounded-lg bg-slate-900 px-2.5 py-1.5 text-xs text-white shadow" style="left: min(calc({(x(hover) / W) * 100}% + 8px), calc(100% - 9rem))">
						<div class="font-medium">{formatDate(d.date)}</div>
						<div>Left: {d.remaining === null ? '—' : fmtPoints(d.remaining)}</div>
						<div class="text-slate-300">Ideal: {fmtPoints(d.ideal)}</div>
					</div>
				{/if}
			</div>
		{/if}
	</section>

	<section class="card p-4" aria-label="Velocity">
		<div class="mb-3 flex items-center gap-3">
			<h2 class="text-sm font-semibold text-slate-900">Velocity</h2>
			{#if velocity?.sprints.length}<span class="ml-auto text-xs text-slate-600" data-velocity-average>Average of the last 3: <span class="font-semibold text-slate-900">{fmtPoints(velocity.average)}</span> points</span>{/if}
		</div>
		{#if !velocity?.sprints.length}
			<p class="py-10 text-center text-sm text-slate-400">Velocity appears once a sprint is completed.</p>
		{:else}
			{@const n = velocity.sprints.length}
			{@const band = (W - L - R) / n}
			{@const bw = Math.min(28, band / 3)}
			<div class="mb-2 flex gap-4 text-xs text-slate-600">
				<span class="flex items-center gap-1.5"><span class="size-2.5 rounded-sm bg-slate-300"></span>Committed</span>
				<span class="flex items-center gap-1.5"><span class="size-2.5 rounded-sm bg-brand-600"></span>Completed</span>
			</div>
			<svg viewBox="0 0 {W} {VH}" class="w-full" role="img" aria-label="Velocity chart">
				<line x1={L} x2={W - R} y1={vy(0)} y2={vy(0)} class="stroke-slate-200" />
				{#each velocity.sprints as s, i (s.id)}
					{@const cx = L + band * i + band / 2}
					<g>
						<title>{s.name}: committed {fmtPoints(s.committed)}, completed {fmtPoints(s.completed)}</title>
						<rect x={cx - bw - 1} y={vy(s.committed)} width={bw} height={Math.max(0, vy(0) - vy(s.committed))} rx="3" class="fill-slate-300" />
						<rect x={cx + 1} y={vy(s.completed)} width={bw} height={Math.max(0, vy(0) - vy(s.completed))} rx="3" class="fill-brand-600" />
						<text x={cx + 1 + bw / 2} y={vy(s.completed) - 4} text-anchor="middle" class="fill-slate-600 text-[12px]">{fmtPoints(s.completed)}</text>
						<text x={cx} y={VH - 8} text-anchor="middle" class="fill-slate-500 text-[12px]">{s.name.length > 14 ? s.name.slice(0, 13) + '…' : s.name}</text>
					</g>
				{/each}
			</svg>
		{/if}
	</section>

	<section class="card xl:col-span-2" aria-label="Sprint history">
		<h2 class="border-b border-slate-100 px-4 py-3 text-sm font-semibold text-slate-900">Sprint history</h2>
		<div class="overflow-x-auto">
			<table class="w-full text-sm">
				<thead class="text-left text-xs text-slate-500">
					<tr><th class="px-4 py-2 font-medium">Sprint</th><th class="px-4 py-2 font-medium">Dates</th><th class="px-4 py-2 text-right font-medium">Tasks done</th><th class="px-4 py-2 text-right font-medium">Committed</th><th class="px-4 py-2 text-right font-medium">Completed</th></tr>
				</thead>
				<tbody class="divide-y divide-slate-100">
					{#each sprints as s (s.id)}
						<tr data-history={s.name}>
							<td class="px-4 py-2 text-slate-800">{s.name}{#if s.status === 'active'} <span class="badge bg-emerald-50 text-emerald-700">Active</span>{/if}{#if s.goal}<span class="block text-xs text-slate-500">{s.goal}</span>{/if}</td>
							<td class="px-4 py-2 text-slate-600">{formatDate(s.starts_at)} – {formatDate(s.completed_at ?? s.ends_at)}</td>
							<td class="px-4 py-2 text-right text-slate-700">{s.done_count} / {s.task_count}</td>
							<td class="px-4 py-2 text-right text-slate-700">{fmtPoints(s.committed_points)}</td>
							<td class="px-4 py-2 text-right font-medium text-slate-900">{s.status === 'completed' ? fmtPoints(s.completed_points) : fmtPoints(s.points_done)}</td>
						</tr>
					{:else}
						<tr><td colspan="5" class="px-4 py-6 text-center text-slate-400">No sprints have run yet.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	</section>
</div>
