<script lang="ts" module>
	export interface BoardColumn {
		key: string;
		label: string;
		/** Tailwind class for the column's colour dot. */
		dot: string;
	}
</script>

<script lang="ts">
	import { api } from '#lib/api.ts';
	import { BOARD_COLUMNS, EPIC_COLORS, fmtPoints, formatDeadline, PRIORITY, PROJECT_COLORS, STATUS, type TaskDetail, type TaskSummary } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Avatar from './Avatar.svelte';

	const statusColumns: BoardColumn[] = BOARD_COLUMNS.map((s) => ({ key: s, label: STATUS[s].label, dot: STATUS[s].dot }));

	let {
		tasks = $bindable<TaskSummary[]>([]),
		columns = statusColumns,
		columnOf = (t: TaskSummary) => t.status,
		move = (t: TaskSummary, column: string, after: string | null, before: string | null) =>
			api.post<TaskDetail>(`/tasks/${t.id}/move`, { status: column, after_id: after, before_id: before }),
		showProject = false,
		onchanged
	}: {
		tasks?: TaskSummary[];
		columns?: BoardColumn[];
		columnOf?: (t: TaskSummary) => string | null;
		move?: (t: TaskSummary, column: string, after: string | null, before: string | null) => Promise<TaskDetail>;
		showProject?: boolean;
		onchanged?: () => void;
	} = $props();

	let dragging = $state<string | null>(null);
	let over = $state<{ column: string; index: number } | null>(null);

	const grouped = $derived(
		columns.map((c, i) => {
			// Cards with no column (e.g. columns were deleted) show in the first one.
			const items = tasks
				.filter((t) => (columnOf(t) ?? columns[0]?.key) === c.key || (i === 0 && !columns.some((x) => x.key === columnOf(t))))
				.sort((a, b) => a.position - b.position || a.created_at.localeCompare(b.created_at));
			// The same cards minus the one being dragged, for placing the drop marker.
			return { ...c, items, visible: items.filter((x) => x.id !== dragging) };
		})
	);

	function onDragOver(e: DragEvent, column: string, list: HTMLElement) {
		e.preventDefault();
		const cards = [...list.querySelectorAll<HTMLElement>('[data-card]')].filter((c) => c.dataset.card !== dragging);
		let index = cards.length;
		for (let i = 0; i < cards.length; i++) {
			const r = cards[i]!.getBoundingClientRect();
			if (e.clientY < r.top + r.height / 2) {
				index = i;
				break;
			}
		}
		over = { column, index };
	}

	async function onDrop(e: DragEvent) {
		e.preventDefault();
		const id = dragging;
		const target = over;
		dragging = null;
		over = null;
		if (!id || !target) return;
		const col = grouped.find((c) => c.key === target.column)!.items.filter((t) => t.id !== id);
		const afterTask = col[target.index - 1];
		const beforeTask = col[target.index];
		const task = tasks.find((t) => t.id === id)!;
		const prev = { ...task };
		// Optimistic: place the card now, then take the server's answer.
		const a = afterTask?.position;
		const b = beforeTask?.position;
		task.position = a !== undefined && b !== undefined ? (a + b) / 2 : a !== undefined ? a + 1 : b !== undefined ? b - 1 : 0;
		if (columns === statusColumns) task.status = target.column as TaskSummary['status'];
		else task.stage_id = target.column;
		try {
			const saved = await move(task, target.column, afterTask?.id ?? null, beforeTask?.id ?? null);
			Object.assign(task, {
				status: saved.status,
				stage_id: saved.stage_id,
				position: saved.position,
				completed_at: saved.completed_at,
				is_overdue: saved.is_overdue
			});
			if (saved.status === 'review' && prev.status !== 'review') toasts.info('Sent for review: the creator will check it');
			onchanged?.();
		} catch (err) {
			Object.assign(task, prev);
			toasts.error(err);
		}
	}
</script>

<div class="flex gap-4 overflow-x-auto pb-4">
	{#each grouped as col (col.key)}
		<section class="flex min-w-52 flex-1 flex-col rounded-2xl bg-surface-container">
			<header class="flex items-center gap-2 px-3 py-2.5">
				<span class="size-2 rounded-full {col.dot}"></span>
				<h3 class="truncate text-sm font-semibold text-slate-700">{col.label}</h3>
				<span class="ml-auto text-xs text-slate-400">{col.items.length}</span>
			</header>
			<div
				role="list"
				data-column={col.key}
				class="min-h-24 flex-1 space-y-2 px-2 pb-2 transition {over?.column === col.key ? 'rounded-b-2xl bg-brand-50' : ''}"
				ondragover={(e) => onDragOver(e, col.key, e.currentTarget)}
				ondragleave={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && (over = null)}
				ondrop={onDrop}
			>
				{#each col.items as t (t.id)}
					{#if over?.column === col.key && t.id !== dragging && col.visible.indexOf(t) === over.index}
						<div class="h-1 rounded bg-brand-400"></div>
					{/if}
					<a
						href="/tasks/{t.id}"
						data-card={t.id}
						draggable="true"
						ondragstart={(e) => {
							dragging = t.id;
							e.dataTransfer?.setData('text/plain', t.id);
						}}
						ondragend={() => ((dragging = null), (over = null))}
						class="block rounded-xl bg-white p-3 shadow-g1 transition-shadow hover:shadow-g2 {dragging === t.id ? 'opacity-40' : ''}"
					>
						<p class="text-sm font-medium {t.status === 'done' ? 'text-slate-400 line-through' : 'text-slate-800'}">
							{#if t.priority === 'high'}<span class={PRIORITY.high.class}>▲ </span>{/if}{t.title}
						</p>
						<div class="mt-1 flex flex-wrap gap-1.5">
							{#if columns !== statusColumns && t.status !== 'new'}
								<span class="badge {STATUS[t.status].badge}">{STATUS[t.status].label}</span>
							{/if}
							{#if t.blocked_by_count}<span class="badge bg-orange-50 text-orange-700">Blocked</span>{/if}
							{#if t.epic_name}<span class="badge {EPIC_COLORS[t.epic_color ?? 'violet']}">{t.epic_name}</span>{/if}
							{#if t.story_points != null}<span class="badge bg-slate-100 text-slate-600" title="Story points">{fmtPoints(t.story_points)} pts</span>{/if}
							{#if t.template_id}<span class="badge bg-slate-100 text-slate-500" title="Created from a template">↻</span>{/if}
						</div>
						{#if showProject && t.project_name}
							<p class="mt-1 flex items-center gap-1.5 text-xs text-slate-500">
								<span class="size-1.5 rounded-full {PROJECT_COLORS[t.project_color ?? 'slate']?.bg}"></span>{t.project_name}
							</p>
						{/if}
						<div class="mt-2.5 flex items-center gap-2 text-xs text-slate-400">
							<Avatar name={t.responsible_name} fileId={t.responsible_avatar_file_id} size="xs" />
							{#if t.deadline}
								<span class={t.is_overdue ? 'rounded bg-red-50 px-1.5 py-0.5 font-medium text-red-600' : ''}>{formatDeadline(t.deadline)}</span>
							{/if}
							<span class="ml-auto flex gap-2">
								{#if t.checklist_total}<span>☑ {t.checklist_done}/{t.checklist_total}</span>{/if}
								{#if t.comment_count}<span>💬 {t.comment_count}</span>{/if}
							</span>
						</div>
					</a>
				{/each}
				{#if over?.column === col.key && over.index >= col.visible.length}
					<div class="h-1 rounded bg-brand-400"></div>
				{/if}
			</div>
		</section>
	{/each}
</div>
