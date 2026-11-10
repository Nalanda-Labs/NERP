<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Gantt from '#lib/components/Gantt.svelte';
	import GroupCalendar from '#lib/components/groups/GroupCalendar.svelte';
	import GroupFeed from '#lib/components/groups/GroupFeed.svelte';
	import GroupFiles from '#lib/components/groups/GroupFiles.svelte';
	import StageEditor from '#lib/components/StageEditor.svelte';
	import ChatCallButtons from '#lib/components/ChatCallButtons.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import TaskBoard from '#lib/components/TaskBoard.svelte';
	import TaskForm from '#lib/components/TaskForm.svelte';
	import TaskTable from '#lib/components/TaskTable.svelte';
	import BoardAutomation from '#lib/components/BoardAutomation.svelte';
	import ProjectFields from '#lib/components/ProjectFields.svelte';
	import ScrumPlanning from '#lib/components/scrum/ScrumPlanning.svelte';
	import ScrumReports from '#lib/components/scrum/ScrumReports.svelte';
	import ScrumSettings from '#lib/components/scrum/ScrumSettings.svelte';
	import ScrumSprint from '#lib/components/scrum/ScrumSprint.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { KIND_LABEL, PROJECT_COLORS, STAGE_COLORS, VISIBILITY, type ProjectDetail, type ScrumOverview, type Stage, type TaskDetail, type TaskSummary } from '#lib/tasks.ts';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { Page } from '#lib/types.ts';

	const id = $derived(page.params.id!);
	let project = $state<ProjectDetail | null>(null);
	let tasks = $state<TaskSummary[]>([]);
	let notFound = $state(false);
	type Tab = 'feed' | 'board' | 'list' | 'gantt' | 'files' | 'calendar' | 'members' | 'planning' | 'sprint' | 'reports';
	let tab = $state<Tab>('board');
	const TASK_TABS = ['board', 'list', 'gantt'];
	let deps = $state<{ task_id: string; depends_on_id: string }[]>([]);
	let stages = $state<Stage[]>([]);
	let columnsOpen = $state(false);
	let automationOpen = $state(false);
	// ----- copying the project -----
	let copyOpen = $state(false);
	let copyForm = $state({ name: '', starts_on: '', members: true, tasks: true });
	function openCopy() {
		copyForm = { name: `${project!.name} (copy)`, starts_on: '', members: true, tasks: true };
		settingsOpen = false;
		copyOpen = true;
	}
	async function copyProject(e: SubmitEvent) {
		e.preventDefault();
		try {
			const p = await api.post<ProjectDetail>(`/projects/${id}/copy`, { ...copyForm, starts_on: copyForm.starts_on || null });
			copyOpen = false;
			toasts.success('Copied');
			goto(`/projects/${p.id}`);
		} catch (err) {
			toasts.error(err);
		}
	}
	const columns = $derived(stages.map((s) => ({ key: s.id, label: s.name, dot: STAGE_COLORS[s.color] ?? 'bg-slate-400' })));
	const moveToStage = (t: TaskSummary, stage: string, after: string | null, before: string | null) =>
		api.post<TaskDetail>(`/tasks/${t.id}/stage`, { stage_id: stage, after_id: after, before_id: before });
	let showDone = $state(false);
	let formOpen = $state(false);

	async function loadProject() {
		try {
			project = await api.get<ProjectDetail>(`/projects/${id}`);
			stages = project.stages;
			if (project.kind === 'scrum' && project.access.view) await loadScrum();
			else scrum = null;
		} catch {
			notFound = true;
		}
	}
	// ----- Scrum teams -----
	let scrum = $state<ScrumOverview | null>(null);
	let scrumSettingsOpen = $state(false);
	async function loadScrum() {
		scrum = await api.get<ScrumOverview>(`/projects/${id}/scrum`);
	}
	const tabs = $derived(
		project?.kind === 'scrum'
			? [['feed', 'Feed', 'message'], ['planning', 'Planning', 'list'], ['sprint', 'Active sprint', 'kanban'], ['reports', 'Reports', 'funnel']]
			: [['feed', 'Feed', 'message'], ['board', 'Board', 'kanban'], ['list', 'List', 'list'], ['gantt', 'Gantt', 'gantt']]
	);
	async function loadTasks() {
		if (!project?.access.view || !TASK_TABS.includes(tab)) return;
		if (tab === 'gantt') {
			const g = await api.get<{ tasks: TaskSummary[]; dependencies: typeof deps }>('/tasks/gantt' + query({ project_id: id }));
			tasks = g.tasks;
			deps = g.dependencies;
			return;
		}
		const statuses = tab === 'board' ? 'new,in_progress,review,done,deferred' : showDone ? '' : 'new,in_progress,review';
		const p = await api.get<Page<TaskSummary>>('/tasks' + query({ project_id: id, status: statuses, sort: tab === 'board' ? 'position' : 'deadline', limit: 200 }));
		tasks = p.items;
	}

	$effect(() => {
		id;
		project = null;
		notFound = false;
		loadProject().then(() => {
			// Workgroups open on their feed, projects on their board (or the tab asked for).
			const asked = page.url.searchParams.get('tab') as Tab | null;
			tab = asked ?? (project?.kind === 'group' ? 'feed' : project?.kind === 'scrum' ? (scrum?.sprints.some((s) => s.status === 'active') ? 'sprint' : 'planning') : 'board');
			loadTasks();
		});
	});

	onMount(() => {
		let pending: ReturnType<typeof setTimeout>;
		const offTask = realtime.on('task.changed', (d: { project_id: string | null }) => {
			if (d.project_id !== id) return;
			clearTimeout(pending);
			pending = setTimeout(() => {
				loadTasks();
				loadProject();
			}, 300);
		});
		const offStages = realtime.on('project.stages', (d: { project_id: string }) => d.project_id === id && loadProject());
		const offScrum = realtime.on('scrum.changed', (d: { project_id: string }) => d.project_id === id && loadScrum());
		return () => {
			offTask();
			offStages();
			offScrum();
		};
	});

	// ----- members -----
	async function setRole(userId: string, role: string) {
		try {
			project = await api.put<ProjectDetail>(`/projects/${id}/members/${userId}`, { role });
		} catch (e) {
			toasts.error(e);
		}
	}
	async function removeMember(userId: string) {
		try {
			project = await api.del<ProjectDetail>(`/projects/${id}/members/${userId}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	const noun = $derived(project?.kind === 'group' ? 'group' : 'project');
	async function join() {
		try {
			if (project!.visibility === 'closed') {
				const message = prompt('A note for the owners (optional)');
				if (message === null) return;
				project = await api.post<ProjectDetail>(`/projects/${id}/join`, { message });
				toasts.success('Request sent');
			} else {
				project = await api.post<ProjectDetail>(`/projects/${id}/join`);
				toasts.success(`You joined the ${noun}`);
				loadTasks();
			}
		} catch (e) {
			toasts.error(e);
		}
	}
	async function withdraw() {
		await api.del(`/projects/${id}/requests/${session.user!.id}`);
		await loadProject();
	}
	const groupChat = async () => (await api.post<{ chat_id: string }>(`/projects/${id}/chat`)).chat_id;

	// ----- join requests (owners) -----
	type Request = { user_id: string; full_name: string; position: string | null; avatar_file_id: string | null; message: string | null; created_at: string };
	let requests = $state<Request[]>([]);
	$effect(() => {
		if (tab === 'members' && project?.access.manage && project.request_count) {
			api.get<{ items: Request[] }>(`/projects/${id}/requests`).then((r) => (requests = r.items));
		} else requests = [];
	});
	async function answer(userId: string, approve: boolean) {
		try {
			project = await api.post<ProjectDetail>(`/projects/${id}/requests/${userId}`, { approve });
			requests = requests.filter((r) => r.user_id !== userId);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function leave() {
		if (!confirm('Leave this project?')) return;
		try {
			await api.post(`/projects/${id}/leave`);
			goto('/projects');
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- settings -----
	let settingsOpen = $state(false);
	let settings = $state({ name: '', description: '', visibility: 'private', color: 'indigo', kind: 'project', starts_on: '', ends_on: '' });
	function openSettings() {
		const p = project!;
		settings = { name: p.name, description: p.description ?? '', visibility: p.visibility, color: p.color, kind: p.kind, starts_on: p.starts_on ?? '', ends_on: p.ends_on ?? '' };
		settingsOpen = true;
	}
	async function saveSettings(e: SubmitEvent) {
		e.preventDefault();
		try {
			const dated = settings.kind === 'project';
			project = await api.patch<ProjectDetail>(`/projects/${id}`, {
				...settings,
				description: settings.description || null,
				starts_on: dated ? settings.starts_on || null : null,
				ends_on: dated ? settings.ends_on || null : null
			});
			settingsOpen = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function setArchived(v: boolean) {
		project = await api.patch<ProjectDetail>(`/projects/${id}`, { is_archived: v });
		settingsOpen = false;
		toasts.success(v ? 'Project archived' : 'Project restored');
	}
	async function removeProject() {
		if (!confirm(`Delete "${project!.name}"? This cannot be undone.`)) return;
		try {
			await api.del(`/projects/${id}`);
			goto('/projects');
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>{project?.name ?? 'Workgroup'} · NERP</title></svelte:head>

{#if notFound}
	<div class="card p-10 text-center text-slate-500">This group doesn't exist or is private.</div>
{:else if project}
	<a href="/projects" class="mb-4 inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800">
		<Icon name="chevronRight" class="size-4 rotate-180" /> Workgroups
	</a>

	<div class="mb-5 flex flex-wrap items-start gap-4">
		<span class="flex size-12 shrink-0 items-center justify-center rounded-xl text-xl font-semibold text-white {PROJECT_COLORS[project.color]?.bg}">{project.name[0]?.toUpperCase()}</span>
		<div class="min-w-0 flex-1">
			<h1 class="text-2xl font-semibold text-slate-900">
				{project.name}
				{#if project.is_archived}<span class="badge ml-2 bg-slate-100 align-middle text-slate-500">Archived</span>{/if}
			</h1>
			<p class="text-sm text-slate-500" data-group-meta>
				{KIND_LABEL[project.kind]} · {VISIBILITY[project.visibility].label} · {project.member_count} {project.member_count === 1 ? 'member' : 'members'}
				{#if project.access.view}· {project.open_task_count} open tasks{/if}
				{#if project.overdue_task_count}· <span class="font-medium text-red-600">{project.overdue_task_count} overdue</span>{/if}
				{#if project.starts_on || project.ends_on}· {project.starts_on ? formatDate(project.starts_on) : '…'} – {project.ends_on ? formatDate(project.ends_on) : '…'}{/if}
			</p>
			{#if project.description}<p class="mt-2 max-w-3xl text-sm text-slate-600">{project.description}</p>{/if}
		</div>
		<div class="flex flex-wrap gap-2">
			{#if !project.access.role && project.visibility === 'open'}
				<button class="btn btn-secondary" onclick={join}>Join {noun}</button>
			{:else if !project.access.role && project.visibility === 'closed'}
				{#if project.requested}
					<span class="badge self-center bg-sky-50 text-sky-700">Request sent</span>
					<button class="btn btn-ghost" onclick={withdraw}>Withdraw</button>
				{:else}
					<button class="btn btn-primary" onclick={join}>Ask to join</button>
				{/if}
			{/if}
			{#if project.access.role}
				<ChatCallButtons chat={groupChat} call={project.member_count > 1} />
			{/if}
			{#if scrum?.can_plan}
				<button class="btn btn-secondary" onclick={() => (scrumSettingsOpen = true)}>Scrum</button>
			{/if}
			{#if project.access.manage}
				<button class="btn btn-secondary" onclick={openSettings}><Icon name="settings" class="size-4" /> Settings</button>
			{/if}
			{#if project.access.contribute && !project.is_archived && (TASK_TABS.includes(tab) || tab === 'planning')}
				<button class="btn btn-primary" onclick={() => (formOpen = true)}><Icon name="plus" class="size-4" /> New task</button>
			{/if}
		</div>
	</div>

	{#if !project.access.view}
		<div class="card max-w-2xl p-6" data-group-card>
			<p class="text-sm text-slate-600">This group is closed: its members see its posts, tasks and files. {project.requested ? 'The owners will look at your request.' : 'Ask to join, and an owner will let you in.'}</p>
			<h2 class="mt-4 mb-2 text-sm font-medium text-slate-700">Owners</h2>
			<ul class="flex flex-wrap gap-3">
				{#each project.members as m (m.user_id)}
					<li class="flex items-center gap-2"><Avatar name={m.full_name} fileId={m.avatar_file_id} size="sm" /><span class="text-sm text-slate-800">{m.full_name}</span></li>
				{/each}
			</ul>
		</div>
	{:else}
	<div class="mb-4 flex flex-wrap items-center gap-3 border-b border-slate-200" role="tablist">
		{#each [...tabs, ['files', 'Files', 'folder'], ['calendar', 'Calendar', 'calendar'], ['members', `Members (${project.members.length})${project.access.manage && project.request_count ? ` · ${project.request_count} asking` : ''}`, 'users']] as [k, label, icon] (k)}
			<button
				role="tab"
				aria-selected={tab === k}
				class="-mb-px flex items-center gap-2 border-b-2 px-3 py-2.5 text-sm font-medium {tab === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}"
				onclick={() => {
					tab = k as Tab;
					loadTasks();
				}}
			>
				<Icon name={icon as 'list'} class="size-4" />{label}
			</button>
		{/each}
		{#if (tab === 'board' || tab === 'sprint') && project.access.manage}
			<button class="btn btn-ghost mb-1 ml-auto text-sm" onclick={() => (automationOpen = true)} data-automation>⚡ Automation</button>
			<button class="btn btn-ghost mb-1 text-sm" onclick={() => (columnsOpen = true)}><Icon name="columns" class="size-4" /> Edit columns</button>
		{/if}
		<a class="btn btn-ghost mb-1 text-sm {(tab === 'board' || tab === 'sprint') && project.access.manage ? '' : 'ml-auto'}" href="/flows?project_id={project.id}">Flows</a>
		{#if tab === 'list'}
			<label class="ml-auto flex items-center gap-2 pb-2 text-sm text-slate-600">
				<input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={showDone} onchange={loadTasks} /> Show finished
			</label>
		{/if}
	</div>

	{#if tab === 'feed'}
		<GroupFeed projectId={project.id} canPost={project.access.contribute} owner={project.access.manage} />
	{:else if tab === 'files'}
		<GroupFiles projectId={project.id} />
	{:else if tab === 'calendar'}
		<GroupCalendar projectId={project.id} canAdd={project.access.contribute} />
	{:else if tab === 'planning' && scrum}
		<ScrumPlanning projectId={project.id} overview={scrum} onchanged={loadScrum} />
	{:else if tab === 'sprint' && scrum}
		<ScrumSprint overview={scrum} {stages} onchanged={loadScrum} />
	{:else if tab === 'reports' && scrum}
		<ScrumReports projectId={project.id} />
	{:else if tab === 'board'}
		<TaskBoard bind:tasks {columns} columnOf={(t) => t.stage_id} move={moveToStage} onchanged={loadProject} />
	{:else if tab === 'gantt'}
		<Gantt bind:tasks dependencies={deps} onchanged={loadTasks} />
	{:else if tab === 'list'}
		<TaskTable {tasks} showProject={false} empty="No tasks in this project yet." />
	{:else}
		{#if requests.length}
			<div class="card mb-4 max-w-3xl" aria-label="Requests to join">
				<h2 class="border-b border-slate-100 px-4 py-3 text-sm font-semibold text-slate-800">Asking to join</h2>
				<ul class="divide-y divide-slate-100">
					{#each requests as r (r.user_id)}
						<li class="flex items-center gap-3 px-4 py-3" data-request={r.full_name}>
							<Avatar name={r.full_name} fileId={r.avatar_file_id} size="sm" />
							<div class="min-w-0 flex-1">
								<span class="block truncate text-sm font-medium text-slate-800">{r.full_name}</span>
								{#if r.message}<span class="block truncate text-xs text-slate-500">“{r.message}”</span>{/if}
							</div>
							<button class="btn btn-primary" onclick={() => answer(r.user_id, true)}>Let in</button>
							<button class="btn btn-ghost" onclick={() => answer(r.user_id, false)}>Decline</button>
						</li>
					{/each}
				</ul>
			</div>
		{/if}
		<div class="card max-w-3xl">
			{#if project.access.manage}
				<div class="border-b border-slate-100 p-4">
					<UserPicker placeholder="Add a member…" exclude={project.members.map((m) => m.user_id)} onpick={(u) => setRole(u.id, 'member')} />
				</div>
			{/if}
			<ul class="divide-y divide-slate-100">
				{#each project.members as m (m.user_id)}
					<li class="flex items-center gap-3 px-4 py-3">
						<Avatar name={m.full_name} fileId={m.avatar_file_id} size="sm" />
						<a href="/employees/{m.user_id}" class="min-w-0 flex-1">
							<span class="block truncate text-sm font-medium text-slate-800">{m.full_name}</span>
							<span class="block truncate text-xs text-slate-500">{m.position ?? ''}</span>
						</a>
						{#if project.access.manage}
							<select class="input w-32" value={m.role} onchange={(e) => setRole(m.user_id, e.currentTarget.value)}>
								<option value="owner">Owner</option>
								<option value="member">Member</option>
								<option value="viewer">Viewer</option>
							</select>
							<button class="btn btn-ghost p-1.5 text-slate-400 hover:text-red-600" aria-label="Remove member" onclick={() => removeMember(m.user_id)}><Icon name="x" class="size-4" /></button>
						{:else}
							<span class="badge bg-slate-100 text-slate-600 capitalize">{m.role}</span>
						{/if}
					</li>
				{/each}
			</ul>
			{#if project.access.role}
				<div class="border-t border-slate-100 px-4 py-3 text-right">
					<button class="text-sm text-red-600 hover:underline" onclick={leave}>Leave {noun}</button>
				</div>
			{/if}
		</div>
	{/if}
	{/if}

	<StageEditor bind:open={columnsOpen} projectId={project.id} bind:stages />
	<BoardAutomation bind:open={automationOpen} projectId={project.id} {stages} />
	<Modal bind:open={copyOpen} title="Copy {noun}">
		<form id="copy-project" class="space-y-3" onsubmit={copyProject}>
			<p class="text-sm text-slate-500">A new {noun} with this one's columns, task fields and automation{project.kind === 'scrum' ? ' and definition of done' : ''}.</p>
			<div><label class="label" for="cp-name">Name</label><input id="cp-name" class="input" required maxlength="200" bind:value={copyForm.name} /></div>
			<div><label class="label" for="cp-start">Starts on <span class="font-normal text-slate-400">— task dates move with it</span></label><input id="cp-start" type="date" class="input" bind:value={copyForm.starts_on} /></div>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={copyForm.tasks} /> Copy its tasks (as new, with checklists and subtasks)</label>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={copyForm.members} /> Copy its members</label>
		</form>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (copyOpen = false)}>Cancel</button>
			<button class="btn btn-primary" form="copy-project">Copy</button>
		{/snippet}
	</Modal>
	<TaskForm bind:open={formOpen} projectId={project.id} onsaved={() => (loadTasks(), loadProject())} />
	{#if scrum}
		<ScrumSettings bind:open={scrumSettingsOpen} projectId={project.id} overview={scrum} members={project.members} onchanged={loadScrum} />
	{/if}

	<Modal bind:open={settingsOpen} title="Settings">
		<form id="project-settings" class="space-y-4" onsubmit={saveSettings}>
			<div><label class="label" for="ps-name">Name</label><input id="ps-name" class="input" required bind:value={settings.name} /></div>
			<div><label class="label" for="ps-desc">Description</label><textarea id="ps-desc" class="input" rows="3" bind:value={settings.description}></textarea></div>
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<label class="label" for="ps-kind">Kind</label>
					<select id="ps-kind" class="input" bind:value={settings.kind}>
						<option value="group">Workgroup</option>
						<option value="project">Project (with dates)</option>
						<option value="scrum">Scrum team</option>
					</select>
				</div>
				<div>
					<label class="label" for="ps-vis">Visibility</label>
					<select id="ps-vis" class="input" bind:value={settings.visibility}>
						{#each Object.entries(VISIBILITY) as [v, x] (v)}<option value={v}>{x.label} — {x.hint.toLowerCase()}</option>{/each}
					</select>
				</div>
				{#if settings.kind === 'project'}
					<div><label class="label" for="ps-start">Starts</label><input id="ps-start" type="date" class="input" bind:value={settings.starts_on} /></div>
					<div><label class="label" for="ps-end">Ends</label><input id="ps-end" type="date" class="input" bind:value={settings.ends_on} /></div>
				{/if}
			</div>
			<div>
				<span class="label">Colour</span>
				<div class="flex gap-2">
					{#each Object.entries(PROJECT_COLORS) as [name, c] (name)}
						<button type="button" aria-label={name} class="size-7 rounded-full {c.bg} {settings.color === name ? 'ring-2 ring-slate-400 ring-offset-2' : ''}" onclick={() => (settings.color = name)}></button>
					{/each}
				</div>
			</div>
			<div class="border-t border-slate-100 pt-4"><ProjectFields projectId={project.id} /></div>
			<div class="flex flex-wrap gap-2 border-t border-slate-100 pt-4">
				<button type="button" class="btn btn-secondary" onclick={openCopy}>Copy as a new {noun}</button>
				<button type="button" class="btn btn-secondary" onclick={() => setArchived(!project!.is_archived)}>{project.is_archived ? `Restore ${noun}` : `Archive ${noun}`}</button>
				<button type="button" class="btn btn-ghost text-red-600 hover:bg-red-50" onclick={removeProject}>Delete {noun}</button>
			</div>
		</form>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (settingsOpen = false)}>Cancel</button>
			<button class="btn btn-primary" form="project-settings">Save</button>
		{/snippet}
	</Modal>
{:else}
	<div class="card p-10 text-center text-slate-400">Loading…</div>
{/if}
