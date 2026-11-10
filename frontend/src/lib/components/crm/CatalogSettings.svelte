<script module lang="ts">
	export type Prop = { id: string; name: string; kind: 'text' | 'number' | 'list'; options: string[]; is_variant: boolean; in_shop: boolean };
</script>

<script lang="ts">
	// Products → Catalog settings: product properties (and which make variants) and price types.
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	let { open = $bindable(false), onchanged }: { open?: boolean; onchanged?: () => void } = $props();

	let props_ = $state<Prop[]>([]);
	let types = $state<{ id: string; name: string }[]>([]);
	$effect(() => {
		if (!open) return;
		api.get<{ items: Prop[] }>('/crm/product-props').then((r) => (props_ = r.items));
		api.get<{ items: typeof types }>('/crm/price-types').then((r) => (types = r.items));
	});

	let draft = $state({ name: '', kind: 'text' as Prop['kind'], options: '', is_variant: false });
	const split = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);
	async function run<T>(fn: () => Promise<T>): Promise<T | null> {
		try {
			const r = await fn();
			onchanged?.();
			return r;
		} catch (e) {
			toasts.error(e);
			return null;
		}
	}
	async function addProp(e: SubmitEvent) {
		e.preventDefault();
		const r = await run(() => api.post<{ items: Prop[] }>('/crm/product-props', { ...draft, options: split(draft.options), is_variant: draft.kind === 'list' && draft.is_variant }));
		if (r) {
			props_ = r.items;
			draft = { name: '', kind: 'text', options: '', is_variant: false };
		}
	}
	async function patchProp(p: Prop, body: Partial<Omit<Prop, 'options'>> & { options?: string[] }) {
		const r = await run(() => api.patch<{ items: Prop[] }>(`/crm/product-props/${p.id}`, body));
		if (r) props_ = r.items;
	}
	async function removeProp(p: Prop) {
		if (!confirm(`Delete the property “${p.name}”? Products lose their values of it.`)) return;
		const r = await run(() => api.del<{ items: Prop[] }>(`/crm/product-props/${p.id}`));
		if (r) props_ = r.items;
	}
	let newType = $state('');
	async function addType(e: SubmitEvent) {
		e.preventDefault();
		const r = await run(() => api.post<{ items: typeof types }>('/crm/price-types', { name: newType }));
		if (r) {
			types = r.items;
			newType = '';
		}
	}
	async function renameType(t: { id: string; name: string }, name: string) {
		if (!name.trim() || name === t.name) return;
		const r = await run(() => api.patch<{ items: typeof types }>(`/crm/price-types/${t.id}`, { name }));
		if (r) types = r.items;
	}
	async function removeType(t: { id: string; name: string }) {
		if (!confirm(`Delete the price type “${t.name}” and its prices? Deals keep the prices they have.`)) return;
		const r = await run(() => api.del<{ items: typeof types }>(`/crm/price-types/${t.id}`));
		if (r) types = r.items;
	}
	const KINDS = { text: 'Text', number: 'Number', list: 'List of values' };
</script>

<Modal bind:open title="Catalog settings">
	<section class="space-y-3" aria-labelledby="cs-props">
		<h3 id="cs-props" class="font-medium text-slate-900">Properties</h3>
		<p class="text-sm text-slate-500">Details products can have. A list marked “makes variants” (sizes, colours…) is used to make a product's variants.</p>
		<ul class="divide-y divide-slate-100 rounded-xl border border-slate-200">
			{#each props_ as p (p.id)}
				<li class="space-y-2 px-3 py-2.5" data-prop={p.name}>
					<div class="flex items-center gap-2">
						<input class="input flex-1" value={p.name} aria-label="Property name" onchange={(e) => patchProp(p, { name: e.currentTarget.value })} />
						<span class="w-28 text-xs text-slate-500">{KINDS[p.kind]}</span>
						<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Delete {p.name}" onclick={() => removeProp(p)}><Icon name="trash" class="size-4" /></button>
					</div>
					{#if p.kind === 'list'}
						<input class="input text-sm" value={p.options.join(', ')} aria-label="Values of {p.name}" onchange={(e) => patchProp(p, { options: split(e.currentTarget.value) })} />
					{/if}
					<div class="flex flex-wrap gap-4 text-sm text-slate-600">
						{#if p.kind === 'list'}<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={p.is_variant} onchange={(e) => patchProp(p, { is_variant: e.currentTarget.checked })} /> Makes variants</label>{/if}
						<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={p.in_shop} onchange={(e) => patchProp(p, { in_shop: e.currentTarget.checked })} /> Shown in the shop</label>
					</div>
				</li>
			{:else}
				<li class="px-3 py-5 text-center text-sm text-slate-500">No properties yet.</li>
			{/each}
		</ul>
		<form class="grid gap-2 rounded-xl bg-slate-50 p-3 sm:grid-cols-[1fr_10rem_auto]" onsubmit={addProp}>
			<input class="input" required placeholder="New property, e.g. Size" aria-label="New property" bind:value={draft.name} />
			<select class="input" aria-label="Kind" bind:value={draft.kind}>{#each Object.entries(KINDS) as [k, v] (k)}<option value={k}>{v}</option>{/each}</select>
			<button class="btn btn-primary">Add</button>
			{#if draft.kind === 'list'}
				<input class="input sm:col-span-3" required placeholder="Values, separated by commas: S, M, L" aria-label="Values" bind:value={draft.options} />
				<label class="flex items-center gap-2 text-sm text-slate-600 sm:col-span-3"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.is_variant} /> Makes variants</label>
			{/if}
		</form>
	</section>

	<section class="mt-6 space-y-3" aria-labelledby="cs-prices">
		<h3 id="cs-prices" class="font-medium text-slate-900">Price types</h3>
		<p class="text-sm text-slate-500">Extra prices beside each product's base price, e.g. Wholesale. Product rows on deals can use them.</p>
		<ul class="divide-y divide-slate-100 rounded-xl border border-slate-200">
			{#each types as t (t.id)}
				<li class="flex items-center gap-2 px-3 py-2" data-price-type={t.name}>
					<input class="input flex-1" value={t.name} aria-label="Price type name" onchange={(e) => renameType(t, e.currentTarget.value)} />
					<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Delete {t.name}" onclick={() => removeType(t)}><Icon name="trash" class="size-4" /></button>
				</li>
			{:else}
				<li class="px-3 py-5 text-center text-sm text-slate-500">Only the base price.</li>
			{/each}
		</ul>
		<form class="flex gap-2" onsubmit={addType}>
			<input class="input flex-1" required placeholder="New price type, e.g. Wholesale" aria-label="New price type" bind:value={newType} />
			<button class="btn btn-primary">Add</button>
		</form>
	</section>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Done</button>
	{/snippet}
</Modal>
