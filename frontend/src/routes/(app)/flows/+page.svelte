<script lang="ts">
	// Flows: queues of similar tasks that teams work through. Anyone sends them tasks.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { DISTRIBUTION, type Flow } from '#lib/flows.ts';
	import type { ProjectSummary } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	const projectId = $derived(page.url.searchParams.get('project_id'));
	let flows = $state<Flow[]>([]);
	$effect(() => {
		api.get<{ items: Flow[] }>('/flows' + query({ project_id: projectId ?? undefined })).then((r) => (flows = r.items));
	});
	let open = $state(false);
	let groups = $state<ProjectSummary[]>([]);
	let form = $state({ project_id: '', name: '', description: '', distribution: 'queue', planned_hours: '' });
	let team = $state<PickedUser[]>([]);
	async function start() {
		groups = (await api.get<ProjectSummary[]>('/projects?mine=true')).filter((g) => g.my_role === 'owner');
		form = { project_id: projectId ?? groups[0]?.id ?? '', name: '', description: '', distribution: 'queue', planned_hours: '' };
		team = [];
		open = true;
	}
	async function create(e: SubmitEvent) {
		e.preventDefault();
		try {
			const f = await api.post<Flow>('/flows', { ...form, description: form.description || null, planned_hours: form.planned_hours ? Number(form.planned_hours) : null, team: team.map((t) => t.id) });
			open = false;
			goto(`/flows/${f.id}`);
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>Flows · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<div class="flex-1">
			<h1 class="text-2xl font-semibold text-slate-900">Flows</h1>
			<p class="text-sm text-slate-500">Queues of similar tasks a team works through. Send a task to a flow and it reaches the right person.</p>
		</div>
		<button class="btn btn-primary" onclick={start}><Icon name="plus" class="size-4" /> New flow</button>
	</div>
	<div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
		{#each flows as f (f.id)}
			<a href="/flows/{f.id}" class="card block p-5 {f.is_active ? '' : 'opacity-60'}" data-flow={f.name}>
				<div class="flex items-start gap-2">
					<h2 class="flex-1 font-semibold text-slate-900">{f.name}</h2>
					{#if !f.is_active}<span class="badge bg-slate-100 text-slate-500">Paused</span>{/if}
				</div>
				<p class="text-xs text-slate-500">{f.project_name} · {DISTRIBUTION[f.distribution].label}{f.planned_hours ? ` · ${f.planned_hours} h planned` : ''}</p>
				{#if f.description}<p class="mt-2 line-clamp-2 text-sm text-slate-600">{f.description}</p>{/if}
				<div class="mt-4 grid grid-cols-4 gap-2 text-center text-xs">
					<div><p class="text-lg font-semibold text-slate-900">{f.stats.waiting}</p><p class="text-slate-500">Waiting</p></div>
					<div><p class="text-lg font-semibold text-slate-900">{f.stats.at_work}</p><p class="text-slate-500">At work</p></div>
					<div><p class="text-lg font-semibold text-slate-900">{f.stats.done}</p><p class="text-slate-500">Done (30 d)</p></div>
					<div><p class="text-lg font-semibold {f.stats.efficiency == null ? 'text-slate-400' : f.stats.efficiency >= 80 ? 'text-emerald-700' : 'text-amber-700'}">{f.stats.efficiency == null ? '—' : `${f.stats.efficiency}%`}</p><p class="text-slate-500">Efficiency</p></div>
				</div>
			</a>
		{:else}
			<div class="card p-10 text-center text-sm text-slate-500 sm:col-span-2 xl:col-span-3">No flows yet.</div>
		{/each}
	</div>
</div>

<Modal bind:open title="New flow">
	<form id="flow-form" class="space-y-3" onsubmit={create}>
		<div>
			<label class="label" for="fl-g">Workgroup or project</label>
			<select id="fl-g" class="input" required bind:value={form.project_id}>{#each groups as g (g.id)}<option value={g.id}>{g.name}</option>{/each}</select>
		</div>
		<div><label class="label" for="fl-n">Name</label><input id="fl-n" class="input" required maxlength="200" bind:value={form.name} /></div>
		<div><label class="label" for="fl-d">What it is for</label><textarea id="fl-d" class="input" rows="2" bind:value={form.description}></textarea></div>
		<div>
			<label class="label" for="fl-dist">Handing tasks out</label>
			<select id="fl-dist" class="input" bind:value={form.distribution}>{#each Object.entries(DISTRIBUTION) as [k, d] (k)}<option value={k}>{d.label} — {d.hint}</option>{/each}</select>
		</div>
		<div><span class="label">Team</span><MultiUserPicker bind:value={team} placeholder="Who works on it…" /></div>
		<div><label class="label" for="fl-h">Planned time per task (hours)</label><input id="fl-h" type="number" min="0.25" step="0.25" class="input w-40" bind:value={form.planned_hours} /></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="flow-form">Create</button>
	{/snippet}
</Modal>
