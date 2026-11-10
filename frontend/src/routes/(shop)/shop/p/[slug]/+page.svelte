<script lang="ts">
	// One product: photo, description, details, price, how many are left, add to cart. A
	// product with variants asks for each choice (size, colour…) first.
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { money } from '#lib/crm.svelte.ts';
	import { shop, type ShopProduct } from '../../../shop.svelte.ts';

	type Axis = { id: string; name: string; values: string[] };
	type Res = { product: ShopProduct; variants: ShopProduct[]; axes: Axis[]; props: { name: string; value: string }[]; selected: string | null };
	let r = $state<Res | null>(null);
	let missing = $state(false);
	let quantity = $state(1);
	let added = $state(false);
	let pick = $state<Record<string, string>>({});
	$effect(() => {
		const slug = page.params.slug;
		r = null;
		missing = false;
		added = false;
		api.get<Res>(`/public/shop/products/${encodeURIComponent(slug ?? '')}`)
			.then((x) => {
				r = x;
				// A variant's own address preselects it.
				const v = x.variants.find((v) => v.slug === x.selected);
				pick = v?.values ? { ...v.values } : {};
			})
			.catch(() => (missing = true));
	});
	const hasVariants = $derived(!!r?.variants.length);
	/** The variant matching every choice, when all are made. */
	const variant = $derived(hasVariants && r ? (r.variants.find((v) => r!.axes.every((a) => v.values?.[a.id] === pick[a.id])) ?? null) : null);
	/** What is being bought: the variant, or the product itself. */
	const item = $derived(hasVariants ? variant : (r?.product ?? null));
	const shown = $derived(item ?? r?.product ?? null);
	/** Can this value be combined with the other choices? */
	function possible(axis: string, value: string) {
		return !!r?.variants.some((v) => v.values?.[axis] === value && r!.axes.every((a) => a.id === axis || !pick[a.id] || v.values?.[a.id] === pick[a.id]));
	}
	function choose(axis: string, value: string) {
		pick = { ...pick, [axis]: pick[axis] === value ? '' : value };
		added = false;
	}
	const left = $derived(!item || item.stock === null ? null : Number(item.stock));
	const inCart = $derived(shop.cart.find((l) => l.slug === item?.slug)?.quantity ?? 0);
	const canAdd = $derived(left === null ? 999 - inCart : Math.max(0, left - inCart));
	const missingChoice = $derived(r?.axes.find((a) => !pick[a.id]));
	function add() {
		if (!item) return;
		shop.add(item, Math.max(1, Math.min(canAdd, Math.floor(quantity))));
		added = true;
		quantity = 1;
	}
</script>

<svelte:head><title>{r?.product.name ?? 'Product'} · {shop.info?.name ?? 'Shop'}</title></svelte:head>

<a href="/shop" class="mb-6 inline-block text-sm text-slate-500 hover:text-slate-800">← All products</a>
{#if missing}
	<div class="rounded-3xl bg-white p-12 text-center shadow-g1">
		<h1 class="text-xl font-semibold text-slate-900">This product isn't sold here any more</h1>
		<a class="btn btn-primary mt-6" href="/shop">See what we sell</a>
	</div>
{:else if r && shown && shop.info}
	<article class="grid gap-8 md:grid-cols-2">
		<div class="aspect-square overflow-hidden rounded-3xl bg-white shadow-g1">
			{#if shown.image}<img src={shown.image} alt={r.product.name} class="size-full object-cover" />{:else}<span class="flex size-full items-center justify-center text-slate-300"><Icon name="store" class="size-16" /></span>{/if}
		</div>
		<div class="space-y-5">
			<h1 class="text-3xl font-semibold tracking-tight text-slate-900">{r.product.name}</h1>
			<p class="text-2xl font-semibold text-slate-900 tabular-nums" data-price>
				{#if hasVariants && !variant}<span class="text-base font-normal text-slate-500">from </span>{/if}{money(shown.price, shop.info.currency)}
				{#if shown.unit !== 'pcs'}<span class="text-base font-normal text-slate-500">/ {shown.unit}</span>{/if}
			</p>
			{#if shown.tax_rate && Number(shown.tax_rate) > 0}<p class="-mt-3 text-sm text-slate-500">Includes {Number(shown.tax_rate)}% tax</p>{/if}
			{#if shown.description}<p class="whitespace-pre-line text-slate-700">{shown.description}</p>{/if}

			{#each r.axes as a (a.id)}
				<div role="group" aria-label={a.name}>
					<p class="mb-1.5 text-sm font-medium text-slate-700">{a.name}{#if pick[a.id]}: <span class="font-normal">{pick[a.id]}</span>{/if}</p>
					<div class="flex flex-wrap gap-2">
						{#each a.values as v (v)}
							<button
								class="rounded-xl border px-3.5 py-2 text-sm font-medium {pick[a.id] === v ? 'border-slate-900 bg-slate-900 text-white' : possible(a.id, v) ? 'border-slate-200 bg-white text-slate-700 hover:border-slate-400' : 'border-dashed border-slate-200 bg-white text-slate-300'}"
								aria-pressed={pick[a.id] === v}
								disabled={!possible(a.id, v) && pick[a.id] !== v}
								onclick={() => choose(a.id, v)}>{v}</button
							>
						{/each}
					</div>
				</div>
			{/each}

			{#if left !== null}
				<p class="text-sm font-medium {left <= 0 ? 'text-red-700' : left <= 5 ? 'text-amber-800' : 'text-emerald-800'}" data-left>{left <= 0 ? 'Sold out' : left <= 5 ? `Only ${left} left` : 'In stock'}</p>
			{/if}
			{#if hasVariants && missingChoice}
				<p class="text-sm text-slate-500">Choose {missingChoice.name.toLowerCase()}.</p>
			{:else if item && (left === null || left > 0)}
				<div class="flex items-center gap-3">
					<label class="sr-only" for="qty">Quantity</label>
					<input id="qty" class="input w-24" type="number" min="1" max={canAdd} step="1" bind:value={quantity} disabled={canAdd <= 0} />
					<button class="btn btn-primary" onclick={add} disabled={canAdd <= 0}>Add to cart</button>
				</div>
				{#if canAdd <= 0 && inCart}<p class="text-sm text-slate-500">All {inCart} left are in your cart.</p>{/if}
				{#if added}<p class="text-sm text-slate-600">Added. <a class="font-medium text-brand-600 hover:underline" href="/shop/cart">Go to your cart ({shop.count})</a></p>{/if}
			{/if}

			{#if r.props.length}
				<dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 border-t border-slate-200 pt-4 text-sm" aria-label="Details">
					{#each r.props as x (x.name)}<dt class="text-slate-500">{x.name}</dt><dd class="text-slate-800">{x.value}</dd>{/each}
				</dl>
			{/if}
		</div>
	</article>
{:else}
	<p class="text-sm text-slate-400">Loading…</p>
{/if}
