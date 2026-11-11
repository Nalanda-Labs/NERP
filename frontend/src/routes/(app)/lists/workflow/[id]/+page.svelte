<script lang="ts">
	// Designing a business process: when it starts, and its steps.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import WfSteps, { type ListField, type Step } from '#lib/components/WfSteps.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type T = { id: string; list_id: string; name: string; trigger: string; steps: Step[]; is_active: boolean; test_users: string[] };
	let testers = $state<PickedUser[]>([]);
	let t = $state<T | null>(null);
	let fields = $state<ListField[]>([]);
	let people = $state<Record<string, { id: string; name: string; avatar_file_id: string | null }>>({});
	$effect(() => {
		api.get<T>(`/workflows/${page.params.id}`).then(async (x) => {
			t = x;
			fields = (await api.get<{ fields: ListField[] }>(`/lists/${x.list_id}`)).fields;
			// Names of the people already in the steps.
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
			t = await api.patch<T>(`/workflows/${t!.id}`, { name: t!.name, trigger: t!.trigger, steps: t!.steps, is_active: t!.is_active, test_users: testers.map((u) => u.id) });
			toasts.success('Saved');
		} catch (e) {
			toasts.error(e);
		}
	}
	// The run log over a period: what the process did on every item.
	type Entry = { at: string; text: string; user: string | null; item_title: string; number: number; item_id: string; status: string };
	let logFrom = $state(new Date(Date.now() - 7 * 86400e3).toISOString().slice(0, 10));
	let logTo = $state(new Date().toISOString().slice(0, 10));
	let entries = $state<Entry[] | null>(null);
	async function loadLog() {
		try {
			const to = new Date(`${logTo}T00:00:00Z`);
			to.setUTCDate(to.getUTCDate() + 1);
			entries = (await api.get<{ items: Entry[] }>(`/workflows/${t!.id}/log?from=${encodeURIComponent(`${logFrom}T00:00:00Z`)}&to=${encodeURIComponent(to.toISOString())}`)).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function remove() {
		if (!confirm(`Delete the process “${t!.name}”? Processes already running finish as they are.`)) return;
		await api.del(`/workflows/${t!.id}`);
		goto(`/lists/${t!.list_id}?tab=settings`);
	}
</script>

<svelte:head><title>{t?.name ?? 'Process'} · NERP</title></svelte:head>

{#if t}
	<a href="/lists/{t.list_id}?tab=settings" class="mb-4 inline-block text-sm text-slate-500 hover:text-slate-800">← The list</a>
	<div class="max-w-5xl space-y-4">
		<div class="card flex flex-wrap items-end gap-3 p-5">
			<div class="min-w-60 flex-1"><label class="label" for="wf-n">Process</label><input id="wf-n" class="input" bind:value={t.name} /></div>
			<div><label class="label" for="wf-t">Starts</label><select id="wf-t" class="input" bind:value={t.trigger}><option value="create">When an item is added</option><option value="update">When an item changes</option><option value="manual">When someone starts it</option></select></div>
			<label class="flex items-center gap-2 pb-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={t.is_active} /> On</label>
			<button class="btn btn-primary" onclick={save}>Save</button>
		</div>
		<p class="text-sm text-slate-500">Steps run from top to bottom. In texts, {'{{title}}'}, {'{{number}}'}, {'{{author}}'} and field codes ({fields.map((f) => `{{${f.code}}}`).join(', ')}) are filled in.</p>
		<div class="card p-5"><WfSteps bind:steps={t.steps} {fields} bind:people /></div>
		<section class="card space-y-2 p-5" aria-labelledby="wf-test-h">
			<h2 id="wf-test-h" class="font-semibold text-slate-900">Testing</h2>
			<p class="text-sm text-slate-500">While people are named here, the process starts only for their items (and only they can start it by hand), so it can be tried without anyone else noticing. Remove them all to run it for everyone.</p>
			<MultiUserPicker bind:value={testers} placeholder="Everyone (not testing)" />
		</section>
		<section class="card space-y-3 p-5" aria-labelledby="wf-log-h" data-run-log>
			<div class="flex flex-wrap items-end gap-3">
				<h2 id="wf-log-h" class="mr-auto font-semibold text-slate-900">Run log</h2>
				<label class="text-sm">From <input type="date" class="input w-40" bind:value={logFrom} /></label>
				<label class="text-sm">To <input type="date" class="input w-40" bind:value={logTo} /></label>
				<button class="btn btn-secondary" onclick={loadLog}>Show</button>
			</div>
			{#if entries}
				<ul class="divide-y divide-slate-50 text-sm">
					{#each entries as e, i (i)}
						<li class="flex flex-wrap gap-x-3 py-1.5"><span class="w-36 text-slate-500 tabular-nums">{formatDateTime(e.at)}</span><a class="text-brand-600 hover:underline" href="/lists/{t.list_id}?item={e.item_id}">#{e.number} {e.item_title}</a><span class="text-slate-800">{e.text}</span>{#if e.user}<span class="text-slate-500">· {e.user}</span>{/if}</li>
					{:else}<li class="py-2 text-slate-400">Nothing in this period.</li>{/each}
				</ul>
			{/if}
		</section>
		<button class="text-sm text-slate-400 hover:text-red-600" onclick={remove}>Delete this process</button>
	</div>
{/if}
