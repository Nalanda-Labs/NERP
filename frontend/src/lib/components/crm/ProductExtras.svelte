<script lang="ts">
	// The catalog part of the product form: property values, extra prices, and variants.
	// The page calls `save()` after saving the product itself.
	import { api } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { money } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { Prop } from './CatalogSettings.svelte';

	type Variant = { id: string; name: string; sku: string | null; price: string; currency: string; stock: string | null; is_active: boolean; props: Record<string, string> };
	type Details = {
		props: Record<string, string | number>;
		prices: Record<string, string>;
		variants: Variant[];
		parent: { id: string; name: string } | null;
		definitions: Prop[];
		price_types: { id: string; name: string }[];
	};
	let { productId, onopen, onchanged }: { productId: string; onopen?: (id: string) => void; onchanged?: () => void } = $props();

	let d = $state<Details | null>(null);
	let values = $state<Record<string, string>>({});
	let prices = $state<Record<string, string>>({});
	function take(x: Details) {
		d = x;
		values = Object.fromEntries(x.definitions.filter((p) => !p.is_variant).map((p) => [p.id, x.props[p.id] === undefined ? '' : String(x.props[p.id])]));
		prices = Object.fromEntries(x.price_types.map((t) => [t.id, x.prices[t.id] === undefined ? '' : String(Number(x.prices[t.id]))]));
	}
	$effect(() => {
		const id = productId;
		d = null;
		api.get<Details>(`/crm/products/${id}/details`).then(take).catch(() => {});
	});
	const own = $derived(d?.definitions.filter((p) => !p.is_variant) ?? []);
	const axes = $derived(d?.definitions.filter((p) => p.is_variant) ?? []);

	/** Saves property values and prices. */
	export async function save() {
		if (!d) return;
		if (own.length) await api.put(`/crm/products/${productId}/props`, { props: values });
		if (d.price_types.length) await api.put(`/crm/products/${productId}/prices`, { prices: Object.fromEntries(Object.entries(prices).map(([k, v]) => [k, v === '' || v === null ? null : String(v)])) });
	}

	// Making variants: pick values of each variant property.
	let picking = $state(false);
	let chosen = $state<Record<string, string[]>>({});
	const count = $derived(Object.values(chosen).filter((v) => v.length).reduce((a, v) => a * v.length, 1));
	function toggle(prop: string, v: string) {
		const cur = chosen[prop] ?? [];
		chosen[prop] = cur.includes(v) ? cur.filter((x) => x !== v) : [...cur, v];
	}
	async function make() {
		try {
			const r = await api.post<Details & { made: number }>(`/crm/products/${productId}/variants`, { values: Object.fromEntries(Object.entries(chosen).filter(([, v]) => v.length)) });
			take(r);
			picking = false;
			chosen = {};
			toasts.success(r.made ? `${r.made} variant${r.made === 1 ? '' : 's'} made` : 'Those variants already exist');
			onchanged?.();
		} catch (e) {
			toasts.error(e);
		}
	}
	const label = (v: Variant) => axes.map((a) => v.props[a.id]).filter(Boolean).join(' · ');
</script>

{#if d}
	{#if d.parent}
		<p class="rounded-xl bg-slate-50 px-3 py-2 text-sm text-slate-600 sm:col-span-2">A variant of <button type="button" class="font-medium text-brand-600 hover:underline" onclick={() => onopen?.(d!.parent!.id)}>{d.parent.name}</button>: {axes.map((a) => `${a.name} ${d!.props[a.id] ?? '—'}`).join(', ')}.</p>
	{/if}
	{#if own.length}
		<fieldset class="grid gap-3 rounded-xl border border-slate-200 p-4 sm:col-span-2 sm:grid-cols-2">
			<legend class="px-1 text-sm font-medium text-slate-800">Properties</legend>
			{#each own as p (p.id)}
				<div>
					<label class="label" for="pp-{p.id}">{p.name}</label>
					{#if p.kind === 'list'}
						<select id="pp-{p.id}" class="input" bind:value={values[p.id]}><option value="">—</option>{#each p.options as o (o)}<option>{o}</option>{/each}</select>
					{:else}
						<input id="pp-{p.id}" class="input" type={p.kind === 'number' ? 'number' : 'text'} step="any" bind:value={values[p.id]} />
					{/if}
				</div>
			{/each}
		</fieldset>
	{/if}
	{#if d.price_types.length}
		<fieldset class="grid gap-3 rounded-xl border border-slate-200 p-4 sm:col-span-2 sm:grid-cols-2">
			<legend class="px-1 text-sm font-medium text-slate-800">Other prices</legend>
			{#each d.price_types as t (t.id)}
				<div><label class="label" for="pt-{t.id}">{t.name}</label><input id="pt-{t.id}" class="input" type="number" min="0" step="0.01" placeholder="Base price" bind:value={prices[t.id]} /></div>
			{/each}
		</fieldset>
	{/if}
	{#if !d.parent && axes.length}
		<fieldset class="rounded-xl border border-slate-200 p-4 sm:col-span-2">
			<legend class="px-1 text-sm font-medium text-slate-800">Variants</legend>
			{#if d.variants.length}
				<ul class="mb-3 divide-y divide-slate-100 text-sm">
					{#each d.variants as v (v.id)}
						<li class="flex items-center gap-3 py-1.5" data-variant={label(v)}>
							<button type="button" class="flex-1 text-left font-medium text-brand-600 hover:underline {v.is_active ? '' : 'opacity-50'}" onclick={() => onopen?.(v.id)}>{label(v)}</button>
							<span class="text-xs text-slate-500">{v.sku ?? ''}</span>
							<span class="w-24 text-right tabular-nums">{money(v.price, v.currency)}</span>
							<span class="w-20 text-right text-slate-500 tabular-nums">{v.stock === null ? '' : `${Number(v.stock)} left`}</span>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="mb-3 text-sm text-slate-500">Sell this in several versions (sizes, colours…): each variant gets its own SKU, price and stock.</p>
			{/if}
			<button type="button" class="btn btn-secondary" onclick={() => (picking = true)}>Make variants</button>
		</fieldset>
	{/if}
{/if}

<Modal bind:open={picking} title="Make variants">
	<p class="mb-4 text-sm text-slate-500">A variant is made for every combination of the values you pick. Ones that exist already are skipped. They copy this product's price, tax, unit, description and shop settings.</p>
	<div class="space-y-4">
		{#each axes as a (a.id)}
			<div>
				<p class="label">{a.name}</p>
				<div class="flex flex-wrap gap-1.5">
					{#each a.options as o (o)}
						<button type="button" class="rounded-lg border px-3 py-1.5 text-sm font-medium {chosen[a.id]?.includes(o) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600 hover:bg-slate-50'}" aria-pressed={chosen[a.id]?.includes(o) ?? false} onclick={() => toggle(a.id, o)}>{o}</button>
					{/each}
				</div>
			</div>
		{/each}
	</div>
	{#snippet footer()}
		<button type="button" class="btn btn-secondary" onclick={() => (picking = false)}>Cancel</button>
		<button type="button" class="btn btn-primary" disabled={!Object.values(chosen).some((v) => v.length)} onclick={make}>Make {Object.values(chosen).some((v) => v.length) ? count : ''} variant{count === 1 ? '' : 's'}</button>
	{/snippet}
</Modal>
