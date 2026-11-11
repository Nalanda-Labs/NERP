<script lang="ts">
	// CRM settings → Business processes: a record type's processes (approvals, tasks,
	// notifications, field changes, conditions, waits), each opened in the designer.
	import { goto } from '$app/navigation';
	import { api } from '#lib/api.ts';
	import type { CrmType } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';

	let { types }: { types: CrmType[] } = $props();
	let code = $state('deal');
	type T = { id: string; name: string; trigger: string; is_active: boolean; test_users: string[] };
	let items = $state<T[]>([]);
	const TRIGGER: Record<string, string> = { create: 'When a record is made', update: 'When a record changes', manual: 'Started by hand' };
	async function load() {
		items = (await api.get<{ items: T[] }>(`/crm/${code}/workflows`).catch(() => ({ items: [] }))).items;
	}
	$effect(() => {
		code;
		load();
	});
	async function create() {
		const name = prompt('Name of the process');
		if (!name?.trim()) return;
		try {
			const t = await api.post<{ id: string }>(`/crm/${code}/workflows`, { name, trigger: 'manual', steps: [] });
			goto(`/crm/workflows/${t.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function toggle(t: T) {
		await api.patch(`/workflows/${t.id}`, { is_active: !t.is_active }).catch(toasts.error);
		load();
	}
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<select class="input w-48" aria-label="Record type" bind:value={code}>{#each types as t (t.code)}<option value={t.code}>{t.name_plural}</option>{/each}</select>
		<p class="flex-1 text-sm text-slate-500">Processes run steps on a record — approvals, tasks, notifications, field changes, conditions and waits — when it is made, when it changes, or when someone starts one.</p>
		<button class="btn btn-primary" onclick={create} data-new-process><Icon name="plus" class="size-4" /> New process</button>
	</div>
	<section class="card divide-y divide-slate-50">
		{#each items as t (t.id)}
			<div class="flex items-center gap-3 px-5 py-3 text-sm {t.is_active ? '' : 'opacity-60'}" data-process={t.name}>
				<a class="flex-1 font-medium text-slate-900 hover:underline" href="/crm/workflows/{t.id}">{t.name}</a>
				<span class="text-slate-500">{TRIGGER[t.trigger]}{t.test_users.length ? ' · testing' : ''}</span>
				<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => toggle(t)}>{t.is_active ? 'Turn off' : 'Turn on'}</button>
			</div>
		{:else}
			<p class="px-5 py-6 text-sm text-slate-500">No processes for these records yet.</p>
		{/each}
	</section>
</div>
