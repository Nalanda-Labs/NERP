<script lang="ts">
	// Shop settings: ways of delivery, coupons, discounts for loyal customers, and the set-up as
	// a file (export, import).
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { currency }: { currency: string } = $props();
	type Way = {
		id: string;
		name: string;
		description: string | null;
		price: string;
		free_from: string | null;
		needs_address: boolean;
		active: boolean;
	};
	type Coupon = {
		id: string;
		code: string;
		kind: 'percent' | 'amount';
		value: string;
		min_total: string | null;
		starts_at: string | null;
		ends_at: string | null;
		max_uses: number | null;
		uses: number;
		once_per_customer: boolean;
		active: boolean;
	};
	let ways = $state<Way[]>([]);
	let coupons = $state<Coupon[]>([]);
	let tiers = $state<{ from: number; percent: number }[]>([]);
	let way = $state({ name: '', price: '', free_from: '', needs_address: true });
	let cp = $state({
		code: '',
		kind: 'percent',
		value: '',
		min_total: '',
		ends: '',
		max_uses: '',
		once: false
	});

	async function load() {
		try {
			[ways, coupons, tiers] = await Promise.all([
				api.get<{ items: Way[] }>('/shop/delivery').then((r) => r.items),
				api.get<{ items: Coupon[] }>('/shop/coupons').then((r) => r.items),
				api
					.get<{ tiers: { from: number; percent: number }[] }>('/shop/loyalty')
					.then((r) => r.tiers)
			]);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	async function addWay(e: SubmitEvent) {
		e.preventDefault();
		try {
			ways = (
				await api.post<{ items: Way[] }>('/shop/delivery', {
					name: way.name,
					price: amount(way.price) ?? '0', free_from: amount(way.free_from),
					needs_address: way.needs_address
				})
			).items;
			way = { name: '', price: '', free_from: '', needs_address: true };
		} catch (err) {
			toasts.error(err);
		}
	}
	async function toggleWay(w: Way) {
		ways = (await api.patch<{ items: Way[] }>(`/shop/delivery/${w.id}`, { active: !w.active }))
			.items;
	}
	async function dropWay(w: Way) {
		if (confirm(`Remove “${w.name}”?`))
			ways = (await api.del<{ items: Way[] }>(`/shop/delivery/${w.id}`)).items;
	}
	async function addCoupon(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.post('/shop/coupons', {
				code: cp.code,
				kind: cp.kind,
				value: amount(cp.value),
				min_total: amount(cp.min_total),
				ends_at: cp.ends ? new Date(`${cp.ends}T23:59:59`).toISOString() : null,
				max_uses: cp.max_uses ? Number(cp.max_uses) : null,
				once_per_customer: cp.once
			});
			cp = {
				code: '',
				kind: 'percent',
				value: '',
				min_total: '',
				ends: '',
				max_uses: '',
				once: false
			};
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function toggleCoupon(c: Coupon) {
		await api.patch(`/shop/coupons/${c.id}`, { active: !c.active }).catch(toasts.error);
		await load();
	}
	async function dropCoupon(c: Coupon) {
		if (!confirm(`Delete coupon ${c.code}?`)) return;
		await api.del(`/shop/coupons/${c.id}`).catch(toasts.error);
		await load();
	}
	async function saveTiers() {
		try {
			tiers = (
				await api.put<{ tiers: typeof tiers }>('/shop/loyalty', {
					tiers: tiers
						.filter((t) => t.percent > 0)
						.map((t) => ({ from: Number(t.from), percent: Number(t.percent) }))
				})
			).tiers;
			toasts.success('Saved');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function exportShop() {
		const data = await api.get('/shop/export');
		const a = document.createElement('a');
		a.href = URL.createObjectURL(
			new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
		);
		a.download = 'shop.nerp-shop.json';
		a.click();
	}
	async function importShop(e: Event) {
		const f = (e.currentTarget as HTMLInputElement).files?.[0];
		if (!f) return;
		try {
			const r = await api.post<{ categories: number; delivery: number; coupons: number }>(
				'/shop/import',
				JSON.parse(await f.text())
			);
			toasts.success(
				`Imported ${r.categories} categories, ${r.delivery} ways of delivery and ${r.coupons} coupons`
			);
			await load();
		} catch (err) {
			toasts.error(err instanceof SyntaxError ? new Error('That is not a shop file') : err);
		}
	}
	const m = (v: string | number) => `${Number(v).toLocaleString()} ${currency}`;
	// Amounts go to the server as text (number fields give numbers, or nothing).
	const amount = (v: unknown) => (v === '' || v === null || v === undefined ? null : String(v));
</script>

<section class="card space-y-3 p-6" aria-label="Delivery">
	<h2 class="font-medium text-slate-900">Delivery</h2>
	<p class="text-sm text-slate-500">
		With ways of delivery, customers choose one at checkout and its price is added to the order.
	</p>
	{#each ways as w (w.id)}
		<div
			class="flex flex-wrap items-center gap-2 text-sm {w.active ? '' : 'text-slate-400'}"
			data-way-row={w.name}
		>
			<span class="flex-1"
				><span class="font-medium">{w.name}</span> · {Number(w.price)
					? m(w.price)
					: 'free'}{w.free_from ? ` · free from ${m(w.free_from)}` : ''}{w.needs_address
					? ''
					: ' · no address'}</span
			>
			<button class="text-brand-600 hover:underline" onclick={() => toggleWay(w)}
				>{w.active ? 'Turn off' : 'Turn on'}</button
			>
			<button
				class="text-slate-400 hover:text-red-600"
				aria-label="Remove {w.name}"
				onclick={() => dropWay(w)}>×</button
			>
		</div>
	{/each}
	<form class="grid gap-2 text-sm sm:grid-cols-5" onsubmit={addWay}>
		<input
			class="input sm:col-span-2"
			required
			maxlength="120"
			placeholder="Courier, Pick-up…"
			aria-label="Way of delivery"
			bind:value={way.name}
		/>
		<input
			class="input"
			type="number"
			min="0"
			step="0.01"
			placeholder="Price"
			aria-label="Delivery price"
			bind:value={way.price}
		/>
		<input
			class="input"
			type="number"
			min="0"
			step="0.01"
			placeholder="Free from"
			aria-label="Free from"
			bind:value={way.free_from}
		/>
		<button class="btn btn-secondary">Add</button>
		<label class="flex items-center gap-2 sm:col-span-5"
			><input type="checkbox" bind:checked={way.needs_address} /> Needs the customer's address</label
		>
	</form>
</section>

<section class="card space-y-3 p-6" aria-label="Coupons">
	<h2 class="font-medium text-slate-900">Coupons</h2>
	{#each coupons as c (c.id)}
		<div
			class="flex flex-wrap items-center gap-2 text-sm {c.active ? '' : 'text-slate-400'}"
			data-coupon-row={c.code}
		>
			<code class="font-semibold">{c.code}</code>
			<span class="flex-1"
				>{c.kind === 'percent' ? `${Number(c.value)}% off` : `${m(c.value)} off`}{c.min_total
					? ` · from ${m(c.min_total)}`
					: ''}{c.ends_at ? ` · until ${formatDate(c.ends_at)}` : ''}{c.once_per_customer
					? ' · once per customer'
					: ''} · used {c.uses}{c.max_uses ? ` of ${c.max_uses}` : ''}</span
			>
			<button class="text-brand-600 hover:underline" onclick={() => toggleCoupon(c)}
				>{c.active ? 'Turn off' : 'Turn on'}</button
			>
			<button
				class="text-slate-400 hover:text-red-600"
				aria-label="Delete {c.code}"
				onclick={() => dropCoupon(c)}>×</button
			>
		</div>
	{/each}
	<form class="grid gap-2 text-sm sm:grid-cols-4" onsubmit={addCoupon}>
		<input
			class="input uppercase"
			required
			maxlength="40"
			placeholder="CODE"
			aria-label="Coupon code"
			bind:value={cp.code}
		/>
		<select class="input" aria-label="Kind of discount" bind:value={cp.kind}
			><option value="percent">% off</option><option value="amount">{currency} off</option></select
		>
		<input
			class="input"
			required
			type="number"
			min="0.01"
			step="0.01"
			placeholder="How much"
			aria-label="Discount"
			bind:value={cp.value}
		/>
		<input
			class="input"
			type="number"
			min="0"
			step="0.01"
			placeholder="Minimum order"
			aria-label="Minimum order"
			bind:value={cp.min_total}
		/>
		<input class="input" type="date" aria-label="Valid until" bind:value={cp.ends} />
		<input
			class="input"
			type="number"
			min="1"
			placeholder="Uses (any)"
			aria-label="Number of uses"
			bind:value={cp.max_uses}
		/>
		<label class="flex items-center gap-2"
			><input type="checkbox" bind:checked={cp.once} /> Once per customer</label
		>
		<button class="btn btn-secondary">Add coupon</button>
	</form>
</section>

<section class="card space-y-3 p-6" aria-label="Loyal customers">
	<h2 class="font-medium text-slate-900">Discounts for loyal customers</h2>
	<p class="text-sm text-slate-500">
		By what a customer (by email) bought before. With a coupon too, the larger discount counts.
	</p>
	{#each tiers as t, i (i)}
		<div class="flex items-center gap-2 text-sm">
			From <input
				class="input w-28"
				type="number"
				min="0"
				aria-label="Bought before"
				bind:value={t.from}
			/>
			{currency}:
			<input
				class="input w-20"
				type="number"
				min="0.1"
				max="100"
				step="0.1"
				aria-label="Percent off"
				bind:value={t.percent}
			/>
			% off
			<button
				class="text-slate-400 hover:text-red-600"
				aria-label="Remove level"
				onclick={() => (tiers = tiers.filter((_, j) => j !== i))}>×</button
			>
		</div>
	{/each}
	<div class="flex gap-2">
		<button class="btn btn-ghost" onclick={() => (tiers = [...tiers, { from: 0, percent: 5 }])}
			>+ Level</button
		><button class="btn btn-secondary" onclick={saveTiers}>Save</button>
	</div>
</section>

<section class="card flex flex-wrap items-center gap-2 p-6 text-sm" aria-label="Set-up as a file">
	<span class="flex-1 text-slate-600"
		>The shop's set-up (texts, categories, delivery, coupons, loyalty; not the products) as a file,
		to copy it to another NERP.</span
	>
	<button class="btn btn-secondary" onclick={exportShop}>Export</button>
	<label class="btn btn-secondary cursor-pointer"
		>Import<input
			type="file"
			accept=".json,application/json"
			class="sr-only"
			aria-label="Shop file"
			onchange={importShop}
		/></label
	>
</section>
