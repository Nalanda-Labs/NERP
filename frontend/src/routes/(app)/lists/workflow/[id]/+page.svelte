<script lang="ts">
	// Designing a business process: when it starts, and its steps.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import WfSteps, { type ListField, type Step } from '#lib/components/WfSteps.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type T = { id: string; list_id: string; name: string; trigger: string; steps: Step[]; is_active: boolean };
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
			for (const uid of ids) api.get<{ id: string; full_name: string; avatar_file_id: string | null }>(`/users/${uid}`).then((u) => (people[uid] = { id: u.id, name: u.full_name, avatar_file_id: u.avatar_file_id })).catch(() => {});
		}).catch((e) => toasts.error(e));
	});
	async function save() {
		try {
			t = await api.patch<T>(`/workflows/${t!.id}`, { name: t!.name, trigger: t!.trigger, steps: t!.steps, is_active: t!.is_active });
			toasts.success('Saved');
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
		<button class="text-sm text-slate-400 hover:text-red-600" onclick={remove}>Delete this process</button>
	</div>
{/if}
