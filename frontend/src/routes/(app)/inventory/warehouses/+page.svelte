<script lang="ts">
	// Inventory → Warehouses.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { qty, type Warehouse } from '#lib/inventory.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let items = $state<Warehouse[]>([]);
	const load = async () => (items = (await api.get<{ items: Warehouse[] }>('/inventory/warehouses')).items);
	onMount(load);

	let open = $state(false);
	let editing = $state<Warehouse | null>(null);
	let form = $state({ name: '', address: '' });
	function edit(w: Warehouse | null) {
		editing = w;
		form = { name: w?.name ?? '', address: w?.address ?? '' };
		open = true;
	}
	async function call(fn: () => Promise<{ items: Warehouse[] }>, done?: string) {
		try {
			items = (await fn()).items;
			if (done) toasts.success(done);
			return true;
		} catch (e) {
			toasts.error(e);
			return false;
		}
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		const body = { name: form.name, address: form.address || null };
		if (await call(() => (editing ? api.patch(`/inventory/warehouses/${editing.id}`, body) : api.post('/inventory/warehouses', body)))) open = false;
	}
	const patch = (w: Warehouse, b: object, done: string) => call(() => api.patch(`/inventory/warehouses/${w.id}`, b), done);
	async function remove(w: Warehouse) {
		if (!confirm(`Delete ${w.name}?`)) return;
		await call(() => api.del(`/inventory/warehouses/${w.id}`), 'Deleted');
	}
</script>

<svelte:head><title>Warehouses · NERP</title></svelte:head>

<div class="max-w-4xl space-y-4">
	<div class="flex items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Warehouses</h1>
		<button class="btn btn-primary ml-auto" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New warehouse</button>
	</div>
	<p class="text-sm text-slate-500">The default warehouse is where new documents start, and where “In stock” on a product form takes or adds units.</p>
	<div class="card divide-y divide-slate-50">
		{#each items as w (w.id)}
			<div class="flex flex-wrap items-center gap-3 px-5 py-3" data-warehouse={w.name}>
				<div class="min-w-0 flex-1">
					<p class="font-medium {w.is_active ? 'text-slate-900' : 'text-slate-400'}">
						{w.name}
						{#if w.is_default}<span class="badge ml-1 bg-brand-50 text-brand-700">Default</span>{/if}
						{#if !w.is_active}<span class="badge ml-1 bg-slate-100">Inactive</span>{/if}
					</p>
					<p class="text-sm text-slate-500">{w.address ?? ''}{w.address ? ' · ' : ''}{w.products} product{w.products === 1 ? '' : 's'}, {qty(w.units)} units</p>
				</div>
				{#if w.is_active && !w.is_default}<button class="btn btn-ghost px-2 py-1 text-sm" onclick={() => patch(w, { is_default: true }, `${w.name} is the default`)}>Make default</button>{/if}
				{#if !w.is_default}<button class="btn btn-ghost px-2 py-1 text-sm" onclick={() => patch(w, { is_active: !w.is_active }, w.is_active ? 'Deactivated' : 'Activated')}>{w.is_active ? 'Deactivate' : 'Activate'}</button>{/if}
				<button class="btn btn-ghost p-1.5" aria-label="Edit {w.name}" onclick={() => edit(w)}><Icon name="edit" class="size-4" /></button>
				{#if !w.is_default}<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Delete {w.name}" onclick={() => remove(w)}><Icon name="trash" class="size-4" /></button>{/if}
			</div>
		{/each}
	</div>
</div>

<Modal bind:open title={editing ? 'Edit warehouse' : 'New warehouse'}>
	<form id="wh-form" class="space-y-3" onsubmit={save}>
		<div><label class="label" for="wh-name">Name</label><input id="wh-name" class="input" required maxlength="120" bind:value={form.name} /></div>
		<div><label class="label" for="wh-addr">Address</label><textarea id="wh-addr" class="input" rows="2" bind:value={form.address}></textarea></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="wh-form">Save</button>
	{/snippet}
</Modal>
