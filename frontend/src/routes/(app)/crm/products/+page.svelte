<script lang="ts">
	import { onMount } from 'svelte';
	import { api, query, requestBlob } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import CatalogSettings from '#lib/components/crm/CatalogSettings.svelte';
	import ProductExtras from '#lib/components/crm/ProductExtras.svelte';
	import { crm, money } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Product = {
		id: string;
		name: string;
		sku: string | null;
		description: string | null;
		price: string;
		currency: string;
		tax_rate: string | null;
		tax_included: boolean;
		unit: string;
		is_active: boolean;
		in_shop: boolean;
		slug: string | null;
		shop_category_id: string | null;
		image_file_id: string | null;
		stock: string | null;
		parent_id: string | null;
	};
	let catalogOpen = $state(false);
	let extras = $state<ReturnType<typeof ProductExtras> | null>(null);
	/** Opens another product (a variant or its parent) in the form. */
	async function openById(id: string) {
		try {
			edit(await api.get<Product>(`/crm/products/${id}`));
		} catch (err) {
			toasts.error(err);
		}
	}
	type Category = { id: string; name: string; slug: string; sort: number; products: number };
	let categories = $state<Category[]>([]);
	let catOpen = $state(false);
	let newCat = $state('');
	async function loadCategories() {
		categories = (await api.get<{ items: Category[] }>('/shop/categories')).items;
	}
	async function addCategory(e: SubmitEvent) {
		e.preventDefault();
		try {
			categories = (await api.post<{ items: Category[] }>('/shop/categories', { name: newCat })).items;
			newCat = '';
		} catch (err) {
			toasts.error(err);
		}
	}
	async function renameCategory(c: Category, name: string) {
		if (!name.trim() || name === c.name) return;
		try {
			categories = (await api.patch<{ items: Category[] }>(`/shop/categories/${c.id}`, { name })).items;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeCategory(c: Category) {
		if (!confirm(`Delete the category “${c.name}”? Its ${c.products} product${c.products === 1 ? '' : 's'} stay in the shop without a category.`)) return;
		categories = (await api.del<{ items: Category[] }>(`/shop/categories/${c.id}`)).items;
	}
	/** A number without trailing zeros ("12.0000" → "12"). */
	const num = (v: string | null) => (v === null ? '' : String(Number(v)));

	// The photo: uploaded as the employee's own file, shown here from a blob URL.
	let photo = $state<string | null>(null);
	let uploading = $state(false);
	async function showPhoto(id: string | null) {
		if (photo) URL.revokeObjectURL(photo);
		photo = null;
		if (id) photo = URL.createObjectURL(await requestBlob('GET', `/files/${id}/content`));
	}
	async function upload(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;
		uploading = true;
		try {
			const fd = new FormData();
			fd.append('file', file);
			const f = await api.post<{ id: string }>('/files', fd);
			form.image_file_id = f.id;
			await showPhoto(f.id);
		} catch (err) {
			toasts.error(err);
		} finally {
			uploading = false;
			input.value = '';
		}
	}
	let items = $state<Product[]>([]);
	let total = $state(0);
	let q = $state('');
	let showInactive = $state(false);
	let open = $state(false);
	let editing = $state<Product | null>(null);
	const blank = () => ({ name: '', sku: '', description: '', price: '', currency: crm.config?.base_currency ?? '', tax_rate: '', tax_included: false, unit: 'pcs', is_active: true, in_shop: false, shop_category_id: null as string | null, image_file_id: null as string | null, stock: '' });
	let form = $state(blank());

	async function load() {
		const p = await api.get<{ items: Product[]; total: number }>('/crm/products' + query({ q, include_inactive: showInactive || undefined, limit: 200 }));
		items = p.items;
		total = p.total;
	}
	onMount(async () => {
		await crm.load();
		load();
		loadCategories().catch(() => {});
	});
	function edit(p: Product | null) {
		editing = p;
		form = p
			? {
					name: p.name,
					sku: p.sku ?? '',
					description: p.description ?? '',
					price: num(p.price),
					currency: p.currency,
					tax_rate: num(p.tax_rate),
					tax_included: p.tax_included,
					unit: p.unit,
					is_active: p.is_active,
					in_shop: p.in_shop,
					shop_category_id: p.shop_category_id,
					image_file_id: p.image_file_id,
					stock: num(p.stock)
				}
			: blank();
		showPhoto(form.image_file_id).catch(() => (photo = null));
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			// Number inputs hand back numbers (or null when cleared); the API takes decimal strings.
			const text = (v: unknown) => (v === null || v === undefined || v === '' ? null : String(v));
			const body: Record<string, unknown> = { ...form, price: text(form.price) ?? '0', tax_rate: text(form.tax_rate), stock: text(form.stock) };
			// Stock is sent only when changed here: orders may have moved it meanwhile.
			if (editing && text(form.stock) === text(num(editing.stock))) delete body.stock;
			if (editing) {
				await api.patch(`/crm/products/${editing.id}`, body);
				await extras?.save();
			} else await api.post('/crm/products', body);
			open = false;
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove(p: Product) {
		if (!confirm(`Delete ${p.name}?`)) return;
		const r = await api.del<{ deactivated?: boolean }>(`/crm/products/${p.id}`);
		toasts.success(r.deactivated ? 'Used on deals, so it was deactivated instead' : 'Deleted');
		load();
	}
	let t: ReturnType<typeof setTimeout>;
	// Products in another currency are not listed; the shop's currency is in its settings,
	// which only shop managers can read, so fall back to the CRM's base currency.
	let shopCurrency = $state<string | null>(null);
	onMount(() => {
		api.get<{ settings: { currency: string } }>('/shop/settings').then((r) => (shopCurrency = r.settings.currency)).catch(() => (shopCurrency = crm.config?.base_currency ?? null));
	});
</script>

<svelte:head><title>Products · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Products</h1>
		<span class="text-sm text-slate-500">{total}</span>
		{#if crm.config?.can_manage_products}<button class="btn btn-primary ml-auto" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New product</button>{/if}
	</div>
	<div class="flex flex-wrap items-center gap-3">
		<input class="input max-w-sm" placeholder="Search name or SKU" bind:value={q} oninput={() => (clearTimeout(t), (t = setTimeout(load, 250)))} />
		<label class="flex items-center gap-2 text-sm text-slate-600"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={showInactive} onchange={load} /> Show inactive</label>
		{#if crm.config?.can_manage_products}
			<button class="btn btn-secondary ml-auto" onclick={() => (catalogOpen = true)}>Catalog settings</button>
			<button class="btn btn-secondary" onclick={() => (catOpen = true)}>Shop categories</button>
		{/if}
	</div>
	<div class="card overflow-x-auto">
		<table class="min-w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500"><tr><th class="px-4 py-3">Name</th><th class="px-4 py-3">SKU</th><th class="px-4 py-3 text-right">Price</th><th class="px-4 py-3 text-right">Tax</th><th class="px-4 py-3">Shop</th><th class="px-4 py-3 text-right">In stock</th><th class="px-4 py-3"></th></tr></thead>
			<tbody class="divide-y divide-slate-50">
				{#each items as p (p.id)}
					<tr class={p.is_active ? '' : 'text-slate-400'}>
						<td class="px-4 py-2.5 {p.parent_id ? 'pl-10' : ''}" data-product-name={p.name}>{#if p.parent_id}<span class="mr-1 text-slate-300" aria-hidden="true">↳</span>{/if}<span class="font-medium {p.parent_id ? 'font-normal' : ''}">{p.name}</span>{#if !p.is_active}<span class="badge ml-2 bg-slate-100">inactive</span>{/if}</td>
						<td class="px-4 py-2.5 text-slate-500">{p.sku ?? ''}</td>
						<td class="px-4 py-2.5 text-right">{money(p.price, p.currency)} / {p.unit}</td>
						<td class="px-4 py-2.5 text-right text-slate-500">{p.tax_rate !== null ? `${Number(p.tax_rate)}%${p.tax_included ? ' incl.' : ''}` : '—'}</td>
						<td class="px-4 py-2.5 text-slate-600">{#if p.in_shop}<span class="badge bg-emerald-50 text-emerald-800">In the shop</span> <span class="text-xs text-slate-500">{categories.find((c) => c.id === p.shop_category_id)?.name ?? ''}</span>{:else}<span class="text-slate-400">—</span>{/if}</td>
						<td class="px-4 py-2.5 text-right tabular-nums" data-stock={p.name}>{#if p.stock === null}<span class="text-slate-400">not counted</span>{:else if Number(p.stock) <= 0}<span class="font-medium text-red-700">Sold out</span>{:else}{num(p.stock)}{/if}</td>
						<td class="px-4 py-2.5 text-right">
							{#if crm.config?.can_manage_products}
								<button class="btn btn-ghost p-1.5" aria-label="Edit" onclick={() => edit(p)}><Icon name="edit" class="size-4" /></button>
								<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Delete" onclick={() => remove(p)}><Icon name="trash" class="size-4" /></button>
							{/if}
						</td>
					</tr>
				{:else}
					<tr><td colspan="7" class="px-4 py-14 text-center text-slate-500">No products yet.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>

<Modal bind:open title={editing ? 'Edit product' : 'New product'}>
	<form id="prod" class="grid gap-3 sm:grid-cols-2" onsubmit={save}>
		<div class="sm:col-span-2"><label class="label" for="p-n">Name</label><input id="p-n" class="input" required bind:value={form.name} /></div>
		<div><label class="label" for="p-sku">SKU</label><input id="p-sku" class="input" bind:value={form.sku} /></div>
		<div><label class="label" for="p-u">Unit</label><input id="p-u" class="input" bind:value={form.unit} /></div>
		<div><label class="label" for="p-p">Price</label><input id="p-p" class="input" type="number" min="0" step="0.01" required bind:value={form.price} /></div>
		<div><label class="label" for="p-c">Currency</label><select id="p-c" class="input" bind:value={form.currency}>{#each crm.config?.currencies ?? [] as c (c.code)}<option>{c.code}</option>{/each}</select></div>
		<div><label class="label" for="p-t">Tax rate, %</label><input id="p-t" class="input" type="number" min="0" max="100" step="0.01" bind:value={form.tax_rate} /></div>
		<label class="flex items-end gap-2 pb-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.tax_included} /> Price includes tax</label>
		<div class="sm:col-span-2"><label class="label" for="p-d">Description</label><textarea id="p-d" class="input" rows="2" bind:value={form.description}></textarea></div>
		{#if editing}<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.is_active} /> Active</label>{/if}
		<div class="sm:col-span-2"><label class="label" for="p-st">In stock</label><input id="p-st" class="input" type="number" min="0" step="1" placeholder="Not counted" bind:value={form.stock} /><p class="mt-1 text-xs text-slate-500">Total over all warehouses; leave empty if you don't count it. A change here goes in or out of the default warehouse. For receipts, write-offs and moves, use Inventory.</p></div>
		{#if editing}<ProductExtras bind:this={extras} productId={editing.id} onopen={openById} onchanged={load} />{/if}
		<fieldset class="space-y-3 rounded-xl border border-slate-200 p-4 sm:col-span-2">
			<legend class="px-1 text-sm font-medium text-slate-800">Online shop</legend>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.in_shop} /> Sell in the online shop</label>
			{#if form.in_shop}
				{#if crm.config && shopCurrency && form.currency !== shopCurrency}<p class="text-xs text-amber-800">The shop sells in {shopCurrency}; this product is priced in {form.currency}, so it won't be listed.</p>{/if}
				<div><label class="label" for="p-cat">Category</label><select id="p-cat" class="input" bind:value={form.shop_category_id}><option value={null}>No category</option>{#each categories as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select></div>
				<div>
					<span class="label">Photo</span>
					<div class="flex items-center gap-3">
						{#if photo}<img src={photo} alt="" class="size-16 rounded-lg border border-slate-200 object-cover" data-photo />{:else}<div class="flex size-16 items-center justify-center rounded-lg border border-dashed border-slate-300 text-xs text-slate-400">None</div>{/if}
						<label class="btn btn-secondary cursor-pointer">{uploading ? 'Uploading…' : photo ? 'Replace' : 'Upload'}<input type="file" class="sr-only" accept="image/png,image/jpeg,image/webp,image/gif" aria-label="Product photo" onchange={upload} /></label>
						{#if photo}<button type="button" class="btn btn-ghost" onclick={() => ((form.image_file_id = null), showPhoto(null))}>Remove</button>{/if}
					</div>
				</div>
			{/if}
		</fieldset>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="prod">Save</button>
	{/snippet}
</Modal>

<CatalogSettings bind:open={catalogOpen} />

<Modal bind:open={catOpen} title="Shop categories">
	<p class="mb-3 text-sm text-slate-500">How the shop groups products. Renaming keeps the category's address.</p>
	<ul class="mb-4 divide-y divide-slate-100 rounded-xl border border-slate-200">
		{#each categories as c (c.id)}
			<li class="flex items-center gap-2 px-3 py-2" data-category={c.name}>
				<input class="input flex-1" value={c.name} aria-label="Category name" onchange={(e) => renameCategory(c, e.currentTarget.value)} />
				<span class="w-20 text-right text-xs text-slate-500">{c.products} product{c.products === 1 ? '' : 's'}</span>
				<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Delete {c.name}" onclick={() => removeCategory(c)}><Icon name="trash" class="size-4" /></button>
			</li>
		{:else}
			<li class="px-3 py-6 text-center text-sm text-slate-500">No categories yet.</li>
		{/each}
	</ul>
	<form class="flex gap-2" onsubmit={addCategory}>
		<input class="input flex-1" required placeholder="New category" bind:value={newCat} aria-label="New category" />
		<button class="btn btn-primary">Add</button>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (catOpen = false)}>Done</button>
	{/snippet}
</Modal>
