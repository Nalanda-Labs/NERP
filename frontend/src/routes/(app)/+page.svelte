<script lang="ts">
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { PERM, type Department, type Page, type User } from '#lib/types.ts';
	import TaskTable from '#lib/components/TaskTable.svelte';
	import type { TaskSummary } from '#lib/tasks.ts';
	import { work } from '#lib/work.svelte.ts';

	let employees = $state<Page<User> | null>(null);
	let departments = $state<Department[]>([]);
	let colleagues = $state<User[]>([]);
	let myTasks = $state<TaskSummary[]>([]);

	const user = $derived(session.user!);
	const hour = new Date().getHours();
	const greeting = hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';

	onMount(async () => {
		// Guests go to their messenger (the layout sends them); the company view isn't theirs.
		if (session.user?.is_guest) return;
		api.get<Page<TaskSummary>>('/tasks' + query({ view: 'my', status: 'new,in_progress,review', sort: 'deadline', limit: 6 })).then((p) => (myTasks = p.items));
		departments = await api.get<Department[]>('/departments');
		if (session.can(PERM.USERS_VIEW)) {
			employees = await api.get<Page<User>>('/users' + query({ limit: 1 }));
			const myDept = user.departments[0];
			if (myDept) {
				const page = await api.get<Page<User>>('/users' + query({ department_id: myDept.id, limit: 8 }));
				colleagues = page.items.filter((u) => u.id !== user.id);
			}
		}
	});

	const myDepartments = $derived(departments.filter((d) => user.departments.some((x) => x.id === d.id)));
</script>

<svelte:head><title>Dashboard · NERP</title></svelte:head>

<div class="space-y-6">
	<div class="card flex flex-col gap-5 p-6 sm:flex-row sm:items-center">
		<Avatar name={user.full_name} fileId={user.avatar_file_id} size="lg" />
		<div class="flex-1">
			<h1 class="text-2xl font-semibold text-slate-900">{greeting}, {user.first_name}</h1>
			<p class="text-slate-500">
				{[user.position ?? 'Team member', ...user.departments.map((d) => d.name)].join(' · ')}
			</p>
		</div>
		<a href="/profile" class="btn btn-secondary"><Icon name="edit" class="size-4" /> Edit profile</a>
	</div>

	<div class="grid gap-4 sm:grid-cols-3">
		<a href="/employees" class="card group flex items-center gap-4 p-5">
			<span class="flex size-11 items-center justify-center rounded-lg bg-brand-50 text-brand-600"><Icon name="users" /></span>
			<span>
				<span class="block text-2xl font-semibold text-slate-900">{employees?.total ?? '—'}</span>
				<span class="text-sm text-slate-500">Active employees</span>
			</span>
		</a>
		<a href="/structure" class="card flex items-center gap-4 p-5">
			<span class="flex size-11 items-center justify-center rounded-lg bg-emerald-50 text-emerald-600"><Icon name="structure" /></span>
			<span>
				<span class="block text-2xl font-semibold text-slate-900">{departments.length}</span>
				<span class="text-sm text-slate-500">Departments</span>
			</span>
		</a>
		<a href="/tasks" class="card flex items-center gap-4 p-5">
			<span class="flex size-11 items-center justify-center rounded-lg bg-amber-50 text-amber-600"><Icon name="checkSquare" /></span>
			<span>
				<span class="block text-2xl font-semibold text-slate-900">{work.counters?.my_open ?? '—'}</span>
				<span class="text-sm text-slate-500">
					Open tasks{#if work.counters?.my_overdue}<span class="font-medium text-red-600"> · {work.counters.my_overdue} overdue</span>{/if}
				</span>
			</span>
		</a>
	</div>

	<section class="space-y-2">
		<div class="flex items-center justify-between">
			<h2 class="font-semibold text-slate-900">My tasks</h2>
			<a href="/tasks" class="text-sm font-medium text-brand-600 hover:underline">All tasks</a>
		</div>
		<TaskTable tasks={myTasks} empty="No open tasks. Nice work!" />
	</section>

	<div class="grid gap-6 lg:grid-cols-2">
		<section class="card">
			<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">My departments</h2>
			{#if myDepartments.length}
				<ul class="divide-y divide-slate-100">
					{#each myDepartments as d (d.id)}
						<li class="flex items-center justify-between px-5 py-3">
							<div>
								<p class="font-medium text-slate-800">{d.name}</p>
								<p class="text-sm text-slate-500">Head: {d.head_name ?? 'not assigned'}</p>
							</div>
							<span class="badge bg-slate-100 text-slate-600">{d.member_count} {d.member_count === 1 ? 'member' : 'members'}</span>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="px-5 py-6 text-sm text-slate-500">You are not in any department yet.</p>
			{/if}
		</section>

		<section class="card">
			<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">Colleagues</h2>
			{#if colleagues.length}
				<ul class="grid grid-cols-1 gap-1 p-2 sm:grid-cols-2">
					{#each colleagues as c (c.id)}
						<li>
							<a href="/employees/{c.id}" class="flex items-center gap-3 rounded-lg p-2.5 hover:bg-slate-50">
								<Avatar name={c.full_name} fileId={c.avatar_file_id} size="sm" />
								<span class="min-w-0">
									<span class="block truncate text-sm font-medium text-slate-800">{c.full_name}</span>
									<span class="block truncate text-xs text-slate-500">{c.position ?? c.email}</span>
								</span>
							</a>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="px-5 py-6 text-sm text-slate-500">No colleagues in your department yet.</p>
			{/if}
		</section>
	</div>
</div>
