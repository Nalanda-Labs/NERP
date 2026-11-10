<script lang="ts">
	// Lets project owners add, rename, recolour, reorder, link and delete board columns.
	import { api } from '#lib/api.ts';
	import { STAGE_COLORS, STATUS, type Stage } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from './Icon.svelte';
	import Modal from './Modal.svelte';

	let {
		open = $bindable(false),
		projectId,
		stages = $bindable<Stage[]>([])
	}: { open?: boolean; projectId: string; stages?: Stage[] } = $props();

	let newName = $state('');
	const base = $derived(`/projects/${projectId}/stages`);

	async function run(p: Promise<Stage[]>) {
		try {
			stages = await p;
		} catch (e) {
			toasts.error(e);
		}
	}

	const patch = (s: Stage, body: Record<string, unknown>) => run(api.patch<Stage[]>(`${base}/${s.id}`, body));

	function swap(i: number, j: number) {
		const a = stages[i]!;
		const b = stages[j]!;
		// Exchange sort keys (nudging if they happen to be equal).
		const sa = a.sort;
		const sb = b.sort === a.sort ? a.sort + (j > i ? 1 : -1) : b.sort;
		run(api.patch<Stage[]>(`${base}/${a.id}`, { sort: sb }).then(() => api.patch<Stage[]>(`${base}/${b.id}`, { sort: sa })));
	}

	async function add(e: SubmitEvent) {
		e.preventDefault();
		if (!newName.trim()) return;
		await run(api.post<Stage[]>(base, { name: newName, color: 'slate' }));
		newName = '';
	}

	async function remove(s: Stage) {
		const target = stages.find((x) => x.id !== s.id);
		if (!target || !confirm(`Delete the column "${s.name}"? Its cards move to "${target.name}".`)) return;
		await run(api.del<Stage[]>(`${base}/${s.id}?move_to=${target.id}`));
	}
</script>

<Modal bind:open title="Board columns" size="lg">
	<p class="mb-4 text-sm text-slate-600">
		Linking a column to a status keeps the two in step: dropping a card there sets the status, and a task that changes status
		elsewhere moves into the matching column.
	</p>
	<ul class="space-y-2">
		{#each stages as s, i (s.id)}
			<li class="flex flex-wrap items-center gap-2 rounded-lg border border-slate-200 p-2">
				<div class="flex flex-col">
					<button class="text-slate-400 hover:text-slate-800 disabled:opacity-30" disabled={i === 0} aria-label="Move left" onclick={() => swap(i, i - 1)}>
						<Icon name="chevronDown" class="size-4 rotate-180" />
					</button>
					<button class="text-slate-400 hover:text-slate-800 disabled:opacity-30" disabled={i === stages.length - 1} aria-label="Move right" onclick={() => swap(i, i + 1)}>
						<Icon name="chevronDown" class="size-4" />
					</button>
				</div>
				<input class="input min-w-36 flex-1" value={s.name} aria-label="Column name" onchange={(e) => patch(s, { name: e.currentTarget.value })} />
				<div class="flex gap-1">
					{#each Object.entries(STAGE_COLORS) as [name, cls] (name)}
						<button aria-label={name} class="size-5 rounded-full {cls} {s.color === name ? 'ring-2 ring-slate-500 ring-offset-1' : ''}" onclick={() => patch(s, { color: name })}></button>
					{/each}
				</div>
				<select class="input w-44" aria-label="Linked status" value={s.status ?? ''} onchange={(e) => patch(s, { status: e.currentTarget.value || null })}>
					<option value="">No linked status</option>
					{#each Object.entries(STATUS) as [k, v] (k)}<option value={k}>→ {v.label}</option>{/each}
				</select>
				<button class="btn btn-ghost p-1.5 text-slate-400 hover:text-red-600" aria-label="Delete column" disabled={stages.length === 1} onclick={() => remove(s)}>
					<Icon name="trash" class="size-4" />
				</button>
			</li>
		{/each}
	</ul>
	<form class="mt-4 flex gap-2" onsubmit={add}>
		<input class="input" placeholder="New column name" maxlength="60" bind:value={newName} />
		<button class="btn btn-secondary shrink-0" disabled={!newName.trim()}><Icon name="plus" class="size-4" /> Add column</button>
	</form>
	{#snippet footer()}
		<button class="btn btn-primary" onclick={() => (open = false)}>Done</button>
	{/snippet}
</Modal>
