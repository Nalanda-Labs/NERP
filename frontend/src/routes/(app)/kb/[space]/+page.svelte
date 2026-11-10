<script lang="ts">
	// A knowledge base space: its pages, a new page, and (for its owners) who reads and edits it.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import KbAccess from '#lib/components/KbAccess.svelte';
	import KbTree from '#lib/components/KbTree.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type Who = { users: { id: string; name: string; avatar_file_id: string | null }[]; departments: string[] };
	type Space = { id: string; name: string; description: string | null; to_all: boolean; can_edit: boolean; can_manage?: boolean; pages: { id: string; parent_id: string | null; title: string }[]; readers?: Who; editors?: Who };
	let s = $state<Space | null>(null);
	let missing = $state(false);
	$effect(() => {
		api.get<Space>(`/kb/spaces/${page.params.space}`).then((x) => (s = x)).catch(() => (missing = true));
	});

	async function newPage() {
		const title = prompt('Title of the new page');
		if (!title?.trim()) return;
		try {
			const p = await api.post<{ id: string }>(`/kb/spaces/${s!.id}/pages`, { title, body: '' });
			goto(`/kb/page/${p.id}?edit=1`);
		} catch (e) {
			toasts.error(e);
		}
	}

	let settings = $state(false);
	let form = $state({ name: '', description: '', to_all: true });
	let readers = $state<Who>({ users: [], departments: [] });
	let editors = $state<Who>({ users: [], departments: [] });
	function openSettings() {
		form = { name: s!.name, description: s!.description ?? '', to_all: s!.to_all };
		readers = { users: s!.readers?.users.map((u) => ({ ...u, avatar_file_id: null })) ?? [], departments: s!.readers?.departments ?? [] };
		editors = { users: s!.editors?.users.map((u) => ({ ...u, avatar_file_id: null })) ?? [], departments: s!.editors?.departments ?? [] };
		settings = true;
	}
	async function saveSettings(e: SubmitEvent) {
		e.preventDefault();
		try {
			s = await api.patch<Space>(`/kb/spaces/${s!.id}`, {
				...form,
				readers: { users: readers.users.map((u) => u.id), departments: readers.departments },
				editors: { users: editors.users.map((u) => u.id), departments: editors.departments }
			});
			settings = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove() {
		if (!confirm(`Delete the space “${s!.name}” and all its pages?`)) return;
		await api.del(`/kb/spaces/${s!.id}`);
		goto('/kb');
	}
</script>

<svelte:head><title>{s?.name ?? 'Knowledge base'} · NERP</title></svelte:head>

<a href="/kb" class="mb-4 inline-block text-sm text-slate-500 hover:text-slate-800">← Knowledge base</a>
{#if missing}
	<div class="card p-10 text-center text-slate-500">This space doesn't exist, or you can't read it.</div>
{:else if s}
	<div class="flex flex-wrap items-start gap-3">
		<div class="min-w-0 flex-1">
			<h1 class="text-2xl font-semibold text-slate-900">{s.name}</h1>
			{#if s.description}<p class="mt-1 text-slate-600">{s.description}</p>{/if}
		</div>
		{#if s.can_manage}<button class="btn btn-secondary" onclick={openSettings}>Settings</button>{/if}
		{#if s.can_edit}<button class="btn btn-primary" onclick={newPage}><Icon name="plus" class="size-4" /> New page</button>{/if}
	</div>
	<div class="card mt-5 max-w-3xl p-4">
		{#if s.pages.length}<KbTree pages={s.pages} />{:else}<p class="p-6 text-center text-sm text-slate-500">No pages yet.</p>{/if}
	</div>
{/if}

<Modal bind:open={settings} title="Space settings">
	<form id="kb-set" class="space-y-3" onsubmit={saveSettings}>
		<div><label class="label" for="ks-name">Name</label><input id="ks-name" class="input" required maxlength="120" bind:value={form.name} /></div>
		<div><label class="label" for="ks-desc">What it's for</label><input id="ks-desc" class="input" bind:value={form.description} /></div>
		<KbAccess bind:toAll={form.to_all} bind:readers bind:editors />
	</form>
	{#snippet footer()}
		<button class="btn btn-ghost mr-auto hover:text-red-600" onclick={remove}>Delete space</button>
		<button class="btn btn-secondary" onclick={() => (settings = false)}>Cancel</button>
		<button class="btn btn-primary" form="kb-set">Save</button>
	{/snippet}
</Modal>
