<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM, type Department, type Page, type User } from '#lib/types.ts';

	let departments = $state<Department[]>([]);
	let expanded = $state<Record<string, boolean>>({});
	let members = $state<User[]>([]);
	let membersTotal = $state(0);

	const canManage = $derived(session.can(PERM.DEPARTMENTS_MANAGE));
	const canSeePeople = $derived(session.can(PERM.USERS_VIEW));
	const selectedId = $derived(page.url.searchParams.get('d'));
	const selected = $derived(departments.find((d) => d.id === selectedId) ?? null);

	const children = (parent: string | null) => departments.filter((d) => d.parent_id === parent);
	const hasChildren = (id: string) => departments.some((d) => d.parent_id === id);

	function path(d: Department): Department[] {
		const out: Department[] = [];
		let cur: Department | undefined = d;
		while (cur) {
			out.unshift(cur);
			cur = departments.find((x) => x.id === cur!.parent_id);
		}
		return out;
	}

	async function load() {
		departments = await api.get<Department[]>('/departments');
		// Expand the roots, and the ancestors of whatever is selected.
		for (const r of children(null)) expanded[r.id] ??= true;
		if (selected) for (const p of path(selected)) expanded[p.id] = true;
		if (!selectedId && departments.length) select(children(null)[0]!.id);
	}

	function select(id: string) {
		goto(`/structure?d=${id}`, { replace: true, reset: false });
	}

	$effect(() => {
		const id = selectedId;
		members = [];
		if (id && canSeePeople) {
			api.get<Page<User>>('/users' + query({ department_id: id, limit: 200 })).then((p) => {
				members = p.items;
				membersTotal = p.total;
			});
		}
	});

	onMount(load);

	// ----- create / edit -----
	let formOpen = $state(false);
	let editing = $state<Department | null>(null);
	let saving = $state(false);
	let form = $state({ name: '', description: '', parent_id: '' as string, head_user_id: null as string | null, deputy_user_id: null as string | null, sort: 100, hidden: false });
	let headLabel = $state<string | null>(null);
	let deputyLabel = $state<string | null>(null);

	function openCreate(parent: Department | null) {
		editing = null;
		form = { name: '', description: '', parent_id: parent?.id ?? '', head_user_id: null, deputy_user_id: null, sort: 100, hidden: false };
		headLabel = null;
		deputyLabel = null;
		formOpen = true;
	}

	function openEdit(d: Department) {
		editing = d;
		form = {
			name: d.name,
			description: d.description ?? '',
			parent_id: d.parent_id ?? '',
			head_user_id: d.head_user_id,
			deputy_user_id: d.deputy_user_id ?? null,
			sort: d.sort,
			hidden: !!d.hidden
		};
		headLabel = d.head_name;
		deputyLabel = d.deputy_name ?? null;
		formOpen = true;
	}

	/** Departments that can be chosen as a parent (not the edited one or its descendants). */
	const parentOptions = $derived.by(() => {
		if (!editing) return departments;
		const banned = new Set<string>([editing.id]);
		let grew = true;
		while (grew) {
			grew = false;
			for (const d of departments) {
				if (d.parent_id && banned.has(d.parent_id) && !banned.has(d.id)) {
					banned.add(d.id);
					grew = true;
				}
			}
		}
		return departments.filter((d) => !banned.has(d.id));
	});

	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		const body = { ...form, parent_id: form.parent_id || null, sort: Number(form.sort) || 100 };
		try {
			const d = editing
				? await api.patch<Department>(`/departments/${editing.id}`, body)
				: await api.post<Department>('/departments', body);
			toasts.success(editing ? 'Department updated' : 'Department created');
			formOpen = false;
			await load();
			select(d.id);
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	async function remove(d: Department) {
		if (!confirm(`Delete "${d.name}"? Its members stay in NERP but leave this department.`)) return;
		try {
			await api.del(`/departments/${d.id}`);
			toasts.success('Department deleted');
			const parent = d.parent_id;
			goto(parent ? `/structure?d=${parent}` : '/structure', { replace: true });
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}

	async function addMember(u: User) {
		try {
			await api.post(`/departments/${selected!.id}/members`, { user_id: u.id });
			members = [...members, u].sort((a, b) => a.last_name.localeCompare(b.last_name));
			departments = await api.get<Department[]>('/departments');
		} catch (err) {
			toasts.error(err);
		}
	}

	async function removeMember(u: User) {
		try {
			await api.del(`/departments/${selected!.id}/members/${u.id}`);
			members = members.filter((m) => m.id !== u.id);
			departments = await api.get<Department[]>('/departments');
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>Company structure · NERP</title></svelte:head>

{#snippet tree(parent: string | null, depth: number)}
	{#each children(parent) as d (d.id)}
		<li>
			<div
				class="flex items-center gap-1 rounded-lg pr-2 text-sm {selectedId === d.id ? 'bg-brand-50 text-brand-700' : 'text-slate-700 hover:bg-slate-50'}"
				style="padding-left: {depth * 16 + 4}px"
			>
				{#if hasChildren(d.id)}
					<button class="rounded p-1 text-slate-400 hover:text-slate-700" aria-label="Toggle" onclick={() => (expanded[d.id] = !expanded[d.id])}>
						<Icon name="chevronRight" class="size-4 transition {expanded[d.id] ? 'rotate-90' : ''}" />
					</button>
				{:else}
					<span class="w-6"></span>
				{/if}
				<button class="flex flex-1 items-center justify-between gap-2 py-2 text-left" onclick={() => select(d.id)}>
					<span class="truncate font-medium">{d.name}</span>
					<span class="text-xs text-slate-400">{d.member_count}</span>
				</button>
			</div>
			{#if expanded[d.id] && hasChildren(d.id)}
				<ul>{@render tree(d.id, depth + 1)}</ul>
			{/if}
		</li>
	{/each}
{/snippet}

<div class="space-y-5">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Company structure</h1>
			<p class="text-sm text-slate-500">{departments.length} departments</p>
		</div>
		{#if canManage}
			<button class="btn btn-primary" onclick={() => openCreate(selected)}>
				<Icon name="plus" class="size-4" /> {selected ? 'Add sub-department' : 'Add department'}
			</button>
		{/if}
	</div>

	<div class="grid gap-6 lg:grid-cols-[320px_1fr]">
		<nav class="card h-fit p-2">
			<ul>{@render tree(null, 0)}</ul>
		</nav>

		{#if selected}
			<div class="space-y-6">
				<section class="card p-6">
					<div class="flex flex-wrap items-start justify-between gap-4">
						<div>
							<p class="mb-1 text-xs text-slate-400">
								{#each path(selected).slice(0, -1) as p (p.id)}<button class="hover:text-slate-700" onclick={() => select(p.id)}>{p.name}</button> / {/each}
							</p>
							<h2 class="text-xl font-semibold text-slate-900">{selected.name}</h2>
							{#if selected.description}<p class="mt-1 text-sm text-slate-600">{selected.description}</p>{/if}
						</div>
						{#if canManage}
							<div class="flex gap-2">
								<button class="btn btn-secondary" onclick={() => openEdit(selected)}><Icon name="edit" class="size-4" /> Edit</button>
								<button class="btn btn-ghost text-red-600 hover:bg-red-50" onclick={() => remove(selected)} aria-label="Delete">
									<Icon name="trash" class="size-4" />
								</button>
							</div>
						{/if}
					</div>
					<div class="mt-5 flex items-center gap-3 rounded-xl bg-surface-container p-3">
						{#if selected.head_user_id && selected.head_name}
							<Avatar name={selected.head_name} fileId={selected.head_avatar_file_id} />
							<div>
								<p class="text-xs text-slate-500">Head of department</p>
								<a href="/employees/{selected.head_user_id}" class="font-medium text-slate-800 hover:underline">{selected.head_name}</a>
							</div>
						{:else}
							<p class="text-sm text-slate-500">No head of department assigned.</p>
						{/if}
						{#if selected.deputy_user_id && selected.deputy_name}
							<div class="ml-6"><p class="text-xs text-slate-500">Deputy</p><a href="/employees/{selected.deputy_user_id}" class="font-medium text-slate-800 hover:underline" data-deputy>{selected.deputy_name}</a></div>
						{/if}
					</div>
				</section>

				{#if canSeePeople}
					<section class="card">
						<div class="flex flex-wrap items-center justify-between gap-3 border-b border-slate-100 px-5 py-3.5">
							<h3 class="font-semibold text-slate-900">Members <span class="font-normal text-slate-400">({membersTotal})</span></h3>
							{#if canManage}
								<div class="w-full sm:w-72">
									<UserPicker placeholder="Add a member…" exclude={members.map((m) => m.id)} onpick={addMember} />
								</div>
							{/if}
						</div>
						<ul class="divide-y divide-slate-100">
							{#each members as m (m.id)}
								<li class="flex items-center gap-3 px-5 py-3">
									<Avatar name={m.full_name} fileId={m.avatar_file_id} size="sm" />
									<a href="/employees/{m.id}" class="min-w-0 flex-1">
										<span class="block truncate text-sm font-medium text-slate-800">
											{m.full_name}
											{#if m.id === selected.head_user_id}<span class="badge ml-1 bg-amber-50 text-amber-700">Head</span>{/if}
										</span>
										<span class="block truncate text-xs text-slate-500">{m.position ?? m.email}</span>
									</a>
									{#if canManage}
										<button class="btn btn-ghost p-1.5 text-slate-400 hover:text-red-600" aria-label="Remove from department" onclick={() => removeMember(m)}>
											<Icon name="x" class="size-4" />
										</button>
									{/if}
								</li>
							{:else}
								<li class="px-5 py-8 text-center text-sm text-slate-500">Nobody is in this department yet.</li>
							{/each}
						</ul>
					</section>
				{/if}
			</div>
		{:else}
			<div class="card p-10 text-center text-slate-500">Select a department.</div>
		{/if}
	</div>
</div>

<Modal bind:open={formOpen} title={editing ? `Edit ${editing.name}` : 'New department'}>
	<form id="dept-form" class="space-y-4" onsubmit={save}>
		<div><label class="label" for="d-name">Name</label><input id="d-name" class="input" required bind:value={form.name} /></div>
		<div>
			<label class="label" for="d-parent">Parent department</label>
			<select id="d-parent" class="input" bind:value={form.parent_id}>
				<option value="">— Top level —</option>
				{#each parentOptions as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
			</select>
		</div>
		<div>
			<label class="label" for="d-head">Head of department</label>
			<div class="flex gap-2">
				<div class="flex-1"><UserPicker bind:value={form.head_user_id} bind:label={headLabel} /></div>
				{#if form.head_user_id}
					<button type="button" class="btn btn-secondary" onclick={() => ((form.head_user_id = null), (headLabel = null))}>Clear</button>
				{/if}
			</div>
		</div>
		<div>
			<label class="label" for="d-deputy">Deputy head <span class="font-normal text-slate-400">(manages and approves with the head)</span></label>
			<div class="flex gap-2">
				<div class="flex-1"><UserPicker bind:value={form.deputy_user_id} bind:label={deputyLabel} /></div>
				{#if form.deputy_user_id}<button type="button" class="btn btn-secondary" onclick={() => ((form.deputy_user_id = null), (deputyLabel = null))}>Clear</button>{/if}
			</div>
		</div>
		<div><label class="label" for="d-desc">Description</label><textarea id="d-desc" class="input" rows="3" bind:value={form.description}></textarea></div>
		<div class="w-32"><label class="label" for="d-sort">Sort order</label><input id="d-sort" type="number" class="input" bind:value={form.sort} /></div>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300" bind:checked={form.hidden} data-dept-hidden /> Hidden: only its own people (and administrators) see it</label>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (formOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="dept-form" disabled={saving}>{editing ? 'Save' : 'Create'}</button>
	{/snippet}
</Modal>
