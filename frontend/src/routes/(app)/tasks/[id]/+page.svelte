<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, downloadFile, request } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import ChatCallButtons from '#lib/components/ChatCallButtons.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import MentionTextarea from '#lib/components/MentionTextarea.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import TaskFieldsCard from '#lib/components/TaskFieldsCard.svelte';
	import TaskForm from '#lib/components/TaskForm.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import TaskPicker from '#lib/components/TaskPicker.svelte';
	import CrmPicker from '#lib/components/crm/CrmPicker.svelte';
	import TaskTable from '#lib/components/TaskTable.svelte';
	import { formatBytes, formatDateTime, formatRelative } from '#lib/format.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import {
		formatClock,
		formatDeadline,
		formatDuration,
		parseMentions,
		PRIORITY,
		PROJECT_COLORS,
		STATUS,
		type HistoryEntry,
		type TaskComment,
		type TaskDetail,
		type TaskStatus,
		type TimeEntry
	} from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { FileMeta } from '#lib/types.ts';

	const id = $derived(page.params.id!);
	let task = $state<TaskDetail | null>(null);
	let notFound = $state(false);
	let comments = $state<TaskComment[]>([]);
	let tab = $state<'comments' | 'time' | 'history'>('comments');
	let timeEntries = $state<TimeEntry[]>([]);
	let history = $state<HistoryEntry[]>([]);
	let busy = $state(false);
	let editOpen = $state(false);
	let subtaskOpen = $state(false);
	let newItem = $state('');
	let newComment = $state('');
	let newCommentEncoded = $state('');
	let posting = $state(false);
	let fileInput: HTMLInputElement | undefined = $state();
	let now = $state(Date.now());

	async function load() {
		try {
			task = await api.get<TaskDetail>(`/tasks/${id}`);
			notFound = false;
		} catch {
			notFound = true;
		}
	}
	async function loadComments() {
		comments = await api.get<TaskComment[]>(`/tasks/${id}/comments`);
	}
	async function loadTab() {
		if (tab === 'time') timeEntries = await api.get<TimeEntry[]>(`/tasks/${id}/time`);
		if (tab === 'history') history = await api.get<HistoryEntry[]>(`/tasks/${id}/history`);
	}

	$effect(() => {
		id;
		task = null;
		tab = 'comments';
		load();
		loadComments();
	});
	$effect(() => {
		tab;
		if (task) loadTab();
	});

	onMount(() => {
		const tick = setInterval(() => (now = Date.now()), 1000);
		let pending: ReturnType<typeof setTimeout>;
		const offs = [
			realtime.on('task.changed', (d: { id: string; deleted: boolean }) => {
				if (d.id !== id) {
					// A subtask changed: refresh the list.
					if (task?.subtasks.some((s) => s.id === d.id)) load();
					return;
				}
				if (d.deleted) {
					toasts.info('This task was deleted');
					goto('/tasks');
					return;
				}
				clearTimeout(pending);
				pending = setTimeout(() => {
					load();
					loadTab();
				}, 250);
			}),
			realtime.on('task.comment', (c: TaskComment) => {
				if (c.task_id !== id) return;
				const i = comments.findIndex((x) => x.id === c.id);
				if (i >= 0) comments[i] = c;
				else comments = [...comments, c];
			}),
			realtime.on('task.comment_deleted', (d: { id: string }) => (comments = comments.filter((c) => c.id !== d.id)))
		];
		return () => {
			clearInterval(tick);
			offs.forEach((off) => off());
		};
	});

	// ----- status -----
	async function setStatus(status: TaskStatus) {
		busy = true;
		try {
			task = await api.post<TaskDetail>(`/tasks/${id}/status`, { status });
			if (task.status === 'review' && status === 'done') toasts.info('Sent to the creator for review');
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}

	const actions = $derived.by(() => {
		if (!task) return [];
		const a = task.access;
		const s = task.status;
		const out: { label: string; status: TaskStatus; primary?: boolean }[] = [];
		if (a.participate && (s === 'new' || s === 'deferred')) out.push({ label: 'Start', status: 'in_progress', primary: s === 'new' });
		if (a.participate && (s === 'new' || s === 'in_progress'))
			out.push({ label: task.requires_review && !a.is_creator ? 'Finish for review' : 'Complete', status: 'done', primary: s === 'in_progress' });
		if (a.edit && s === 'review') {
			out.push({ label: 'Accept', status: 'done', primary: true });
			out.push({ label: 'Return for rework', status: 'in_progress' });
		}
		if (a.participate && s === 'in_progress') out.push({ label: 'Pause', status: 'new' });
		if (a.participate && (s === 'new' || s === 'in_progress')) out.push({ label: 'Defer', status: 'deferred' });
		if (a.participate && s === 'done') out.push({ label: 'Reopen', status: 'new' });
		return out;
	});

	// ----- result and rating -----
	async function markResult(commentId: string | null) {
		try {
			task = await api.put<TaskDetail>(`/tasks/${id}/result`, { comment_id: commentId });
			if (commentId) toasts.success('Marked as the result');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function rate(rating: 'good' | 'bad' | null) {
		try {
			task = await api.post<TaskDetail>(`/tasks/${id}/rate`, { rating });
		} catch (e) {
			toasts.error(e);
		}
	}
	// ----- who does a checklist point -----
	let assigning = $state<{ id: string; title: string } | null>(null);
	async function assignItem(userId: string | null) {
		if (!assigning) return;
		try {
			await api.patch(`/tasks/${id}/checklist/${assigning.id}`, { assignee_id: userId });
			assigning = null;
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- checklist -----
	async function toggleItem(itemId: string, done: boolean) {
		const item = task!.checklist.find((c) => c.id === itemId)!;
		item.is_done = done;
		task!.checklist_done += done ? 1 : -1;
		try {
			await api.patch(`/tasks/${id}/checklist/${itemId}`, { is_done: done });
		} catch (e) {
			toasts.error(e);
			load();
		}
	}
	async function addItem(e: SubmitEvent) {
		e.preventDefault();
		if (!newItem.trim()) return;
		try {
			await api.post(`/tasks/${id}/checklist`, { title: newItem });
			newItem = '';
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function deleteItem(itemId: string) {
		await api.del(`/tasks/${id}/checklist/${itemId}`);
		await load();
	}

	// ----- comments -----
	async function postComment() {
		if (!newComment.trim()) return;
		posting = true;
		try {
			const c = await api.post<TaskComment>(`/tasks/${id}/comments`, { body: newCommentEncoded || newComment });
			if (!comments.some((x) => x.id === c.id)) comments = [...comments, c];
			newComment = '';
		} catch (e) {
			toasts.error(e);
		} finally {
			posting = false;
		}
	}
	async function deleteComment(c: TaskComment) {
		if (!confirm('Delete this comment?')) return;
		await api.del(`/tasks/${id}/comments/${c.id}`);
		comments = comments.filter((x) => x.id !== c.id);
	}

	// ----- files -----
	async function upload(files: FileList | null) {
		for (const f of Array.from(files ?? [])) {
			const fd = new FormData();
			fd.append('file', f);
			try {
				const meta = await api.post<FileMeta>('/files', fd);
				await api.post(`/tasks/${id}/files`, { file_id: meta.id });
			} catch (e) {
				toasts.error(e);
			}
		}
		if (fileInput) fileInput.value = '';
		await load();
	}
	async function detach(fileId: string) {
		await api.del(`/tasks/${id}/files/${fileId}`);
		await load();
	}

	// ----- time -----
	const running = $derived(task?.my_timer_started_at ? Math.max(0, (now - new Date(task.my_timer_started_at).getTime()) / 1000) : 0);
	async function toggleTimer() {
		try {
			await api.post(`/tasks/${id}/timer/${task!.my_timer_started_at ? 'stop' : 'start'}`);
			await load();
			if (tab === 'time') loadTab();
		} catch (e) {
			toasts.error(e);
		}
	}
	let manualMinutes = $state('');
	let manualComment = $state('');
	async function addManual(e: SubmitEvent) {
		e.preventDefault();
		const hm = manualMinutes.trim();
		const m = /^(\d+):(\d{1,2})$/.exec(hm);
		const minutes = m ? +m[1]! * 60 + +m[2]! : Math.round(parseFloat(hm) * 60);
		if (!minutes || minutes < 1) {
			toasts.error(new Error('Enter hours like 1.5 or 1:30'));
			return;
		}
		try {
			await api.post(`/tasks/${id}/time`, { minutes, comment: manualComment || null });
			manualMinutes = '';
			manualComment = '';
			await Promise.all([load(), loadTab()]);
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- dependencies & templates -----
	async function addBlocker(other: { id: string }) {
		try {
			await api.post(`/tasks/${id}/dependencies`, { depends_on_id: other.id });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function setCrm(ids: string[]) {
		try {
			task = await api.put<TaskDetail>(`/tasks/${id}/crm`, { item_ids: ids });
		} catch (e) {
			toasts.error(e);
		}
	}

	async function removeBlocker(other: string) {
		await api.del(`/tasks/${id}/dependencies/${other}`);
		await load();
	}
	async function saveAsTemplate() {
		try {
			const t = await api.post<{ id: string }>(`/tasks/${id}/save-as-template`, {});
			toasts.success('Saved as a template. You can now add a schedule.');
			goto(`/tasks/templates/${t.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- misc -----
	async function remove() {
		if (!confirm(`Delete task #${task!.number}? This cannot be undone.`)) return;
		await api.del(`/tasks/${id}`);
		toasts.success('Task deleted');
		goto('/tasks');
	}
	const taskChat = async () => (await api.post<{ chat_id: string }>(`/tasks/${id}/chat`)).chat_id;

	async function toggleWatch() {
		await request(task!.access.is_watcher ? 'DELETE' : 'POST', `/tasks/${id}/watch`);
		await load();
	}

	const people = (role: string) => task?.people.filter((p) => p.role === role) ?? [];
	const isParticipant = $derived(!!task && (task.access.is_creator || task.access.is_responsible || task.access.is_assistant));

	const FIELD_LABELS: Record<string, string> = {
		created: 'created the task',
		title: 'renamed the task',
		description: 'edited the description',
		responsible: 'changed who is responsible',
		deadline: 'changed the deadline',
		priority: 'changed the priority',
		project: 'moved the task to another project',
		parent: 'changed the parent task',
		requires_review: 'changed the review setting',
		time_estimate: 'changed the estimate',
		status: 'changed the status',
		checklist_done: 'ticked',
		checklist_undone: 'unticked',
		file_added: 'attached',
		file_removed: 'removed the attachment',
		start_date: 'changed the start date',
		stage: 'moved the card to',
		rescheduled: 'shifted the dates because an earlier task moved:',
		blocked_by_added: 'made this wait for',
		blocked_by_removed: 'stopped this waiting for'
	};
	function historyValue(h: HistoryEntry, v: string | null): string {
		if (v === null) return '—';
		if (h.field === 'status') return STATUS[v as TaskStatus]?.label ?? v;
		if (h.field === 'deadline' || h.field === 'start_date' || h.field === 'rescheduled') return formatDateTime(v);
		if (h.field === 'time_estimate') return formatDuration(+v);
		return v;
	}
</script>

<svelte:head><title>{task ? `#${task.number} ${task.title}` : 'Task'} · NERP</title></svelte:head>

{#if notFound}
	<div class="card p-10 text-center text-slate-500">This task doesn't exist or you don't have access to it.</div>
{:else if task}
	<div class="mb-4 flex flex-wrap items-center gap-2 text-sm text-slate-500">
		<a href="/tasks" class="hover:text-slate-800">Tasks</a>
		{#if task.project_id}
			<Icon name="chevronRight" class="size-4" />
			<a href="/projects/{task.project_id}" class="flex items-center gap-1.5 hover:text-slate-800">
				<span class="size-2 rounded-full {PROJECT_COLORS[task.project_color ?? 'slate']?.bg}"></span>{task.project_name}
			</a>
		{/if}
		{#if task.parent}
			<Icon name="chevronRight" class="size-4" />
			<a href="/tasks/{task.parent.id}" class="hover:text-slate-800">#{task.parent.number} {task.parent.title}</a>
		{/if}
	</div>

	<div class="grid gap-6 lg:grid-cols-[1fr_320px]">
		<div class="min-w-0 space-y-6">
			<section class="card p-6">
				<div class="flex flex-wrap items-start justify-between gap-3">
					<div class="min-w-0">
						<p class="text-sm text-slate-400">#{task.number}</p>
						<h1 class="text-2xl font-semibold break-words text-slate-900">{task.title}</h1>
					</div>
					<div class="flex gap-1">
						{#if isParticipant || task.access.is_watcher}
							<ChatCallButtons chat={taskChat} call={new Set(task.people.map((p) => p.user_id)).size > 1} />
						{/if}
						{#if !isParticipant}
							<button class="btn btn-ghost p-2" title={task.access.is_watcher ? 'Stop watching' : 'Watch'} onclick={toggleWatch}>
								<Icon name="eye" class="size-5 {task.access.is_watcher ? 'text-brand-600' : ''}" />
							</button>
						{/if}
						<button class="btn btn-ghost p-2" title="Save as template / make recurring" aria-label="Save as template" onclick={saveAsTemplate}><Icon name="refresh" class="size-5" /></button>
						{#if task.access.edit}
							<button class="btn btn-secondary" onclick={() => (editOpen = true)}><Icon name="edit" class="size-4" /> Edit</button>
						{/if}
						{#if task.access.delete}
							<button class="btn btn-ghost p-2 text-slate-400 hover:text-red-600" aria-label="Delete task" onclick={remove}><Icon name="trash" class="size-4" /></button>
						{/if}
					</div>
				</div>

				<div class="mt-4 flex flex-wrap items-center gap-2">
					<span class="badge py-1 {STATUS[task.status].badge}">{STATUS[task.status].label}</span>
					{#if task.is_overdue}<span class="badge bg-red-50 py-1 text-red-700">Overdue</span>{/if}
					<span class="flex-1"></span>
					{#each actions as a (a.label)}
						<button class="btn {a.primary ? 'btn-primary' : 'btn-secondary'}" disabled={busy} onclick={() => setStatus(a.status)}>{a.label}</button>
					{/each}
				</div>
				{#if task.blocked_by_count && task.status !== 'done'}
					<p class="mt-3 rounded-lg bg-orange-50 px-3 py-2 text-sm text-orange-800">
						Waiting for {task.blocked_by_count} unfinished {task.blocked_by_count === 1 ? 'task' : 'tasks'} — see <em>Depends on</em> below.
					</p>
				{/if}
				{#if task.status === 'review' && !task.access.edit}
					<p class="mt-3 rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-800">Waiting for {task.creator_name} to accept the work.</p>
				{/if}

				{#if task.description}
					<div class="mt-5 border-t border-slate-100 pt-5 text-sm leading-relaxed whitespace-pre-line text-slate-700">{task.description}</div>
				{/if}
				{#if task.result}
					<div class="mt-5 rounded-lg border border-emerald-200 bg-emerald-50 px-4 py-3" data-result>
						<p class="text-xs font-semibold tracking-wide text-emerald-700 uppercase">Result</p>
						<p class="mt-1 text-sm whitespace-pre-line text-slate-800">{task.result.body}</p>
						<p class="mt-1 text-xs text-slate-500">{task.result.author_name ?? ''} · {formatDateTime(task.result.created_at)}</p>
					</div>
				{:else if task.requires_result && task.status !== 'done'}
					<p class="mt-4 rounded-lg bg-slate-50 px-3 py-2 text-sm text-slate-600">This task needs a result: write a comment and mark it as the result before finishing.</p>
				{/if}
				{#if task.status === 'done' && (task.access.is_creator || task.rating)}
					<div class="mt-4 flex items-center gap-2 text-sm" data-rating={task.rating ?? ''}>
						<span class="text-slate-500">{task.access.is_creator ? 'How was it done?' : 'Rated:'}</span>
						{#if task.access.is_creator}
							<button class="btn px-3 py-1 {task.rating === 'good' ? 'btn-primary' : 'btn-secondary'}" aria-pressed={task.rating === 'good'} onclick={() => rate(task!.rating === 'good' ? null : 'good')}>👍 Well</button>
							<button class="btn px-3 py-1 {task.rating === 'bad' ? 'btn-danger' : 'btn-secondary'}" aria-pressed={task.rating === 'bad'} onclick={() => rate(task!.rating === 'bad' ? null : 'bad')}>👎 Not well</button>
						{:else}
							<span>{task.rating === 'good' ? '👍 Well' : '👎 Not well'}</span>
						{/if}
					</div>
				{/if}
			</section>

			<!-- Checklist -->
			<section class="card">
				<div class="flex items-center gap-3 border-b border-slate-100 px-5 py-3">
					<h2 class="font-semibold text-slate-900">Checklist</h2>
					{#if task.checklist_total}
						<div class="h-1.5 flex-1 overflow-hidden rounded-full bg-slate-100">
							<div class="h-full rounded-full bg-emerald-500 transition-all" style="width: {(task.checklist_done / task.checklist_total) * 100}%"></div>
						</div>
						<span class="text-xs text-slate-500">{task.checklist_done}/{task.checklist_total}</span>
					{/if}
				</div>
				<ul class="divide-y divide-slate-50">
					{#each task.checklist as item (item.id)}
						<li class="group flex items-center gap-3 px-5 py-2">
							<input
								type="checkbox"
								class="rounded border-slate-300 text-emerald-600 focus:ring-emerald-500"
								checked={item.is_done}
								disabled={!task.access.participate}
								onchange={(e) => toggleItem(item.id, e.currentTarget.checked)}
							/>
							<span class="flex-1 text-sm {item.is_done ? 'text-slate-400 line-through' : 'text-slate-700'}">{item.title}</span>
							{#if item.assignee_name}<span class="badge bg-slate-100 text-slate-600" data-assignee>{item.assignee_name}</span>{/if}
							{#if task.access.participate}
								<button class="invisible text-xs text-slate-400 group-hover:visible hover:text-brand-600" aria-label="Assign {item.title}" onclick={() => (assigning = { id: item.id, title: item.title })}><Icon name="user" class="size-4" /></button>
								<button class="invisible text-slate-300 group-hover:visible hover:text-red-500" aria-label="Remove item" onclick={() => deleteItem(item.id)}><Icon name="x" class="size-4" /></button>
							{/if}
						</li>
					{/each}
				</ul>
				{#if task.access.participate}
					<form class="flex gap-2 px-5 py-3" onsubmit={addItem}>
						<input class="input" placeholder="Add an item…" bind:value={newItem} />
						<button class="btn btn-secondary" disabled={!newItem.trim()}>Add</button>
					</form>
				{:else if !task.checklist.length}
					<p class="px-5 py-4 text-sm text-slate-400">No checklist.</p>
				{/if}
			</section>

			<!-- CRM records -->
			{#if task.crm.length || task.access.edit}
				<section class="card" aria-labelledby="crm-h">
					<h2 id="crm-h" class="border-b border-slate-100 px-5 py-3 font-semibold text-slate-900">CRM</h2>
					<div class="space-y-2 p-5 text-sm">
						{#each task.crm as r (r.id)}
							<div class="group flex items-center gap-2">
								<span class="w-20 shrink-0 text-xs text-slate-500">{r.type_name}</span>
								<a class="min-w-0 flex-1 truncate text-brand-700 hover:underline" href="/crm/{r.code}/{r.id}">#{r.number} {r.title}</a>
								{#if task.access.edit}<button class="invisible text-slate-300 group-hover:visible hover:text-red-500" aria-label="Unlink {r.title}" onclick={() => setCrm(task!.crm.filter((x) => x.id !== r.id).map((x) => x.id))}><Icon name="x" class="size-4" /></button>{/if}
							</div>
						{:else}
							<p class="text-slate-400">Not linked to any CRM record.</p>
						{/each}
						{#if task.access.edit}
							<CrmPicker types={['deal', 'lead', 'contact', 'company', 'quote', 'invoice']} placeholder="Link a deal, lead, contact…" exclude={task.crm.map((r) => r.id)} onpick={(h) => setCrm([...task!.crm.map((r) => r.id), h.id])} />
						{/if}
					</div>
				</section>
			{/if}

			<!-- Dependencies -->
			{#if task.blocked_by.length || task.blocking.length || task.access.edit}
				<section class="card">
					<h2 class="border-b border-slate-100 px-5 py-3 font-semibold text-slate-900">Dependencies</h2>
					<div class="grid gap-5 p-5 sm:grid-cols-2">
						<div>
							<p class="mb-2 text-sm font-medium text-slate-600">Depends on</p>
							<ul class="space-y-1.5">
								{#each task.blocked_by as d (d.id)}
									<li class="group flex items-center gap-2 text-sm">
										<span class="size-2 shrink-0 rounded-full {STATUS[d.status].dot}"></span>
										<a href="/tasks/{d.id}" class="truncate hover:underline {d.status === 'done' ? 'text-slate-400 line-through' : 'text-slate-800'}">#{d.number} {d.title}</a>
										{#if task.access.edit}<button class="invisible ml-auto text-slate-300 group-hover:visible hover:text-red-500" aria-label="Remove dependency" onclick={() => removeBlocker(d.id)}><Icon name="x" class="size-4" /></button>{/if}
									</li>
								{:else}
									<li class="text-sm text-slate-400">Nothing — this task can start any time.</li>
								{/each}
							</ul>
							{#if task.access.edit}
								<div class="mt-3"><TaskPicker placeholder="This task waits for…" exclude={[task.id, ...task.blocked_by.map((d) => d.id)]} onpick={addBlocker} /></div>
							{/if}
						</div>
						<div>
							<p class="mb-2 text-sm font-medium text-slate-600">Blocks</p>
							<ul class="space-y-1.5">
								{#each task.blocking as d (d.id)}
									<li class="flex items-center gap-2 text-sm">
										<span class="size-2 shrink-0 rounded-full {STATUS[d.status].dot}"></span>
										<a href="/tasks/{d.id}" class="truncate text-slate-800 hover:underline">#{d.number} {d.title}</a>
									</li>
								{:else}
									<li class="text-sm text-slate-400">No tasks are waiting for this one.</li>
								{/each}
							</ul>
						</div>
					</div>
				</section>
			{/if}

			<!-- Subtasks -->
			{#if task.subtasks.length || task.access.participate}
				<section class="space-y-2">
					<div class="flex items-center justify-between">
						<h2 class="font-semibold text-slate-900">Subtasks</h2>
						{#if task.access.participate}<button class="btn btn-ghost text-sm" onclick={() => (subtaskOpen = true)}><Icon name="plus" class="size-4" /> Add subtask</button>{/if}
					</div>
					{#if task.subtasks.length}<TaskTable tasks={task.subtasks} showProject={false} />{/if}
				</section>
			{/if}

			<!-- Attachments -->
			<section class="card">
				<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
					<h2 class="font-semibold text-slate-900">Files</h2>
					{#if task.access.participate}
						<button class="btn btn-ghost text-sm" onclick={() => fileInput?.click()}><Icon name="paperclip" class="size-4" /> Attach</button>
						<input bind:this={fileInput} type="file" multiple class="hidden" onchange={(e) => upload(e.currentTarget.files)} />
					{/if}
				</div>
				{#if task.files.length}
					<ul class="divide-y divide-slate-50">
						{#each task.files as f (f.id)}
							<li class="group flex items-center gap-3 px-5 py-2.5 text-sm">
								<Icon name="file" class="size-4 text-slate-400" />
								<button class="flex-1 truncate text-left font-medium text-slate-700 hover:text-brand-600" onclick={() => downloadFile(f.id, f.original_name).catch((e) => toasts.error(e))}>{f.original_name}</button>
								<span class="text-xs text-slate-400">{formatBytes(f.size_bytes)}</span>
								{#if task.access.edit || f.added_by === session.user?.id}
									<button class="invisible text-slate-300 group-hover:visible hover:text-red-500" aria-label="Remove file" onclick={() => detach(f.id)}><Icon name="x" class="size-4" /></button>
								{/if}
							</li>
						{/each}
					</ul>
				{:else}
					<p class="px-5 py-4 text-sm text-slate-400">No files attached.</p>
				{/if}
			</section>

			<!-- Activity tabs -->
			<section class="card">
				<nav class="flex gap-1 border-b border-slate-100 px-3">
					{#each [['comments', `Comments (${comments.length})`], ['time', 'Time'], ['history', 'History']] as [key, label] (key)}
						<button
							class="-mb-px border-b-2 px-3 py-3 text-sm font-medium {tab === key ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}"
							onclick={() => (tab = key as typeof tab)}>{label}</button
						>
					{/each}
				</nav>

				{#if tab === 'comments'}
					<ul class="divide-y divide-slate-50">
						{#each comments as c (c.id)}
							<li class="group flex gap-3 px-5 py-4">
								<Avatar name={c.author_name ?? '?'} fileId={c.author_avatar_file_id} size="sm" />
								<div class="min-w-0 flex-1">
									<p class="text-sm">
										<span class="font-medium text-slate-900">{c.author_id ? (c.author_name ?? 'Former employee') : '⚡ Automation'}</span>
										<span class="ml-1 text-xs text-slate-400" title={formatDateTime(c.created_at)}>{formatRelative(c.created_at)}{c.edited_at ? ' · edited' : ''}</span>
									</p>
									<p class="mt-1 text-sm break-words whitespace-pre-line text-slate-700">
										{#each parseMentions(c.body) as part, i (i)}{#if 'id' in part}<a href="/employees/{part.id}" class="rounded bg-brand-50 px-1 font-medium text-brand-700 hover:underline">@{part.name}</a>{:else}{part.text}{/if}{/each}
									</p>
								</div>
								{#if task.result_comment_id === c.id}
									<span class="badge self-start bg-emerald-50 text-emerald-700">Result</span>
								{/if}
								{#if task.access.participate}
									<button class="invisible self-start text-xs text-slate-400 group-hover:visible hover:text-emerald-700" onclick={() => markResult(task!.result_comment_id === c.id ? null : c.id)}>{task.result_comment_id === c.id ? 'Unmark' : 'Mark as result'}</button>
								{/if}
								{#if c.author_id === session.user?.id || task.access.edit}
									<button class="invisible self-start text-slate-300 group-hover:visible hover:text-red-500" aria-label="Delete comment" onclick={() => deleteComment(c)}><Icon name="trash" class="size-4" /></button>
								{/if}
							</li>
						{:else}
							<li class="px-5 py-6 text-center text-sm text-slate-400">No comments yet.</li>
						{/each}
					</ul>
					<div class="space-y-2 border-t border-slate-100 px-5 py-4">
						<MentionTextarea bind:value={newComment} bind:encoded={newCommentEncoded} onsubmit={postComment} />
						<div class="flex items-center justify-between">
							<span class="text-xs text-slate-400">Ctrl+Enter to send</span>
							<button class="btn btn-primary" disabled={posting || !newComment.trim()} onclick={postComment}>Comment</button>
						</div>
					</div>
				{:else if tab === 'time'}
					<div class="p-5">
						{#if task.access.participate || task.access.is_creator}
							<form class="mb-4 flex flex-wrap gap-2" onsubmit={addManual}>
								<input class="input w-28" placeholder="1.5 or 1:30" bind:value={manualMinutes} aria-label="Hours" />
								<input class="input min-w-40 flex-1" placeholder="What did you work on? (optional)" bind:value={manualComment} />
								<button class="btn btn-secondary">Log time</button>
							</form>
						{/if}
						<ul class="divide-y divide-slate-50 text-sm">
							{#each timeEntries as e (e.id)}
								<li class="flex items-center gap-3 py-2">
									<span class="w-20 font-medium text-slate-800">{e.stopped_at ? formatDuration(e.seconds) : 'running…'}</span>
									<span class="flex-1 text-slate-600">{e.user_name}{e.comment ? ` — ${e.comment}` : ''}</span>
									<span class="text-xs text-slate-400">{formatDateTime(e.started_at)}</span>
								</li>
							{:else}
								<li class="py-4 text-center text-slate-400">No time logged yet.</li>
							{/each}
						</ul>
					</div>
				{:else}
					<ul class="divide-y divide-slate-50 text-sm">
						{#each history as h (h.id)}
							<li class="px-5 py-2.5 text-slate-600">
								<span class="font-medium text-slate-800">{h.actor_name ?? 'Someone'}</span>
								{FIELD_LABELS[h.field] ?? h.field}
								{#if ['checklist_done', 'checklist_undone', 'file_added', 'file_removed', 'created', 'stage', 'blocked_by_added', 'blocked_by_removed'].includes(h.field)}
									<span class="text-slate-800">“{h.new_value}”</span>
								{:else if !['description', 'parent'].includes(h.field)}
									<span class="text-slate-400">{historyValue(h, h.old_value)}</span> → <span class="text-slate-800">{historyValue(h, h.new_value)}</span>
								{/if}
								<span class="ml-1 text-xs text-slate-400">{formatRelative(h.created_at)}</span>
							</li>
						{/each}
					</ul>
				{/if}
			</section>
		</div>

		<!-- Sidebar -->
		<aside class="space-y-4">
			<section class="card divide-y divide-slate-100 text-sm">
				{#if task.start_date}
					<div class="px-5 py-3">
						<p class="text-xs text-slate-500">Starts</p>
						<p class="mt-0.5 font-medium text-slate-800" title={formatDateTime(task.start_date)}>{formatDeadline(task.start_date)}</p>
					</div>
				{/if}
				<div class="px-5 py-3">
					<p class="text-xs text-slate-500">Deadline</p>
					<p class="mt-0.5 font-medium {task.is_overdue ? 'text-red-600' : 'text-slate-800'}" title={task.deadline ? formatDateTime(task.deadline) : ''}>{formatDeadline(task.deadline)}</p>
				</div>
				<div class="px-5 py-3">
					<p class="text-xs text-slate-500">Priority</p>
					<p class="mt-0.5 font-medium {PRIORITY[task.priority].class}">{PRIORITY[task.priority].label}</p>
				</div>
				{#each [['responsible', 'Responsible'], ['creator', 'Created by'], ['assistant', 'Assistants'], ['watcher', 'Watchers']] as [role, label] (role)}
					{#if people(role).length}
						<div class="px-5 py-3">
							<p class="mb-1.5 text-xs text-slate-500">{label}</p>
							<ul class="space-y-1.5">
								{#each people(role) as p (p.user_id)}
									<li>
										<a href="/employees/{p.user_id}" class="flex items-center gap-2 hover:underline">
											<Avatar name={p.full_name} fileId={p.avatar_file_id} size="xs" />
											<span class="text-slate-800">{p.full_name}</span>
										</a>
									</li>
								{/each}
							</ul>
						</div>
					{/if}
				{/each}
				{#if task.requires_review}
					<div class="px-5 py-3 text-xs text-slate-500">✓ {task.creator_name} reviews the result before it closes</div>
				{/if}
				{#if task.skip_nonworking}
					<div class="px-5 py-3 text-xs text-slate-500">📅 Dates skip weekends and holidays</div>
				{/if}
				{#if task.template}
					<div class="px-5 py-3 text-xs text-slate-500">
						↻ {task.template.schedule ? `Recurring: ${task.template.schedule}` : 'Created from a template'}
						{#if task.template.mine}· <a class="text-brand-600 hover:underline" href="/tasks/templates/{task.template.id}">edit</a>{/if}
					</div>
				{/if}
				<div class="px-5 py-3 text-xs text-slate-400">Created {formatDateTime(task.created_at)}</div>
			</section>

			<TaskFieldsCard bind:task />

			<section class="card p-5 text-sm">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-xs text-slate-500">Time spent</p>
						<p class="mt-0.5 font-medium text-slate-800">
							{formatDuration(task.time_spent_secs)}
							{#if task.time_estimate_secs}<span class="font-normal text-slate-400"> of {formatDuration(task.time_estimate_secs)}</span>{/if}
						</p>
					</div>
					{#if (task.access.participate || task.access.is_creator) && task.status !== 'done'}
						<button class="btn {task.my_timer_started_at ? 'btn-danger' : 'btn-secondary'}" onclick={toggleTimer}>
							<Icon name={task.my_timer_started_at ? 'stop' : 'play'} class="size-4" />
							{task.my_timer_started_at ? formatClock(running) : 'Start timer'}
						</button>
					{/if}
				</div>
				{#if task.time_estimate_secs}
					<div class="mt-3 h-1.5 overflow-hidden rounded-full bg-slate-100">
						<div class="h-full rounded-full {task.time_spent_secs > task.time_estimate_secs ? 'bg-red-500' : 'bg-brand-500'}" style="width: {Math.min(100, (task.time_spent_secs / task.time_estimate_secs) * 100)}%"></div>
					</div>
				{/if}
			</section>
		</aside>
	</div>

	<TaskForm bind:open={editOpen} {task} onsaved={(t) => (task = t)} />
	<TaskForm bind:open={subtaskOpen} parentId={task.id} projectId={task.project_id} onsaved={() => load()} />
	<Modal open={!!assigning} title="Who does this?" size="sm" onclose={() => (assigning = null)}>
		{#if assigning}
			<p class="mb-3 text-sm text-slate-600">{assigning.title}</p>
			<UserPicker placeholder="Choose someone…" onpick={(u) => assignItem(u.id)} />
			<p class="mt-2 text-xs text-slate-500">They join the task as an assistant if they aren't on it.</p>
		{/if}
		{#snippet footer()}
			<button class="btn btn-ghost mr-auto" onclick={() => assignItem(null)}>Nobody</button>
			<button class="btn btn-secondary" onclick={() => (assigning = null)}>Cancel</button>
		{/snippet}
	</Modal>
{:else}
	<div class="card p-10 text-center text-slate-400">Loading…</div>
{/if}
