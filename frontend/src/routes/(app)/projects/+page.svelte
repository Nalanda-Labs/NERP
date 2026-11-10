<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { KIND_LABEL, KIND_NOUN, PROJECT_COLORS, VISIBILITY, type ProjectDetail, type ProjectSummary } from '#lib/tasks.ts';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	let projects = $state<ProjectSummary[] | null>(null);
	let q = $state('');
	let scope = $state<'mine' | 'all' | 'archived'>('mine');
	let kind = $state<'' | 'group' | 'project' | 'scrum'>('');

	async function load() {
		projects = await api.get<ProjectSummary[]>(
			'/projects' + query({ q, mine: scope === 'mine' || undefined, archived: scope === 'archived' || undefined, kind: kind || undefined })
		);
	}
	onMount(load);

	let timer: ReturnType<typeof setTimeout>;
	const onSearch = () => {
		clearTimeout(timer);
		timer = setTimeout(load, 250);
	};

	// ----- create -----
	let open = $state(false);
	let saving = $state(false);
	let form = $state({ name: '', description: '', visibility: 'private', color: 'indigo', kind: 'project', starts_on: '', ends_on: '' });
	let members = $state<PickedUser[]>([]);
	function start(k: 'group' | 'project' | 'scrum') {
		form = { name: '', description: '', visibility: k === 'group' ? 'open' : 'private', color: 'indigo', kind: k, starts_on: '', ends_on: '' };
		members = [];
		open = true;
	}

	async function create(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const dated = form.kind === 'project';
			const p = await api.post<ProjectDetail>('/projects', {
				...form,
				starts_on: dated ? form.starts_on || null : null,
				ends_on: dated ? form.ends_on || null : null,
				member_ids: members.map((m) => m.id)
			});
			open = false;
			goto(`/projects/${p.id}`);
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
</script>

<svelte:head><title>Workgroups · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Workgroups & projects</h1>
			<p class="text-sm text-slate-500">Spaces for teams and goals, each with its own feed, tasks, files, calendar and chat.</p>
		</div>
		{#if session.can(PERM.PROJECTS_CREATE)}
			<div class="flex gap-2">
				<button class="btn btn-secondary" onclick={() => start('group')}><Icon name="plus" class="size-4" /> New workgroup</button>
				<button class="btn btn-secondary" onclick={() => start('scrum')}><Icon name="plus" class="size-4" /> New Scrum team</button>
				<button class="btn btn-primary" onclick={() => start('project')}><Icon name="plus" class="size-4" /> New project</button>
			</div>
		{/if}
	</div>

	<div class="flex flex-wrap items-center gap-3">
		<nav class="flex rounded-lg bg-slate-100 p-1 text-sm">
			{#each [['mine', 'Mine'], ['all', 'All'], ['archived', 'Archived']] as [k, label] (k)}
				<button class="rounded-md px-3 py-1.5 font-medium {scope === k ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-600 hover:text-slate-900'}" onclick={() => ((scope = k as typeof scope), load())}>{label}</button>
			{/each}
		</nav>
		<select class="input w-44" aria-label="Kind" bind:value={kind} onchange={load}>
			<option value="">Groups and projects</option>
			<option value="group">Workgroups</option>
			<option value="project">Projects</option>
			<option value="scrum">Scrum teams</option>
		</select>
		<div class="relative min-w-48 flex-1">
			<Icon name="search" class="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-slate-400" />
			<input class="input pl-9" placeholder="Search" aria-label="Search groups" bind:value={q} oninput={onSearch} />
		</div>
	</div>

	{#if projects && projects.length}
		<div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
			{#each projects as p (p.id)}
				<a href="/projects/{p.id}" class="card group flex flex-col p-5">
					<div class="flex items-start gap-3">
						<span class="flex size-10 shrink-0 items-center justify-center rounded-lg text-lg font-semibold text-white {PROJECT_COLORS[p.color]?.bg}">{p.name[0]?.toUpperCase()}</span>
						<div class="min-w-0 flex-1">
							<h2 class="truncate font-semibold text-slate-900 group-hover:text-brand-700">{p.name}</h2>
							<p class="text-xs text-slate-500">
								{KIND_LABEL[p.kind]} · {VISIBILITY[p.visibility].label}{p.my_role ? ` · you're ${p.my_role === 'owner' ? 'an owner' : `a ${p.my_role}`}` : p.requested ? ' · request sent' : ''}
							</p>
						</div>
					</div>
					{#if p.description}<p class="mt-3 line-clamp-2 text-sm text-slate-600">{p.description}</p>{/if}
					{#if p.starts_on || p.ends_on}<p class="mt-2 text-xs text-slate-500">{p.starts_on ? formatDate(p.starts_on) : '…'} – {p.ends_on ? formatDate(p.ends_on) : '…'}</p>{/if}
					<div class="mt-auto flex gap-4 pt-4 text-xs text-slate-500">
						<span>{p.member_count} {p.member_count === 1 ? 'member' : 'members'}</span>
						<span>{p.open_task_count} open tasks</span>
						{#if p.overdue_task_count}<span class="font-medium text-red-600">{p.overdue_task_count} overdue</span>{/if}
					</div>
				</a>
			{/each}
		</div>
	{:else if projects}
		<div class="card flex flex-col items-center gap-2 p-14 text-center">
			<Icon name="briefcase" class="size-8 text-slate-300" />
			<p class="font-medium text-slate-700">{scope === 'mine' ? "You're not in any groups yet" : 'Nothing found'}</p>
			{#if scope === 'mine'}<button class="text-sm text-brand-600 hover:underline" onclick={() => ((scope = 'all'), load())}>Browse groups you can join</button>{/if}
		</div>
	{/if}
</div>

<Modal bind:open title={`New ${KIND_NOUN[form.kind as 'group']}`}>
	<form id="new-project" class="space-y-4" onsubmit={create}>
		<div><label class="label" for="p-name">Name</label><input id="p-name" class="input" required maxlength="200" bind:value={form.name} /></div>
		<div><label class="label" for="p-desc">Description</label><textarea id="p-desc" class="input" rows="3" bind:value={form.description}></textarea></div>
		{#if form.kind === 'project'}
			<div class="grid grid-cols-2 gap-3">
				<div><label class="label" for="p-start">Starts</label><input id="p-start" type="date" class="input" bind:value={form.starts_on} /></div>
				<div><label class="label" for="p-end">Ends</label><input id="p-end" type="date" class="input" bind:value={form.ends_on} /></div>
			</div>
		{/if}
		<fieldset>
			<legend class="label">Who can see it?</legend>
			<div class="grid gap-2 sm:grid-cols-3">
				{#each Object.entries(VISIBILITY).map(([v, x]) => [v, x.label, x.hint]) as [v, label, hint] (v)}
					<label class="flex cursor-pointer items-start gap-2 rounded-lg border p-3 text-sm {form.visibility === v ? 'border-brand-500 bg-brand-50' : 'border-slate-200'}">
						<input type="radio" class="mt-0.5 text-brand-600" value={v} bind:group={form.visibility} />
						<span><span class="block font-medium text-slate-800">{label}</span><span class="text-xs text-slate-500">{hint}</span></span>
					</label>
				{/each}
			</div>
		</fieldset>
		<div>
			<span class="label">Colour</span>
			<div class="flex gap-2">
				{#each Object.entries(PROJECT_COLORS) as [name, c] (name)}
					<button type="button" aria-label={name} class="size-7 rounded-full {c.bg} {form.color === name ? 'ring-2 ring-slate-400 ring-offset-2' : ''}" onclick={() => (form.color = name)}></button>
				{/each}
			</div>
		</div>
		<div>
			<span class="label">Members</span>
			<MultiUserPicker bind:value={members} exclude={[session.user!.id]} />
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="new-project" disabled={saving}>Create {KIND_NOUN[form.kind as 'group']}</button>
	{/snippet}
</Modal>
