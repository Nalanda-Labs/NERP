<script lang="ts">
	// Cross-functional teams: people from any department under a lead, beside the departments.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';

	type Team = { id: string; name: string; description: string | null; color: string | null; lead: { id: string; name: string; avatar_file_id: string | null } | null; members: number; mine: boolean };
	type Member = { id: string; name: string; avatar_file_id: string | null; position: string | null; role: string | null; departments: string | null; lead: boolean };
	type Detail = { id: string; name: string; description: string | null; color: string | null; lead_id: string | null; members: Member[]; chat_id: string | null; in_team: boolean; can_edit: boolean; can_edit_members: boolean };
	let teams = $state<Team[]>([]);
	let canManage = $state(false);
	let open = $state<Detail | null>(null);
	let form = $state<{ name: string; description: string; color: string; lead: { id: string; name: string } | null } | null>(null);

	async function load() {
		try {
			const r = await api.get<{ items: Team[]; can_manage: boolean }>('/teams');
			teams = r.items;
			canManage = r.can_manage;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	async function show(id: string) {
		try {
			open = await api.get<Detail>(`/teams/${id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function create(e: SubmitEvent) {
		e.preventDefault();
		if (!form) return;
		try {
			const t = await api.post<Detail>('/teams', { name: form.name, description: form.description, color: form.color || null, lead_id: form.lead?.id ?? null, members: [] });
			form = null;
			await load();
			open = t;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function saveMembers(members: { user_id: string; role: string | null }[]) {
		if (!open) return;
		try {
			open = await api.put<Detail>(`/teams/${open.id}/members`, { members });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	const current = () => open!.members.filter((m) => !m.lead).map((m) => ({ user_id: m.id, role: m.role }));
	async function remove() {
		if (!open || !confirm(`Remove the team “${open.name}”? Its chat goes too.`)) return;
		try {
			await api.del(`/teams/${open.id}`);
			open = null;
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function openChat() {
		if (!open) return;
		try {
			const r = await api.post<{ chat_id: string }>(`/teams/${open.id}/chat`, {});
			goto(`/messenger?chat=${r.chat_id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Teams · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Teams</h1>
			<p class="text-sm text-slate-500">Cross-functional teams: people from any department working together, beside the <a class="text-brand-700 hover:underline" href="/structure">company structure</a>.</p>
		</div>
		{#if canManage}<button class="btn btn-primary ml-auto" onclick={() => (form = { name: '', description: '', color: '', lead: null })} data-new-team>New team</button>{/if}
	</div>

	{#if form}
		<form class="card grid gap-3 p-5 text-sm sm:grid-cols-2" onsubmit={create} aria-label="New team">
			<label class="block">Name<input class="input mt-1" required bind:value={form.name} name="name" /></label>
			<div><span class="block">Lead</span><div class="mt-1">{#if form.lead}<span class="badge bg-slate-100">{form.lead.name} <button type="button" onclick={() => (form!.lead = null)}>×</button></span>{:else}<UserPicker placeholder="Choose the lead…" onpick={(u) => (form!.lead = { id: u.id, name: u.full_name })} />{/if}</div></div>
			<label class="block sm:col-span-2">What the team does<textarea class="input mt-1" rows="2" bind:value={form.description}></textarea></label>
			<div class="flex gap-2 sm:col-span-2"><button class="btn btn-primary">Create the team</button><button type="button" class="btn btn-ghost" onclick={() => (form = null)}>Cancel</button></div>
		</form>
	{/if}

	<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
		{#each teams as t (t.id)}
			<button class="card p-4 text-left text-sm hover:ring-2 hover:ring-brand-200 {open?.id === t.id ? 'ring-2 ring-brand-400' : ''}" onclick={() => show(t.id)} data-team={t.name}>
				<p class="flex items-center gap-2 font-semibold text-slate-900"><span class="size-2.5 rounded-full" style="background: {t.color ?? '#94a3b8'}"></span>{t.name}{#if t.mine}<span class="badge bg-brand-50 text-brand-700">yours</span>{/if}</p>
				{#if t.description}<p class="mt-1 line-clamp-2 text-slate-600">{t.description}</p>{/if}
				<p class="mt-2 text-xs text-slate-500">{t.members} {t.members === 1 ? 'person' : 'people'}{t.lead ? ` · led by ${t.lead.name}` : ''}</p>
			</button>
		{:else}<p class="text-sm text-slate-500">No teams yet.</p>{/each}
	</div>

	{#if open}
		<section class="card space-y-3 p-5 text-sm" aria-label="Team">
			<div class="flex flex-wrap items-center gap-2">
				<h2 class="text-lg font-semibold text-slate-900">{open.name}</h2>
				<div class="ml-auto flex gap-2">
					{#if open.in_team}<button class="btn btn-secondary" onclick={openChat} data-team-chat>Team chat</button>{/if}
					{#if open.can_edit}<button class="btn btn-ghost text-red-600" onclick={remove}>Remove</button>{/if}
				</div>
			</div>
			{#if open.description}<p class="text-slate-600">{open.description}</p>{/if}
			<ul class="divide-y divide-slate-100" data-members>
				{#each open.members as m (m.id)}
					<li class="flex items-center gap-3 py-2" data-member={m.name}>
						<Avatar name={m.name} fileId={m.avatar_file_id} size="sm" />
						<div class="min-w-0 flex-1">
							<a class="font-medium text-slate-900 hover:underline" href="/employees/{m.id}">{m.name}</a>{#if m.lead}<span class="ml-1 badge bg-amber-50 text-amber-800">Lead</span>{/if}
							<p class="text-xs text-slate-500">{[m.role && !m.lead ? m.role : null, m.departments, m.position].filter(Boolean).join(' · ')}</p>
						</div>
						{#if open.can_edit_members && !m.lead}
							<input class="input w-36 text-xs" placeholder="Role in the team" value={m.role ?? ''} aria-label="Role of {m.name}" onchange={(e) => saveMembers(current().map((x) => (x.user_id === m.id ? { ...x, role: (e.currentTarget as HTMLInputElement).value } : x)))} />
							<button class="text-slate-400 hover:text-red-600" aria-label="Remove {m.name}" onclick={() => saveMembers(current().filter((x) => x.user_id !== m.id))}>×</button>
						{/if}
					</li>
				{/each}
			</ul>
			{#if open.can_edit_members}<div class="w-72"><UserPicker placeholder="+ Add someone from any department…" exclude={open.members.map((m) => m.id)} onpick={(u) => saveMembers([...current(), { user_id: u.id, role: null }])} /></div>{/if}
		</section>
	{/if}
</div>
