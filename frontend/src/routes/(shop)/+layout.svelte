<script lang="ts">
	// The public online shop: its own header with the cart, no app chrome, no sign-in.
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import type { LayoutProps } from './$types';
	import Icon from '#lib/components/Icon.svelte';
	import { shop } from './shop.svelte.ts';

	let { children }: LayoutProps = $props();
	onMount(() => shop.load());
	// Customers keep their order links while the shop is closed.
	const orderPage = $derived(page.url.pathname.startsWith('/shop/order/'));
</script>

<div class="flex min-h-screen flex-col bg-slate-50">
	<header class="sticky top-0 z-10 border-b border-slate-200 bg-white/95 backdrop-blur">
		<div class="mx-auto flex h-16 max-w-6xl items-center gap-4 px-4">
			<a href="/shop" class="flex min-w-0 items-center gap-2.5 font-semibold text-slate-900">
				<span class="flex size-9 shrink-0 items-center justify-center rounded-xl bg-brand-600 text-white"><Icon name="store" class="size-5" /></span>
				<span class="truncate">{shop.info?.name ?? ''}</span>
			</a>
			{#if shop.info}
				<a href="/shop/cart" class="btn btn-secondary relative ml-auto" aria-label="Cart, {shop.count} item{shop.count === 1 ? '' : 's'}" data-cart-count={shop.count}>
					<Icon name="cart" class="size-5" />
					<span class="hidden sm:inline">Cart</span>
					{#if shop.count}<span class="rounded-full bg-brand-600 px-2 py-0.5 text-xs font-semibold text-white tabular-nums">{shop.count}</span>{/if}
				</a>
			{/if}
		</div>
	</header>

	<main class="mx-auto w-full max-w-6xl flex-1 px-4 py-8">
		{#if shop.closed && !orderPage}
			<div class="mx-auto max-w-md rounded-3xl bg-white p-10 text-center shadow-g1">
				<h1 class="text-xl font-semibold text-slate-900">The shop is closed</h1>
				<p class="mt-2 text-sm text-slate-600">Please come back later. If you placed an order, the link in your confirmation email still works.</p>
			</div>
		{:else if shop.info || orderPage}
			{@render children()}
		{:else}
			<p class="text-center text-sm text-slate-400">Loading…</p>
		{/if}
	</main>

	{#if shop.info}
		<footer class="border-t border-slate-200 bg-white">
			<div class="mx-auto flex max-w-6xl flex-wrap gap-x-6 gap-y-1 px-4 py-6 text-sm text-slate-500">
				<span>© {new Date().getFullYear()} {shop.info.company ?? shop.info.name}</span>
				{#if shop.info.terms}<a class="hover:text-slate-800 hover:underline" href="/shop/cart#terms">Terms of sale</a>{/if}
			</div>
		</footer>
	{/if}
</div>
