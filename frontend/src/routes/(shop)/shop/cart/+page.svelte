<script lang="ts">
	// The cart and checkout: change quantities, enter delivery details, place the order.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { money } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { shop, type ShopProduct } from '../../shop.svelte.ts';

	let gone = $state<string[]>([]);
	// Prices and stock may have changed since the products went in the cart.
	onMount(async () => {
		if (!shop.cart.length) return;
		try {
			const r = await api.get<{ items: ShopProduct[] }>('/public/shop/products');
			const bySlug = new Map(r.items.map((p) => [p.slug, p]));
			gone = shop.cart.filter((l) => !bySlug.has(l.slug)).map((l) => l.name);
			for (const l of [...shop.cart]) {
				const p = bySlug.get(l.slug);
				if (!p) shop.set(l.slug, 0);
				else {
					l.price = p.price;
					l.name = p.name;
					l.image = p.image;
					l.stock = p.stock === null ? null : Number(p.stock);
					shop.set(l.slug, Math.min(l.quantity, shop.max(l.stock)));
				}
			}
		} catch {
			/* the shop may have closed: the layout says so */
		}
	});

	let form = $state({ name: '', email: '', phone: '', address: '', note: '', accept_terms: false, website: '' });
	let busy = $state(false);
	let error = $state('');
	async function place(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			const r = await api.post<{ token: string; number: number }>('/public/shop/checkout', { ...form, items: shop.cart.map((l) => ({ slug: l.slug, quantity: l.quantity })) });
			shop.clear();
			goto(`/shop/order/${r.token}?placed=1`);
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>Your cart · {shop.info?.name ?? 'Shop'}</title></svelte:head>

{#if shop.info}
	<h1 class="mb-6 text-3xl font-semibold tracking-tight text-slate-900">Your cart</h1>
	{#if gone.length}
		<p class="mb-4 rounded-xl bg-amber-50 px-4 py-3 text-sm text-amber-900" role="status">No longer sold, so taken out of your cart: {gone.join(', ')}.</p>
	{/if}
	{#if !shop.cart.length}
		<div class="rounded-3xl bg-white p-12 text-center shadow-g1">
			<p class="text-slate-600">Your cart is empty.</p>
			<a class="btn btn-primary mt-6" href="/shop">Browse products</a>
		</div>
	{:else}
		<div class="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_24rem]">
			<section class="rounded-2xl bg-white shadow-g1" aria-label="Products in your cart">
				<ul class="divide-y divide-slate-100">
					{#each shop.cart as l (l.slug)}
						<li class="flex flex-wrap items-center gap-x-4 gap-y-3 p-4" data-line={l.name}>
							<a href="/shop/p/{l.slug}" class="size-16 shrink-0 overflow-hidden rounded-xl bg-slate-100">
								{#if l.image}<img src={l.image} alt="" class="size-full object-cover" />{:else}<span class="flex size-full items-center justify-center text-slate-300"><Icon name="store" class="size-6" /></span>{/if}
							</a>
							<div class="min-w-0 flex-1 basis-40">
								<a href="/shop/p/{l.slug}" class="font-medium break-words text-slate-900 hover:underline">{l.name}</a>
								<p class="text-sm text-slate-500 tabular-nums">{money(l.price, shop.info.currency)}{l.unit !== 'pcs' ? ` / ${l.unit}` : ''}</p>
								{#if l.stock !== null && l.quantity >= l.stock}<p class="text-xs text-amber-800">That's all we have.</p>{/if}
							</div>
							<div class="ml-auto flex items-center gap-3">
								<input class="input w-20" type="number" min="1" max={shop.max(l.stock)} step="1" value={l.quantity} aria-label="Quantity of {l.name}" onchange={(e) => shop.set(l.slug, Number(e.currentTarget.value))} />
								<span class="w-24 text-right font-medium text-slate-900 tabular-nums">{money(Number(l.price) * l.quantity, shop.info.currency)}</span>
								<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Remove {l.name}" onclick={() => shop.set(l.slug, 0)}><Icon name="trash" class="size-4" /></button>
							</div>
						</li>
					{/each}
				</ul>
				<div class="flex items-baseline justify-between border-t border-slate-100 px-4 py-4">
					<span class="text-slate-600">Total <span class="text-xs text-slate-400">(tax included)</span></span>
					<span class="text-xl font-semibold text-slate-900 tabular-nums" data-cart-total>{money(shop.total, shop.info.currency)}</span>
				</div>
			</section>

			<form class="space-y-4 rounded-2xl bg-white p-5 shadow-g1" onsubmit={place} aria-labelledby="co-h">
				<h2 id="co-h" class="text-lg font-semibold text-slate-900">Checkout</h2>
				<div><label class="label" for="co-name">Full name</label><input id="co-name" class="input" required maxlength="200" autocomplete="name" bind:value={form.name} /></div>
				<div><label class="label" for="co-email">Email</label><input id="co-email" class="input" type="email" required maxlength="254" autocomplete="email" bind:value={form.email} /></div>
				<div><label class="label" for="co-phone">Phone <span class="font-normal text-slate-400">(optional)</span></label><input id="co-phone" class="input" type="tel" maxlength="40" autocomplete="tel" bind:value={form.phone} /></div>
				<div><label class="label" for="co-addr">Delivery address</label><textarea id="co-addr" class="input" rows="3" required maxlength="1000" autocomplete="street-address" bind:value={form.address}></textarea></div>
				<div><label class="label" for="co-note">Note <span class="font-normal text-slate-400">(optional)</span></label><textarea id="co-note" class="input" rows="2" maxlength="2000" bind:value={form.note}></textarea></div>
				<!-- Left empty by people; bots fill it. -->
				<input class="hidden" tabindex="-1" autocomplete="off" aria-hidden="true" name="website" bind:value={form.website} />
				{#if shop.info.payment_instructions}
					<p class="rounded-xl bg-slate-50 px-3 py-2.5 text-sm text-slate-600">You pay after ordering: we email you the payment details.</p>
				{/if}
				{#if shop.info.terms}
					<details id="terms" class="rounded-xl border border-slate-200 px-3 py-2 text-sm">
						<summary class="cursor-pointer font-medium text-slate-700">Terms of sale</summary>
						<p class="mt-2 max-h-60 overflow-y-auto whitespace-pre-line text-slate-600">{shop.info.terms}</p>
					</details>
					<label class="flex items-start gap-2 text-sm text-slate-700"><input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" required bind:checked={form.accept_terms} /> I accept the terms of sale</label>
				{/if}
				{#if error}<p class="text-sm text-red-700" role="alert">{error}</p>{/if}
				<button class="btn btn-primary w-full justify-center py-2.5" disabled={busy}>{busy ? 'Placing your order…' : `Place order · ${money(shop.total, shop.info.currency)}`}</button>
			</form>
		</div>
	{/if}
{/if}
