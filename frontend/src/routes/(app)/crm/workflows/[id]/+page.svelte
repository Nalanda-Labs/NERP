<script lang="ts">
	// Designing a CRM business process: when it starts (a record is made, changes, or someone
	// starts it), and its steps — as a flowchart or step by step.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import WfFlow from '#lib/components/WfFlow.svelte';
	import WfSteps, { type ListField, type Step } from '#lib/components/WfSteps.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type T = { id: string; crm_type: string; fields: ListField[]; name: string; trigger: string; steps: Step[]; is_active: boolean; test_users: string[] };
	let t = $state<T | null>(null);
	let testers = $state<PickedUser[]>([]);
	let people = $state<Record<string, { id: string; name: string; avatar_file_id: string | null }>>({});
	let view = $state<'flow' | 'steps'>('flow');
	$effect(() => {
		api.get<T>(`/workflows/${page.params.id}`).then((x) => {
			t = x;
			const ids = new Set<string>();
			const walk = (s: Step[]) => s.forEach((st) => { st.who?.users.forEach((u) => ids.add(u)); [st.approved, st.rejected, st.then, st.else].forEach((b) => b && walk(b)); });
			walk(x.steps);
			testers = [];
			for (const uid of x.test_users ?? []) api.get<{ id: string; full_name: string; avatar_file_id: string | null }>(`/users/${uid}`).then((u) => testers.push({ id: u.id, name: u.full_name, avatar_file_id: u.avatar_file_id })).catch(() => {});
			for (const uid of ids) api.get<{ id: string; full_name: string; avatar_file_id: string | null }>(`/users/${uid}`).then((u) => (people[uid] = { id: u.id, name: u.full_name, avatar_file_id: u.avatar_file_id })).catch(() => {});
		}).catch((e) => toasts.error(e));
	});
	async function save() {
		try {
			const x = await api.patch<T>(`/workflows/${t!.id}`, { name: t!.name, trigger: t!.trigger, steps: t!.steps, is_active: t!.is_active, test_users: testers.map((u) => u.id) });
			t = { ...t!, ...x };
			toasts.success('Saved');
		} catch (e) {
			toasts.error(e);
		}
	}
	type Entry = { at: string; text: string; user: string | null; item_title: string; number: number; item_id: string };
	let entries = $state<Entry[] | null>(null);
	async function loadLog() {
		entries = (await api.get<{ items: Entry[] }>(`/workflows/${t!.id}/log`)).items;
	}
	async function remove() {
		if (!confirm(`Delete the process “${t!.name}”? Processes already running finish as they are.`)) return;
		await api.del(`/workflows/${t!.id}`);
		goto('/crm/settings?tab=processes');
	}
</script>

<svelte:head><title>{t?.name ?? 'Process'} · NERP</title></svelte:head>

{#if t}
	<a href="/crm/settings?tab=processes" class="mb-4 inline-block text-sm text-slate-500 hover:text-slate-800">← Business processes</a>
	<div class="space-y-4">
		<div class="card flex flex-wrap items-end gap-3 p-5">
			<div class="min-w-60 flex-1"><label class="label" for="wf-n">Process</label><input id="wf-n" class="input" bind:value={t.name} /></div>
			<div><label class="label" for="wf-t">Starts</label><select id="wf-t" class="input" bind:value={t.trigger}><option value="create">When a record is made</option><option value="update">When a record changes</option><option value="manual">When someone starts it on a record</option></select></div>
			<label class="flex items-center gap-2 pb-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={t.is_active} /> On</label>
			<button class="btn btn-primary" onclick={save} data-save-process>Save</button>
		</div>
		<div class="flex flex-wrap items-center gap-3">
			<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" aria-label="Designer view">
				{#each [['flow', 'Flowchart'], ['steps', 'Step details']] as const as [k, l] (k)}
					<button role="tab" aria-selected={view === k} class="rounded-full px-3 py-1 {view === k ? 'bg-white font-medium shadow-sm' : 'text-slate-600'}" onclick={() => (view = k)}>{l}</button>
				{/each}
			</div>
			<p class="text-sm text-slate-500">In texts, {'{{title}}'}, {'{{number}}'}, {'{{author}}'} (the responsible person) and field codes ({t.fields.map((f) => `{{${f.code}}}`).join(', ')}) are filled in.</p>
		</div>
		<div class="card overflow-x-auto p-5" data-designer={view}>
			{#if view === 'flow'}
				<WfFlow bind:steps={t.steps} fields={t.fields} author="responsible person" />
			{:else}
				<WfSteps bind:steps={t.steps} fields={t.fields} bind:people author="responsible person" />
			{/if}
		</div>
		<section class="card space-y-2 p-5">
			<h2 class="font-semibold text-slate-900">Testing</h2>
			<p class="text-sm text-slate-500">While people are named here, the process starts only on records they make or change (and only they can start it by hand).</p>
			<MultiUserPicker bind:value={testers} placeholder="Everyone (not testing)" />
		</section>
		<section class="card space-y-3 p-5" data-run-log>
			<div class="flex items-center gap-3"><h2 class="mr-auto font-semibold text-slate-900">Run log</h2><button class="btn btn-secondary" onclick={loadLog}>Show</button></div>
			{#if entries}
				<ul class="divide-y divide-slate-50 text-sm">
					{#each entries as e, i (i)}
						<li class="flex flex-wrap gap-x-3 py-1.5"><span class="w-36 text-slate-500 tabular-nums">{formatDateTime(e.at)}</span><a class="text-brand-600 hover:underline" href="/crm/{t.crm_type}/{e.item_id}">#{e.number} {e.item_title}</a><span class="text-slate-800">{e.text}</span>{#if e.user}<span class="text-slate-500">· {e.user}</span>{/if}</li>
					{:else}<li class="py-2 text-slate-400">Nothing yet.</li>{/each}
				</ul>
			{/if}
		</section>
		<button class="text-sm text-slate-400 hover:text-red-600" onclick={remove}>Delete this process</button>
	</div>
{/if}
