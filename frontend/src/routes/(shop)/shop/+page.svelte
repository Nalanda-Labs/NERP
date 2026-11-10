<script lang="ts">
	// The shop's front page: products by category, with search.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { money } from '#lib/crm.svelte.ts';
	import { shop, type ShopProduct } from '../shop.svelte.ts';

	let items = $state<ShopProduct[] | null>(null);
	let q = $state(page.url.searchParams.get('q') ?? '');
	const category = $derived(page.url.searchParams.get('category') ?? '');
	let seq = 0;
	async function load() {
		const mine = ++seq;
		const r = await api.get<{ items: ShopProduct[] }>('/public/shop/products' + query({ category: category || undefined, q: q.trim() || undefined }));
		if (mine === seq) items = r.items;
	}
	$effect(() => {
		category;
		load().catch(() => (items = []));
	});
	let timer: ReturnType<typeof setTimeout> | undefined;
	function search() {
		clearTimeout(timer);
		timer = setTimeout(load, 250);
	}
	function pick(slug: string) {
		goto(slug ? `/shop?category=${slug}` : '/shop', { replace: true });
	}
	let added = $state<string | null>(null);
	function add(p: ShopProduct) {
		shop.add(p);
		added = p.slug;
		setTimeout(() => added === p.slug && (added = null), 1500);
	}
	const soldOut = (p: ShopProduct) => p.stock !== null && Number(p.stock) <= 0;
</script>

<svelte:head><title>{shop.info?.name ?? 'Shop'}</title></svelte:head>

{#if shop.info}
	<div class="space-y-6">
		<div>
			<h1 class="text-3xl font-semibold tracking-tight text-slate-900">{shop.info.name}</h1>
			{#if shop.info.intro}<p class="mt-2 max-w-2xl whitespace-pre-line text-slate-600">{shop.info.intro}</p>{/if}
		</div>

		<div class="flex flex-wrap items-center gap-2">
			{#if shop.info.categories.length}
				<div class="flex flex-wrap gap-2" role="group" aria-label="Categories">
					<button class="rounded-full px-3.5 py-1.5 text-sm font-medium {category ? 'bg-white text-slate-700 ring-1 ring-slate-200 hover:bg-slate-100' : 'bg-slate-900 text-white'}" aria-pressed={!category} onclick={() => pick('')}>All</button>
					{#each shop.info.categories as c (c.slug)}
						<button class="rounded-full px-3.5 py-1.5 text-sm font-medium {category === c.slug ? 'bg-slate-900 text-white' : 'bg-white text-slate-700 ring-1 ring-slate-200 hover:bg-slate-100'}" aria-pressed={category === c.slug} onclick={() => pick(c.slug)}>{c.name}</button>
					{/each}
				</div>
			{/if}
			<input class="input ml-auto w-full sm:w-64" type="search" placeholder="Search products" aria-label="Search products" bind:value={q} oninput={search} />
		</div>

		{#if items === null}
			<p class="text-sm text-slate-400">Loading…</p>
		{:else if !items.length}
			<div class="rounded-3xl bg-white p-12 text-center text-slate-500 shadow-g1">{q.trim() ? `Nothing matches “${q.trim()}”.` : 'Nothing for sale here yet.'}</div>
		{:else}
			<ul class="grid gap-5 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
				{#each items as p (p.slug)}
					<li class="flex flex-col overflow-hidden rounded-2xl bg-white shadow-g1" data-product={p.name}>
						<a href="/shop/p/{p.slug}" class="block aspect-[4/3] bg-slate-100">
							{#if p.image}<img src={p.image} alt={p.name} class="size-full object-cover" loading="lazy" />{:else}<span class="flex size-full items-center justify-center text-slate-300"><Icon name="store" class="size-10" /></span>{/if}
						</a>
						<div class="flex flex-1 flex-col gap-1 p-4">
							<a href="/shop/p/{p.slug}" class="font-medium text-slate-900 hover:underline">{p.name}</a>
							{#if p.description}<p class="line-clamp-2 text-sm text-slate-500">{p.description}</p>{/if}
							<div class="mt-auto flex items-center gap-2 pt-3">
								<span class="text-lg font-semibold text-slate-900 tabular-nums">{#if p.variants}<span class="text-sm font-normal text-slate-500">from </span>{/if}{money(p.price, shop.info.currency)}</span>
								{#if p.unit !== 'pcs'}<span class="text-xs text-slate-500">/ {p.unit}</span>{/if}
								{#if soldOut(p)}
									<span class="ml-auto text-sm font-medium text-slate-500">Sold out</span>
								{:else if p.variants}
									<a class="btn btn-secondary ml-auto px-3 py-1.5 text-sm" href="/shop/p/{p.slug}">Choose</a>
								{:else}
									<button class="btn btn-primary ml-auto px-3 py-1.5 text-sm" onclick={() => add(p)}>{added === p.slug ? 'Added' : 'Add to cart'}</button>
								{/if}
							</div>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</div>
{/if}
