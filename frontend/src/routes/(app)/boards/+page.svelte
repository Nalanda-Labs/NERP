<script lang="ts">
	// Whiteboards: mine, shared with me, and templates; a new board blank or from a template.
	import { goto } from '$app/navigation';
	import { api, query } from '#lib/api.ts';
	import { TEMPLATES } from '#lib/boardTemplates.ts';
	import { loadExcalidraw } from '#lib/excalidraw.ts';
	import { formatRelative } from '#lib/format.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type Board = { id: string; title: string; updated_at: string; level: string; is_template: boolean; owner: { name: string }; task: { number: number; title: string } | null; project: { name: string } | null };
	let scope = $state<'mine' | 'shared' | 'templates'>('mine');
	let q = $state('');
	let items = $state<Board[] | null>(null);
	let newOpen = $state(false);
	let name = $state('');
	let template = $state('');
	let busy = $state(false);
	let companyTemplates = $state<Board[]>([]);

	async function load() {
		try {
			items = (await api.get<{ items: Board[] }>('/boards' + query({ scope, q: q || undefined }))).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		scope;
		q;
		load();
	});
	async function openNew() {
		name = '';
		template = '';
		newOpen = true;
		companyTemplates = (await api.get<{ items: Board[] }>('/boards?scope=templates')).items;
	}
	async function create(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const body: Record<string, unknown> = { title: name };
			const builtIn = TEMPLATES.find((t) => t.id === template);
			if (builtIn) {
				const lib = await loadExcalidraw();
				body.scene = { elements: lib.convertToExcalidrawElements(builtIn.build() as never), appState: {} };
			} else if (template) body.from = template;
			const b = await api.post<{ id: string }>('/boards', body);
			goto(`/boards/${b.id}`);
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Boards · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="mr-2 text-2xl font-semibold text-slate-900">Boards</h1>
		<div class="flex rounded-full bg-slate-100 p-0.5" role="group" aria-label="Which boards">
			{#each [['mine', 'Mine'], ['shared', 'Shared with me'], ['templates', 'Templates']] as [v, l] (v)}
				<button class="rounded-full px-3 py-1 text-sm font-medium {scope === v ? 'bg-white shadow-sm' : 'text-slate-600'}" aria-pressed={scope === v} onclick={() => (scope = v as typeof scope)}>{l}</button>
			{/each}
		</div>
		<input class="input ml-auto w-56" type="search" placeholder="Find a board…" aria-label="Find a board" bind:value={q} />
		<button class="btn btn-primary" onclick={openNew}><Icon name="plus" class="size-4" /> New board</button>
	</div>
	{#if items === null}
		<p class="text-sm text-slate-400">Loading…</p>
	{:else if items.length}
		<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
			{#each items as b (b.id)}
				<a class="card block p-4 hover:border-brand-300" href="/boards/{b.id}" data-board={b.title}>
					<p class="font-medium text-slate-900">{b.title}</p>
					<p class="mt-1 text-xs text-slate-500">{b.owner.name} · changed {formatRelative(b.updated_at)}{b.level === 'view' ? ' · view only' : ''}</p>
					{#if b.task}<p class="mt-1 text-xs text-slate-500">Task #{b.task.number} {b.task.title}</p>{/if}
					{#if b.project}<p class="mt-1 text-xs text-slate-500">{b.project.name}</p>{/if}
				</a>
			{/each}
		</div>
	{:else}
		<p class="text-sm text-slate-500">{scope === 'templates' ? 'No company templates yet: open a board and mark it as a template.' : 'No boards here yet.'}</p>
	{/if}
</div>

<Modal bind:open={newOpen} title="New board">
	<form id="nb-form" class="space-y-3 text-sm" onsubmit={create}>
		<div><label class="label" for="nb-title">Name</label><input id="nb-title" class="input" required maxlength="200" bind:value={name} /></div>
		<fieldset>
			<legend class="label">Start from</legend>
			<div class="grid gap-2 sm:grid-cols-2">
				<label class="flex items-start gap-2 rounded-lg border border-slate-200 p-2"><input type="radio" name="tpl" value="" bind:group={template} /> <span><span class="font-medium">Blank</span></span></label>
				{#each TEMPLATES as t (t.id)}
					<label class="flex items-start gap-2 rounded-lg border border-slate-200 p-2"><input type="radio" name="tpl" value={t.id} bind:group={template} /> <span><span class="font-medium">{t.name}</span><br /><span class="text-xs text-slate-500">{t.description}</span></span></label>
				{/each}
				{#each companyTemplates as t (t.id)}
					<label class="flex items-start gap-2 rounded-lg border border-slate-200 p-2"><input type="radio" name="tpl" value={t.id} bind:group={template} /> <span><span class="font-medium">{t.title}</span><br /><span class="text-xs text-slate-500">Company template</span></span></label>
				{/each}
			</div>
		</fieldset>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (newOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="nb-form" disabled={busy}>Create</button>
	{/snippet}
</Modal>
