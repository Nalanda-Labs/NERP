<script lang="ts">
	// The customer's order page (the link in their emails): progress, what they ordered, how to pay.
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import { money } from '#lib/crm.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';

	type Order = {
		number: number;
		created_at: string;
		currency: string;
		stage: { name: string; semantics: string };
		steps: { name: string; done: boolean }[];
		rows: { name: string; quantity: string; unit: string; total: string }[];
		totals: { subtotal: string; tax: string; total: string };
		customer: { name: string; email: string; phone: string | null; address: string; note: string | null };
		shop: string;
		payment_instructions: string | null;
	};
	let o = $state<Order | null>(null);
	let missing = $state(false);
	$effect(() => {
		api.get<Order>(`/public/shop/orders/${encodeURIComponent(page.params.token ?? '')}`)
			.then((r) => (o = r))
			.catch(() => (missing = true));
	});
	const placed = $derived(page.url.searchParams.has('placed'));
</script>

<svelte:head><title>{o ? `Order ${o.number}` : 'Your order'}</title><meta name="robots" content="noindex" /></svelte:head>

{#if missing}
	<div class="mx-auto max-w-md rounded-3xl bg-white p-10 text-center shadow-g1">
		<h1 class="text-xl font-semibold text-slate-900">We can't find this order</h1>
		<p class="mt-2 text-sm text-slate-600">The link may be incomplete. Use the link from your confirmation email.</p>
	</div>
{:else if o}
	<div class="mx-auto max-w-3xl space-y-5">
		{#if placed}
			<div class="rounded-2xl bg-emerald-50 px-5 py-4 text-emerald-900" role="status">
				<p class="font-semibold">Thank you! Your order is placed.</p>
				<p class="text-sm">We've emailed a confirmation to {o.customer.email}. Keep it: its link brings you back here.</p>
			</div>
		{/if}
		<div class="flex flex-wrap items-baseline gap-x-4">
			<h1 class="text-3xl font-semibold tracking-tight text-slate-900">Order {o.number}</h1>
			<span class="text-sm text-slate-500">{formatDateTime(o.created_at)}</span>
		</div>

		{#if o.stage.semantics === 'failure'}
			<p class="rounded-2xl bg-red-50 px-5 py-4 font-medium text-red-800" data-order-stage>{o.stage.name}</p>
		{:else}
			<ol class="flex flex-wrap gap-2" aria-label="Progress" data-order-stage={o.stage.name}>
				{#each o.steps as s, i (s.name)}
					<li class="flex items-center gap-2 rounded-full px-3 py-1.5 text-sm font-medium {s.done ? 'bg-brand-600 text-white' : 'bg-white text-slate-500 ring-1 ring-slate-200'}" aria-current={s.name === o.stage.name ? 'step' : undefined}>
						<span class="tabular-nums">{i + 1}</span>{s.name}
					</li>
				{/each}
			</ol>
		{/if}

		{#if o.payment_instructions}
			<section class="rounded-2xl bg-white p-5 shadow-g1" aria-labelledby="pay-h">
				<h2 id="pay-h" class="mb-2 font-semibold text-slate-900">How to pay</h2>
				<p class="whitespace-pre-line text-sm text-slate-700" data-pay>{o.payment_instructions}</p>
			</section>
		{/if}

		<section class="rounded-2xl bg-white shadow-g1" aria-labelledby="items-h">
			<h2 id="items-h" class="border-b border-slate-100 px-5 py-3 font-semibold text-slate-900">What you ordered</h2>
			<ul class="divide-y divide-slate-50">
				{#each o.rows as r, i (i)}
					<li class="flex gap-3 px-5 py-2.5 text-sm">
						<span class="w-14 shrink-0 text-slate-500 tabular-nums">{Number(r.quantity)} ×</span>
						<span class="flex-1 text-slate-800">{r.name}</span>
						<span class="font-medium text-slate-900 tabular-nums">{money(r.total, o.currency)}</span>
					</li>
				{/each}
			</ul>
			<dl class="space-y-1 border-t border-slate-100 px-5 py-3 text-sm">
				{#if Number(o.totals.tax) > 0}<div class="flex justify-between text-slate-500"><dt>Of which tax</dt><dd class="tabular-nums">{money(o.totals.tax, o.currency)}</dd></div>{/if}
				<div class="flex justify-between text-base font-semibold text-slate-900"><dt>Total</dt><dd class="tabular-nums" data-order-total>{money(o.totals.total, o.currency)}</dd></div>
			</dl>
		</section>

		<section class="rounded-2xl bg-white p-5 text-sm shadow-g1" aria-labelledby="to-h">
			<h2 id="to-h" class="mb-2 font-semibold text-slate-900">Delivery</h2>
			<p class="text-slate-800">{o.customer.name}</p>
			<p class="whitespace-pre-line text-slate-600">{o.customer.address}</p>
			{#if o.customer.phone}<p class="text-slate-600">{o.customer.phone}</p>{/if}
			{#if o.customer.note}<p class="mt-2 whitespace-pre-line text-slate-500">Your note: {o.customer.note}</p>{/if}
		</section>
		<a href="/shop" class="inline-block text-sm text-slate-500 hover:text-slate-800">← Back to {o.shop}</a>
	</div>
{:else}
	<p class="text-center text-sm text-slate-400">Loading…</p>
{/if}
