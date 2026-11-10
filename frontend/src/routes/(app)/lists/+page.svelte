<script lang="ts">
	// Lists: the company's own small databases, with business processes.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type L = { id: string; name: string; description: string | null; items: number; level: string; to_all: boolean };
	let items = $state<L[]>([]);
	let canCreate = $state(false);
	onMount(async () => {
		const r = await api.get<{ items: L[]; can_create: boolean }>('/lists');
		items = r.items;
		canCreate = r.can_create;
	});
	// Ready-made starting points.
	const STARTS: Record<string, { label: string; fields: { label: string; kind: string; options?: string[]; required?: boolean }[] }> = {
		empty: { label: 'Empty', fields: [] },
		purchase: { label: 'Purchase requests', fields: [{ label: 'Amount', kind: 'money', required: true }, { label: 'Supplier', kind: 'text' }, { label: 'Needed by', kind: 'date' }, { label: 'Reason', kind: 'textarea' }, { label: 'Status', kind: 'select', options: ['New', 'Approved', 'Rejected', 'Ordered'] }] },
		assets: { label: 'Company assets', fields: [{ label: 'Serial number', kind: 'text' }, { label: 'Holder', kind: 'user' }, { label: 'Bought on', kind: 'date' }, { label: 'Price', kind: 'money' }] },
		contracts: { label: 'Contracts', fields: [{ label: 'Client', kind: 'crm' }, { label: 'Signed on', kind: 'date' }, { label: 'Ends on', kind: 'date' }, { label: 'Document', kind: 'file' }] }
	};
	let open = $state(false);
	let form = $state({ name: '', description: '', start: 'empty' });
	async function create(e: SubmitEvent) {
		e.preventDefault();
		try {
			const l = await api.post<{ id: string }>('/lists', { name: form.name, description: form.description || null, fields: STARTS[form.start].fields.map((f) => ({ ...f, options: f.options ?? [], required: f.required ?? false })) });
			goto(`/lists/${l.id}?tab=settings`);
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>Lists · NERP</title></svelte:head>

<div class="max-w-5xl space-y-4">
	<div class="flex items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Lists</h1>
		<a class="btn btn-secondary" href="/approvals">My approvals</a>
		{#if canCreate}<button class="btn btn-primary" onclick={() => (open = true)}><Icon name="plus" class="size-4" /> New list</button>{/if}
	</div>
	<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
		{#each items as l (l.id)}
			<a href="/lists/{l.id}" class="card block p-5 hover:shadow-g2" data-list={l.name}>
				<p class="font-semibold text-slate-900">{l.name}</p>
				{#if l.description}<p class="mt-1 line-clamp-2 text-sm text-slate-600">{l.description}</p>{/if}
				<p class="mt-3 text-xs text-slate-500">{l.items} item{l.items === 1 ? '' : 's'} · you can {l.level === 'manage' ? 'manage it' : l.level === 'add' ? 'add items' : 'read it'}</p>
			</a>
		{:else}
			<div class="card p-10 text-center text-slate-500 sm:col-span-2 lg:col-span-3">No lists yet.{canCreate ? ' Make one for purchase requests, assets, contracts… and add an approval process.' : ''}</div>
		{/each}
	</div>
</div>

<Modal bind:open title="New list">
	<form id="list-new" class="space-y-3" onsubmit={create}>
		<div><label class="label" for="ln-n">Name</label><input id="ln-n" class="input" required maxlength="120" bind:value={form.name} /></div>
		<div><label class="label" for="ln-d">What it's for</label><input id="ln-d" class="input" bind:value={form.description} /></div>
		<div><label class="label" for="ln-s">Start from</label><select id="ln-s" class="input" bind:value={form.start}>{#each Object.entries(STARTS) as [k, v] (k)}<option value={k}>{v.label}</option>{/each}</select></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="list-new">Create</button>
	{/snippet}
</Modal>
