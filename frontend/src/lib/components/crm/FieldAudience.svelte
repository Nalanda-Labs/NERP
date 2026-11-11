<script lang="ts">
	// Who sees a custom CRM field: chosen people and departments (with their sub-departments).
	// Nobody chosen: everyone who can see the record. CRM admins always see every field.
	import { api } from '#lib/api.ts';
	import type { FieldDef } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import MultiUserPicker, { type PickedUser } from '../MultiUserPicker.svelte';

	let { field, typeCode, onsaved }: { field: FieldDef; typeCode: string; onsaved: () => void } = $props();
	let dialog: HTMLDialogElement | undefined = $state();
	let users = $state<PickedUser[]>([]);
	let deps = $state<string[]>([]);
	let departments = $state<{ id: string; name: string }[]>([]);
	let saving = $state(false);
	const count = $derived((field.visible_user_ids?.length ?? 0) + (field.visible_department_ids?.length ?? 0));

	async function open() {
		const ids = field.visible_user_ids ?? [];
		const [people, d] = await Promise.all([
			Promise.all(ids.map((id) => api.get<{ id: string; first_name: string; last_name: string; avatar_file_id: string | null }>(`/users/${id}`).catch(() => null))),
			departments.length ? Promise.resolve(departments) : api.get<{ id: string; name: string }[]>('/departments').catch(() => [])
		]);
		users = people.filter((u) => !!u).map((u) => ({ id: u.id, name: `${u.first_name} ${u.last_name}`, avatar_file_id: u.avatar_file_id }));
		departments = d;
		deps = [...(field.visible_department_ids ?? [])];
		dialog?.showModal();
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			await api.patch(`/crm/${typeCode}/fields/${field.id}`, { visible_user_ids: users.map((u) => u.id), visible_department_ids: deps });
			dialog?.close();
			onsaved();
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
</script>

<button type="button" class="btn btn-ghost px-2 py-1 text-xs" data-field-audience={field.code} onclick={open}>
	{count ? `Seen by ${count} chosen` : 'Seen by everyone'}
</button>

<dialog bind:this={dialog} class="w-full max-w-lg rounded-xl p-0 shadow-xl backdrop:bg-slate-900/40">
	<form class="space-y-4 p-5 text-sm" onsubmit={save}>
		<h2 class="font-medium text-slate-900">Who sees “{field.label}”</h2>
		<div><span class="label">People</span><MultiUserPicker bind:value={users} placeholder="Everyone" /></div>
		<div>
			<span class="label">Departments <span class="font-normal text-slate-400">(with sub-departments)</span></span>
			<div class="grid max-h-48 grid-cols-2 gap-1 overflow-y-auto">
				{#each departments as d (d.id)}<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" value={d.id} bind:group={deps} /> {d.name}</label>{/each}
			</div>
		</div>
		<p class="text-xs text-slate-500">Nobody chosen: everyone who can see the record. Others don’t see the field, its values, its changes in the history, or filter by it. CRM admins always see it.</p>
		<div class="flex justify-end gap-2">
			<button type="button" class="btn btn-secondary" onclick={() => dialog?.close()}>Cancel</button>
			<button class="btn btn-primary" disabled={saving}>Save</button>
		</div>
	</form>
</dialog>
