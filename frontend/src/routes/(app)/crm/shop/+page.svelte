<script lang="ts">
	// CRM → Online shop: open or close the shop, and how it sells (currency, who handles
	// orders, how customers pay, terms).
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import ShopExtras from '#lib/components/shop/ShopExtras.svelte';
	import { crm } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Picked = { id: string; name: string; avatar_file_id: string | null };
	type Settings = {
		enabled: boolean;
		name: string;
		intro: string | null;
		currency: string;
		handlers: string[];
		payment_instructions: string | null;
		terms: string | null;
		warehouse_id: string | null;
	};
	type View = {
		settings: Settings;
		handlers: Picked[];
		shop_url: string;
		listed: number;
		other_currency: number;
	};

	let view = $state<View | null>(null);
	let form = $state({
		name: '',
		intro: '',
		currency: '',
		payment_instructions: '',
		terms: '',
		warehouse_id: null as string | null
	});
	let warehouses = $state<{ id: string; name: string; is_default: boolean; is_active: boolean }[]>(
		[]
	);
	let handlers = $state<Picked[]>([]);
	let busy = $state(false);

	function take(v: View) {
		view = v;
		const s = v.settings;
		form = {
			name: s.name,
			intro: s.intro ?? '',
			currency: s.currency,
			payment_instructions: s.payment_instructions ?? '',
			terms: s.terms ?? '',
			warehouse_id: s.warehouse_id
		};
		handlers = v.handlers;
	}
	onMount(async () => {
		await crm.load();
		try {
			take(await api.get<View>('/shop/settings'));
			warehouses = (
				await api.get<{ items: typeof warehouses }>('/inventory/warehouses')
			).items.filter((w) => w.is_active);
		} catch (e) {
			toasts.error(e);
		}
	});
	async function save(extra: Partial<Settings> = {}) {
		busy = true;
		try {
			take(
				await api.patch<View>('/shop/settings', {
					...form,
					handlers: handlers.map((h) => h.id),
					...extra
				})
			);
			toasts.success(
				extra.enabled === true
					? 'The shop is open'
					: extra.enabled === false
						? 'The shop is closed'
						: 'Saved'
			);
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	function submit(e: SubmitEvent) {
		e.preventDefault();
		save();
	}
	const orderType = $derived(crm.config?.types.find((t) => t.kind === 'order'));
</script>

<svelte:head><title>Online shop · NERP</title></svelte:head>

{#if view}
	<div class="max-w-3xl space-y-5">
		<div class="flex flex-wrap items-start gap-3">
			<div class="min-w-0 flex-1">
				<h1 class="text-2xl font-semibold text-slate-900">Online shop</h1>
				<p class="mt-1 text-sm text-slate-500">
					Sells the products marked “Sell in the online shop”. Orders arrive as <a
						class="text-brand-600 hover:underline"
						href="/crm/{orderType?.code ?? 'order'}">{orderType?.name_plural ?? 'Orders'}</a
					> in the CRM.
				</p>
			</div>
			{#if view.settings.enabled}
				<button
					class="btn btn-secondary"
					disabled={busy}
					onclick={() =>
						confirm(
							'Close the shop? Its pages will say it is closed; customers keep their order links.'
						) && save({ enabled: false })}>Close the shop</button
				>
			{:else}
				<button class="btn btn-primary" disabled={busy} onclick={() => save({ enabled: true })}
					>Open the shop</button
				>
			{/if}
		</div>

		<div class="card flex flex-wrap items-center gap-x-6 gap-y-2 p-5 text-sm" data-shop-status>
			<span class="flex items-center gap-2">
				<span
					class="size-2.5 rounded-full {view.settings.enabled ? 'bg-emerald-500' : 'bg-slate-300'}"
				></span>
				<span class="font-medium text-slate-900">{view.settings.enabled ? 'Open' : 'Closed'}</span>
			</span>
			<a class="text-brand-600 hover:underline" href={view.shop_url} target="_blank" rel="noopener"
				>{view.shop_url}</a
			>
			<span class="text-slate-600"
				><a class="hover:underline" href="/crm/products"
					>{view.listed} product{view.listed === 1 ? '' : 's'}</a
				> listed</span
			>
			{#if view.other_currency}<span class="text-amber-800"
					>{view.other_currency} in another currency, not listed</span
				>{/if}
		</div>

		<form class="card space-y-4 p-6" onsubmit={submit}>
			<div class="grid gap-4 sm:grid-cols-[1fr_10rem]">
				<div>
					<label class="label" for="sh-name">Shop name</label><input
						id="sh-name"
						class="input"
						required
						maxlength="120"
						bind:value={form.name}
					/>
				</div>
				<div>
					<label class="label" for="sh-cur">Currency</label>
					<select id="sh-cur" class="input" bind:value={form.currency}
						>{#each crm.config?.currencies ?? [] as c (c.code)}<option>{c.code}</option
							>{/each}</select
					>
				</div>
			</div>
			<div>
				<label class="label" for="sh-wh">Sells from</label>
				<select id="sh-wh" class="input" bind:value={form.warehouse_id}>
					<option value={null}
						>The default warehouse{warehouses.find((w) => w.is_default)
							? ` (${warehouses.find((w) => w.is_default)!.name})`
							: ''}</option
					>
					{#each warehouses as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
				</select>
				<p class="mt-1 text-xs text-slate-500">
					The shop shows and sells what this warehouse holds; cancelled orders return there.
				</p>
			</div>
			<div>
				<label class="label" for="sh-intro">Introduction</label><textarea
					id="sh-intro"
					class="input"
					rows="2"
					maxlength="2000"
					placeholder="Shown at the top of the shop"
					bind:value={form.intro}></textarea>
			</div>
			<div>
				<span class="label">Who handles orders</span>
				<MultiUserPicker bind:value={handlers} placeholder="Add an employee…" />
				<p class="mt-1 text-xs text-slate-500">
					New orders go to them in turn, and they get a notification. Nobody: the order has no
					responsible person.
				</p>
			</div>
			<div>
				<label class="label" for="sh-pay">How customers pay</label>
				<textarea
					id="sh-pay"
					class="input font-mono text-xs"
					rows="5"
					maxlength="4000"
					placeholder={'Transfer {{total}} to IBAN … with "Order {{order}}" as the reference.'}
					bind:value={form.payment_instructions}></textarea>
				<p class="mt-1 text-xs text-slate-500">
					Shown after checkout and in the confirmation email. <code>{'{{order}}'}</code> becomes the
					order number and <code>{'{{total}}'}</code> the amount. Move the order to “Paid” when the money
					arrives.
				</p>
			</div>
			<div>
				<label class="label" for="sh-terms">Terms of sale</label>
				<textarea
					id="sh-terms"
					class="input"
					rows="5"
					maxlength="20000"
					placeholder="Delivery, returns, warranty…"
					bind:value={form.terms}></textarea>
				<p class="mt-1 text-xs text-slate-500">
					When filled in, customers must accept them to order.
				</p>
			</div>
			<div class="flex justify-end">
				<button class="btn btn-primary" disabled={busy}>Save</button>
			</div>
		</form>
		<ShopExtras currency={form.currency || 'EUR'} />
	</div>
{:else}
	<div class="card p-10 text-center text-slate-400">Loading…</div>
{/if}
