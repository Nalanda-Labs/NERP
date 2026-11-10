<script lang="ts">
	import { goto } from '$app/navigation';
	import { api } from '#lib/api.ts';
	import { STATUS, type TaskDetail, type TaskSummary } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let {
		tasks = $bindable<TaskSummary[]>([]),
		dependencies = [],
		onchanged
	}: {
		tasks?: TaskSummary[];
		dependencies?: { task_id: string; depends_on_id: string }[];
		onchanged?: () => void;
	} = $props();

	const DAY = 864e5;
	const ROW = 38;
	const HEADER = 48;
	type Zoom = 'day' | 'week' | 'month';
	let zoom = $state<Zoom>('day');
	const dayWidth = $derived(({ day: 36, week: 14, month: 4 }) [zoom]);

	const startOf = (t: TaskSummary) => new Date(t.start_date ?? t.deadline ?? t.created_at).getTime();
	const endOf = (t: TaskSummary) => {
		const s = startOf(t);
		const e = t.deadline ? new Date(t.deadline).getTime() : s + DAY;
		return Math.max(e, s + DAY / 4);
	};
	const hasDates = (t: TaskSummary) => !!(t.start_date || t.deadline);

	// Visible range: from a few days before the earliest task to a week after the latest.
	const range = $derived.by(() => {
		const now = Date.now();
		let lo = now - 3 * DAY;
		let hi = now + 21 * DAY;
		for (const t of tasks) {
			lo = Math.min(lo, startOf(t) - 3 * DAY);
			hi = Math.max(hi, endOf(t) + 7 * DAY);
		}
		const start = new Date(lo);
		start.setHours(0, 0, 0, 0);
		start.setDate(start.getDate() - ((start.getDay() + 6) % 7)); // back to Monday
		const days = Math.ceil((hi - start.getTime()) / DAY);
		return { start: start.getTime(), days };
	});

	const x = (ms: number) => ((ms - range.start) / DAY) * dayWidth;

	const days = $derived(Array.from({ length: range.days }, (_, i) => new Date(range.start + i * DAY)));
	const months = $derived.by(() => {
		const out: { label: string; left: number; width: number }[] = [];
		for (const d of days) {
			const label = d.toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
			const last = out[out.length - 1];
			if (last && last.label === label) last.width += dayWidth;
			else out.push({ label, left: x(d.getTime()), width: dayWidth });
		}
		return out;
	});

	const rowOf = $derived(new Map(tasks.map((t, i) => [t.id, i])));
	const width = $derived(range.days * dayWidth);
	const height = $derived(tasks.length * ROW);

	// ----- drag to move / resize -----
	type Drag = { id: string; mode: 'move' | 'end'; x0: number; dx: number; moved: boolean };
	let drag = $state<Drag | null>(null);

	// The drag is followed on the window, not on the bar: the bar may be re-rendered while it
	// moves (e.g. when a live update refreshes the list), and the release must still count.
	function begin(e: PointerEvent, t: TaskSummary, mode: Drag['mode']) {
		if (e.button !== 0) return;
		e.preventDefault();
		e.stopPropagation();
		drag = { id: t.id, mode, x0: e.clientX, dx: 0, moved: false };
		window.addEventListener('pointermove', onMove);
		window.addEventListener('pointerup', onUp);
		window.addEventListener('pointercancel', onCancel);
	}
	function stopFollowing() {
		window.removeEventListener('pointermove', onMove);
		window.removeEventListener('pointerup', onUp);
		window.removeEventListener('pointercancel', onCancel);
	}
	function onMove(e: PointerEvent) {
		if (!drag) return;
		drag.dx = e.clientX - drag.x0;
		if (Math.abs(drag.dx) > 3) drag.moved = true;
	}
	function onUp() {
		stopFollowing();
		end();
	}
	function onCancel() {
		stopFollowing();
		drag = null;
	}
	$effect(() => stopFollowing);
	const snapDays = (dx: number) => Math.round(dx / dayWidth);

	async function end() {
		const d = drag;
		drag = null;
		if (!d) return;
		const t = tasks.find((x) => x.id === d.id);
		if (!t) return;
		if (!d.moved) {
			goto(`/tasks/${t.id}`);
			return;
		}
		const shift = snapDays(d.dx) * DAY;
		if (!shift) return;
		const start = startOf(t);
		const finish = endOf(t);
		const body =
			d.mode === 'move'
				? { start_date: new Date(start + shift).toISOString(), deadline: new Date(finish + shift).toISOString() }
				: { deadline: new Date(Math.max(start + DAY / 4, finish + shift)).toISOString(), start_date: new Date(start).toISOString() };
		const prev = { start_date: t.start_date, deadline: t.deadline };
		Object.assign(t, body);
		try {
			const saved = await api.patch<TaskDetail>(`/tasks/${t.id}`, body);
			Object.assign(t, { start_date: saved.start_date, deadline: saved.deadline, is_overdue: saved.is_overdue });
			onchanged?.(); // dependants may have shifted
		} catch (err) {
			Object.assign(t, prev);
			toasts.error(err);
		}
	}

	/** Pixel offset applied to a bar while it is being dragged. */
	function live(t: TaskSummary): { dx: number; dw: number } {
		if (!drag || drag.id !== t.id) return { dx: 0, dw: 0 };
		const px = snapDays(drag.dx) * dayWidth;
		return drag.mode === 'move' ? { dx: px, dw: 0 } : { dx: 0, dw: px };
	}

	const barColor = (t: TaskSummary) =>
		t.status === 'done'
			? 'bg-emerald-400'
			: t.status === 'review'
				? 'bg-amber-400'
				: t.is_overdue
					? 'bg-red-400'
					: t.status === 'in_progress'
						? 'bg-sky-500'
						: 'bg-slate-400';

	// Dependency arrows: from the end of the predecessor to the start of the dependant.
	const arrows = $derived(
		dependencies
			.filter((d) => rowOf.has(d.task_id) && rowOf.has(d.depends_on_id))
			.map((d) => {
				const pred = tasks[rowOf.get(d.depends_on_id)!]!;
				const succ = tasks[rowOf.get(d.task_id)!]!;
				const x1 = x(endOf(pred)) + live(pred).dx + live(pred).dw;
				const y1 = rowOf.get(pred.id)! * ROW + ROW / 2;
				const x2 = x(startOf(succ)) + live(succ).dx;
				const y2 = rowOf.get(succ.id)! * ROW + ROW / 2;
				const conflict = x2 < x1 - 1 && succ.status !== 'done';
				const mid = x1 + 8;
				const path = `M ${x1} ${y1} H ${mid} V ${y2} H ${x2 - 2}`;
				return { key: `${d.depends_on_id}-${d.task_id}`, path, conflict };
			})
	);

	let scroller: HTMLDivElement | undefined = $state();
	function scrollToToday() {
		scroller?.scrollTo({ left: Math.max(0, x(Date.now()) - 120), behavior: 'smooth' });
	}
	$effect(() => {
		// Start with today in view.
		if (scroller && tasks.length >= 0) queueMicrotask(scrollToToday);
	});
</script>

<div class="card overflow-hidden">
	<div class="flex items-center gap-2 border-b border-slate-100 px-4 py-2 text-sm">
		<span class="text-slate-500">Zoom</span>
		<div class="flex rounded-lg bg-slate-100 p-0.5">
			{#each ['day', 'week', 'month'] as z (z)}
				<button class="rounded-md px-2.5 py-1 capitalize {zoom === z ? 'bg-white font-medium text-slate-900 shadow-sm' : 'text-slate-500'}" onclick={() => ((zoom = z as Zoom), queueMicrotask(scrollToToday))}>{z}</button>
			{/each}
		</div>
		<button class="btn btn-ghost px-2 py-1 text-sm" onclick={scrollToToday}>Today</button>
		<span class="ml-auto hidden text-xs text-slate-400 sm:inline">Drag a bar to move it · drag its right edge to change the deadline</span>
	</div>

	{#if !tasks.length}
		<p class="px-5 py-14 text-center text-slate-500">No tasks to plan yet.</p>
	{:else}
		<div class="flex">
			<!-- Task names -->
			<div class="w-64 shrink-0 border-r border-slate-200">
				<div class="flex items-end border-b border-slate-200 px-3 pb-2 text-sm font-medium text-slate-600" style="height: {HEADER}px">Task</div>
				{#each tasks as t (t.id)}
					<a href="/tasks/{t.id}" class="flex items-center gap-2 border-b border-slate-50 px-3 text-sm hover:bg-slate-50" style="height: {ROW}px">
						<span class="size-2 shrink-0 rounded-full {STATUS[t.status].dot}"></span>
						<span class="truncate {t.status === 'done' ? 'text-slate-400 line-through' : 'text-slate-800'}" title={t.title}>{t.title}</span>
						<span class="ml-auto shrink-0 text-xs text-slate-300">#{t.number}</span>
					</a>
				{/each}
			</div>

			<!-- Timeline -->
			<div class="min-w-0 flex-1 overflow-x-auto" bind:this={scroller}>
				<div class="relative" style="width: {width}px">
					<!-- header -->
					<div class="sticky top-0 border-b border-slate-200 bg-white" style="height: {HEADER}px">
						{#each months as m (m.label)}
							<!-- The label sticks to the left edge while its month is in view. -->
							<div class="absolute top-0 border-l border-slate-200 pt-1 text-xs font-medium text-slate-600" style="left: {m.left}px; width: {m.width}px">
								<span class="sticky left-0 block w-max px-2">{m.width > 70 ? m.label : ''}</span>
							</div>
						{/each}
						{#each days as d, i (i)}
							{#if zoom === 'day' || (zoom === 'week' && d.getDay() === 1)}
								<div class="absolute bottom-1 text-center text-[11px] {d.getDay() === 0 || d.getDay() === 6 ? 'text-slate-300' : 'text-slate-500'}" style="left: {i * dayWidth}px; width: {zoom === 'day' ? dayWidth : dayWidth * 7}px">
									{zoom === 'day' ? d.getDate() : d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}
								</div>
							{/if}
						{/each}
					</div>

					<!-- grid + bars -->
					<div class="relative" style="height: {height}px" role="presentation">
						{#if zoom !== 'month'}
							{#each days as d, i (i)}
								{#if d.getDay() === 0 || d.getDay() === 6}
									<div class="absolute top-0 bottom-0 bg-slate-50" style="left: {i * dayWidth}px; width: {dayWidth}px"></div>
								{/if}
							{/each}
						{/if}
						{#each tasks as _, i (i)}
							<div class="absolute right-0 left-0 border-b border-slate-50" style="top: {(i + 1) * ROW - 1}px"></div>
						{/each}
						<div class="absolute top-0 bottom-0 w-px bg-red-400" style="left: {x(Date.now())}px" title="Today"></div>

						<svg class="pointer-events-none absolute inset-0" width={width} height={height} aria-hidden="true">
							<defs>
								<marker id="gantt-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
									<path d="M0,0 L8,4 L0,8 z" fill="#94a3b8" />
								</marker>
								<marker id="gantt-arrow-bad" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
									<path d="M0,0 L8,4 L0,8 z" fill="#ef4444" />
								</marker>
							</defs>
							{#each arrows as a (a.key)}
								<path d={a.path} fill="none" stroke={a.conflict ? '#ef4444' : '#94a3b8'} stroke-width="1.5" stroke-dasharray={a.conflict ? '4 3' : ''} marker-end="url(#{a.conflict ? 'gantt-arrow-bad' : 'gantt-arrow'})" />
							{/each}
						</svg>

						{#each tasks as t, i (t.id)}
							{@const l = live(t)}
							{@const left = x(startOf(t)) + l.dx}
							{@const w = Math.max(dayWidth / 2, x(endOf(t)) - x(startOf(t)) + l.dw)}
							<div
								role="button"
								tabindex="0"
								data-bar={t.id}
								class="group absolute flex cursor-grab items-center overflow-hidden rounded-md text-xs text-white shadow-sm select-none active:cursor-grabbing {barColor(t)} {hasDates(t) ? '' : 'opacity-40'} {drag?.id === t.id ? 'ring-2 ring-brand-500' : ''}"
								style="left: {left}px; width: {w}px; top: {i * ROW + 7}px; height: {ROW - 14}px"
								title="{t.title}{hasDates(t) ? '' : ' (no dates yet: drag to schedule)'}"
								onpointerdown={(e) => begin(e, t, 'move')}
								onkeydown={(e) => e.key === 'Enter' && goto(`/tasks/${t.id}`)}
							>
								{#if t.checklist_total}
									<div class="absolute inset-y-0 left-0 bg-black/15" style="width: {(t.checklist_done / t.checklist_total) * 100}%"></div>
								{/if}
								<span class="relative truncate px-2">{w > 60 ? t.title : ''}</span>
								<span
									role="presentation"
									class="absolute top-0 right-0 bottom-0 w-2 cursor-ew-resize bg-black/0 group-hover:bg-black/20"
									onpointerdown={(e) => begin(e, t, 'end')}
								></span>
							</div>
						{/each}
					</div>
				</div>
			</div>
		</div>
	{/if}
</div>
