<script lang="ts">
	// Knowledge base: spaces, and search over every page you can read.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import KbAccess from '#lib/components/KbAccess.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Space = { id: string; name: string; description: string | null; to_all: boolean; pages: number; can_edit: boolean; updated_at: string | null };
	let spaces = $state<Space[]>([]);
	let canCreate = $state(false);
	onMount(async () => {
		const r = await api.get<{ items: Space[]; can_create: boolean }>('/kb/spaces');
		spaces = r.items;
		canCreate = r.can_create;
	});

	let q = $state('');
	type Hit = { id: string; title: string; space_name: string; snippet: string };
	let hits = $state<Hit[] | null>(null);
	let t: ReturnType<typeof setTimeout> | undefined;
	function search() {
		clearTimeout(t);
		if (q.trim().length < 2) return void (hits = null);
		t = setTimeout(async () => (hits = (await api.get<{ items: Hit[] }>(`/kb/search?q=${encodeURIComponent(q.trim())}`)).items), 250);
	}

	let open = $state(false);
	let form = $state({ name: '', description: '', to_all: true });
	type Who = { users: { id: string; name: string; avatar_file_id: string | null }[]; departments: string[] };
	let readers = $state<Who>({ users: [], departments: [] });
	let editors = $state<Who>({ users: [], departments: [] });
	async function create(e: SubmitEvent) {
		e.preventDefault();
		try {
			const s = await api.post<{ id: string }>('/kb/spaces', {
				...form,
				readers: { users: readers.users.map((u) => u.id), departments: readers.departments },
				editors: { users: editors.users.map((u) => u.id), departments: editors.departments }
			});
			goto(`/kb/${s.id}`);
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>Knowledge base · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Knowledge base</h1>
		<input class="input w-72" type="search" placeholder="Search all pages…" aria-label="Search all pages" bind:value={q} oninput={search} />
		{#if canCreate}<button class="btn btn-primary" onclick={() => (open = true)}><Icon name="plus" class="size-4" /> New space</button>{/if}
	</div>

	{#if hits}
		<section class="card divide-y divide-slate-50" aria-label="Search results">
			{#each hits as h (h.id)}
				<a class="block px-5 py-3 hover:bg-slate-50" href="/kb/page/{h.id}" data-hit={h.title}>
					<p class="font-medium text-slate-900">{h.title} <span class="text-xs font-normal text-slate-400">{h.space_name}</span></p>
					<p class="truncate text-sm text-slate-500">{h.snippet}</p>
				</a>
			{:else}
				<p class="px-5 py-6 text-center text-sm text-slate-500">Nothing found.</p>
			{/each}
		</section>
	{/if}

	<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
		{#each spaces as s (s.id)}
			<a href="/kb/{s.id}" class="card block p-5 hover:shadow-g2" data-space={s.name}>
				<p class="font-semibold text-slate-900">{s.name}</p>
				{#if s.description}<p class="mt-1 line-clamp-2 text-sm text-slate-600">{s.description}</p>{/if}
				<p class="mt-3 text-xs text-slate-500">{s.pages} page{s.pages === 1 ? '' : 's'}{s.updated_at ? ` · updated ${formatRelative(s.updated_at)}` : ''}{s.to_all ? '' : ' · restricted'}</p>
			</a>
		{:else}
			<div class="card p-10 text-center text-slate-500 sm:col-span-2 lg:col-span-3">No spaces yet.{canCreate ? ' Make one for a handbook, a playbook or how-tos.' : ''}</div>
		{/each}
	</div>
</div>

<Modal bind:open title="New space">
	<form id="kb-new" class="space-y-3" onsubmit={create}>
		<div><label class="label" for="kb-name">Name</label><input id="kb-name" class="input" required maxlength="120" placeholder="Employee handbook" bind:value={form.name} /></div>
		<div><label class="label" for="kb-desc">What it's for</label><input id="kb-desc" class="input" bind:value={form.description} /></div>
		<KbAccess bind:toAll={form.to_all} bind:readers bind:editors />
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="kb-new">Create</button>
	{/snippet}
</Modal>
