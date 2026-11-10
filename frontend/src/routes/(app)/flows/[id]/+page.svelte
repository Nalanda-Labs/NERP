<script lang="ts">
	// One flow: send it a task; its team sees what waits, what is at work and what is done.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { DISTRIBUTION, type Flow } from '#lib/flows.ts';
	import { formatRelative } from '#lib/format.ts';
	import { formatDeadline, type TaskDetail, type TaskSummary } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	const id = $derived(page.params.id!);
	let flow = $state<Flow | null>(null);
	type FlowTask = TaskSummary & { flow_waiting: boolean };
	let columns = $state<Record<'waiting' | 'at_work' | 'done', FlowTask[]>>({ waiting: [], at_work: [], done: [] });
	let insider = $state(true);

	async function load() {
		flow = await api.get<Flow>(`/flows/${id}`);
		try {
			const [w, a, d] = await Promise.all((['waiting', 'at_work', 'done'] as const).map((s) => api.get<{ items: FlowTask[] }>(`/flows/${id}/tasks` + query({ state: s }))));
			columns = { waiting: w!.items, at_work: a!.items, done: d!.items };
			insider = true;
		} catch {
			insider = false;
		}
	}
	$effect(() => {
		id;
		load();
	});

	let sendOpen = $state(false);
	let draft = $state({ title: '', description: '' });
	async function send(e: SubmitEvent) {
		e.preventDefault();
		try {
			const t = await api.post<TaskDetail>(`/flows/${id}/tasks`, { title: draft.title, description: draft.description || null });
			sendOpen = false;
			draft = { title: '', description: '' };
			toasts.success(`Task #${t.number} sent${t.responsible_name ? ` — ${t.responsible_name}` : ''}`);
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function take(t: FlowTask) {
		try {
			await api.post(`/tasks/${t.id}/take`);
			goto(`/tasks/${t.id}`);
		} catch (err) {
			toasts.error(err);
		}
	}

	let editOpen = $state(false);
	let form = $state({ name: '', description: '', distribution: 'queue', planned_hours: '', is_active: true });
	let team = $state<PickedUser[]>([]);
	function edit() {
		const f = flow!;
		form = { name: f.name, description: f.description ?? '', distribution: f.distribution, planned_hours: f.planned_hours ? String(f.planned_hours) : '', is_active: f.is_active };
		team = (f.members ?? []).map((m) => ({ id: m.id, name: m.full_name, avatar_file_id: m.avatar_file_id }));
		editOpen = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			flow = await api.patch<Flow>(`/flows/${id}`, { ...form, description: form.description || null, planned_hours: form.planned_hours ? Number(form.planned_hours) : null, team: team.map((t) => t.id) });
			editOpen = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove() {
		if (!confirm('Delete this flow? Its tasks stay in the group.')) return;
		await api.del(`/flows/${id}`);
		goto('/flows');
	}
	const hours = (h: number | null) => (h == null ? '—' : h < 48 ? `${h} h` : `${(h / 24).toFixed(1)} d`);
</script>

<svelte:head><title>{flow?.name ?? 'Flow'} · NERP</title></svelte:head>

{#if flow}
	<div class="space-y-5">
		<a href="/flows" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Flows</a>
		<div class="flex flex-wrap items-start gap-3">
			<div class="min-w-0 flex-1">
				<h1 class="text-2xl font-semibold text-slate-900">{flow.name}{#if !flow.is_active}<span class="badge ml-2 bg-slate-100 align-middle text-slate-500">Paused</span>{/if}</h1>
				<p class="text-sm text-slate-500"><a class="hover:underline" href="/projects/{flow.project_id}">{flow.project_name}</a> · {DISTRIBUTION[flow.distribution].label} · run by {flow.owner_name}{flow.planned_hours ? ` · ${flow.planned_hours} h planned per task` : ''}</p>
				{#if flow.description}<p class="mt-2 max-w-3xl text-sm text-slate-600">{flow.description}</p>{/if}
			</div>
			{#if flow.can_manage}<button class="btn btn-secondary" onclick={edit}><Icon name="settings" class="size-4" /> Settings</button>{/if}
			{#if flow.is_active}<button class="btn btn-primary" onclick={() => (sendOpen = true)}><Icon name="plus" class="size-4" /> Send a task</button>{/if}
		</div>

		<div class="grid gap-3 sm:grid-cols-5" aria-label="Numbers">
			{#each [['Waiting', String(flow.stats.waiting)], ['At work', String(flow.stats.at_work)], ['Done, 30 days', String(flow.stats.done)], ['Avg. wait before start', hours(flow.stats.avg_wait_hours)], ['Efficiency', flow.stats.efficiency == null ? '—' : `${flow.stats.efficiency}%`]] as [l, v] (l)}
				<div class="card px-4 py-3" data-stat={l}><p class="text-xs text-slate-500">{l}</p><p class="text-2xl font-semibold text-slate-900">{v}</p></div>
			{/each}
		</div>

		{#if insider}
			<div class="grid gap-4 lg:grid-cols-[1fr_1fr_1fr_16rem]">
				{#each [['waiting', 'Waiting'], ['at_work', 'At work'], ['done', 'Done']] as [k, label] (k)}
					<section class="rounded-2xl bg-surface-container p-2" aria-label={label}>
						<h2 class="px-2 py-1.5 text-sm font-semibold text-slate-700">{label} <span class="font-normal text-slate-400">{columns[k as 'waiting'].length}</span></h2>
						<ul class="space-y-2">
							{#each columns[k as 'waiting'] as t (t.id)}
								<li class="rounded-xl bg-white p-3 shadow-g1" data-flow-task={t.title}>
									<a href="/tasks/{t.id}" class="block text-sm font-medium text-slate-800 hover:text-brand-700">{t.title}</a>
									<p class="mt-1 text-xs text-slate-500">{t.flow_waiting ? 'Not handed out yet' : t.responsible_name} · {k === 'done' ? `done ${formatRelative(t.completed_at!)}` : t.deadline ? formatDeadline(t.deadline) : `came in ${formatRelative(t.created_at)}`}</p>
									{#if t.flow_waiting && (flow.in_team || flow.can_manage) && flow.distribution === 'self'}
										<button class="btn btn-secondary mt-2 py-1 text-xs" onclick={() => take(t)}>Take it</button>
									{/if}
								</li>
							{:else}
								<li class="px-2 py-4 text-center text-xs text-slate-400">Nothing.</li>
							{/each}
						</ul>
					</section>
				{/each}
				<aside class="card self-start" aria-label="Team">
					<h2 class="border-b border-slate-100 px-4 py-2.5 text-sm font-semibold text-slate-800">Team</h2>
					<ul class="divide-y divide-slate-100">
						{#each flow.members ?? [] as m (m.id)}
							<li class="flex items-center gap-2 px-4 py-2 text-sm" data-member={m.full_name}><Avatar name={m.full_name} fileId={m.avatar_file_id} size="xs" /><span class="flex-1">{m.full_name}</span><span class="text-xs text-slate-500">{m.open} open</span></li>
						{/each}
					</ul>
				</aside>
			</div>
		{:else}
			<p class="card p-6 text-sm text-slate-600">Send this flow a task and its team takes it from there. You'll see it among the tasks you created.</p>
		{/if}
	</div>

	<Modal bind:open={sendOpen} title="Send a task to {flow.name}">
		<form id="flow-send" class="space-y-3" onsubmit={send}>
			<div><label class="label" for="fs-t">What's needed</label><input id="fs-t" class="input" required maxlength="500" bind:value={draft.title} /></div>
			<div><label class="label" for="fs-d">Details</label><textarea id="fs-d" class="input" rows="4" bind:value={draft.description}></textarea></div>
		</form>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (sendOpen = false)}>Cancel</button>
			<button class="btn btn-primary" form="flow-send">Send</button>
		{/snippet}
	</Modal>

	<Modal bind:open={editOpen} title="Flow settings">
		<form id="flow-edit" class="space-y-3" onsubmit={save}>
			<div><label class="label" for="fe-n">Name</label><input id="fe-n" class="input" required bind:value={form.name} /></div>
			<div><label class="label" for="fe-d">What it is for</label><textarea id="fe-d" class="input" rows="2" bind:value={form.description}></textarea></div>
			<div>
				<label class="label" for="fe-dist">Handing tasks out</label>
				<select id="fe-dist" class="input" bind:value={form.distribution}>{#each Object.entries(DISTRIBUTION) as [k, d] (k)}<option value={k}>{d.label} — {d.hint}</option>{/each}</select>
			</div>
			<div><span class="label">Team</span><MultiUserPicker bind:value={team} /></div>
			<div><label class="label" for="fe-h">Planned time per task (hours)</label><input id="fe-h" type="number" min="0.25" step="0.25" class="input w-40" bind:value={form.planned_hours} /></div>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.is_active} /> Taking tasks</label>
			<button type="button" class="text-sm text-red-600 hover:underline" onclick={remove}>Delete flow</button>
		</form>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (editOpen = false)}>Cancel</button>
			<button class="btn btn-primary" form="flow-edit">Save</button>
		{/snippet}
	</Modal>
{:else}
	<div class="card p-10 text-center text-slate-400">Loading…</div>
{/if}
