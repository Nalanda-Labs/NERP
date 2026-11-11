<script lang="ts">
	// Websites: the company's sites.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Site = {
		id: string;
		name: string;
		slug: string;
		pages: number;
		published: number;
		updated_at: string;
		settings: { color: string };
	};
	let items = $state<Site[]>([]);
	let base = $state('');
	onMount(async () => {
		const r = await api.get<{ items: Site[]; base: string }>('/sites');
		items = r.items;
		base = r.base;
	});
	let open = $state(false);
	let form = $state({ name: '', slug: '', template: 'business' });
	const TEMPLATES: [string, string, string][] = [
		['business', 'Company', 'A home page to start from'],
		['sales', 'Sales', 'Offer, prices, products and a request form'],
		['marketing', 'Marketing', 'A landing page with sign-up, and an event page'],
		['recruitment', 'Careers', 'Why join, how we hire, open positions and applying']
	];
	async function importSite(e: Event) {
		const f = (e.currentTarget as HTMLInputElement).files?.[0];
		if (!f) return;
		try {
			const data = JSON.parse(await f.text());
			const slug = prompt('Address for the imported site', `${data.slug ?? 'site'}-imported`);
			if (!slug) return;
			const s = await api.post<{ id: string }>('/sites/import', { data, slug });
			goto(`/sites/${s.id}`);
		} catch (err) {
			toasts.error(err instanceof SyntaxError ? new Error('That is not a site file') : err);
		}
	}
	let touched = false;
	const auto = (n: string) =>
		n
			.toLowerCase()
			.normalize('NFKD')
			.replace(/[^a-z0-9]+/g, '-')
			.replace(/^-|-$/g, '')
			.slice(0, 40);
	async function create(e: SubmitEvent) {
		e.preventDefault();
		try {
			const s = await api.post<{ id: string }>('/sites', form);
			goto(`/sites/${s.id}`);
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>Websites · NERP</title></svelte:head>

<div class="max-w-5xl space-y-4">
	<div class="flex items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Websites</h1>
		<label class="btn btn-secondary cursor-pointer"
			>Import<input
				type="file"
				accept=".json,application/json"
				class="sr-only"
				aria-label="Site file"
				onchange={importSite}
			/></label
		>
		<button class="btn btn-primary" onclick={() => (open = true)}
			><Icon name="plus" class="size-4" /> New website</button
		>
	</div>
	<div class="grid gap-4 sm:grid-cols-2">
		{#each items as s (s.id)}
			<a href="/sites/{s.id}" class="card block overflow-hidden hover:shadow-g2" data-site={s.name}>
				<div class="h-2" style="background:{s.settings.color ?? '#1a73e8'}"></div>
				<div class="p-5">
					<p class="font-semibold text-slate-900">{s.name}</p>
					<p class="text-sm text-slate-500">{base}{s.slug}</p>
					<p class="mt-3 text-xs text-slate-500">
						{s.published} of {s.pages} page{s.pages === 1 ? '' : 's'} published · changed {formatRelative(
							s.updated_at
						)}
					</p>
				</div>
			</a>
		{:else}
			<div class="card p-10 text-center text-slate-500 sm:col-span-2">
				No websites yet. Make a simple site for the company, a product or an event, with forms that
				send leads to the CRM.
			</div>
		{/each}
	</div>
</div>

<Modal bind:open title="New website">
	<form id="site-new" class="space-y-3" onsubmit={create}>
		<div>
			<label class="label" for="st-n">Name</label><input
				id="st-n"
				class="input"
				required
				maxlength="120"
				bind:value={form.name}
				oninput={() => !touched && (form.slug = auto(form.name))}
			/>
		</div>
		<fieldset>
			<legend class="label">Start from</legend>
			<div class="grid gap-2 sm:grid-cols-2">
				{#each TEMPLATES as [k, l, d] (k)}<label
						class="flex items-start gap-2 rounded-lg border border-slate-200 p-2 text-sm"
						><input type="radio" name="tpl" value={k} bind:group={form.template} /><span
							><span class="font-medium">{l}</span><br /><span class="text-xs text-slate-500"
								>{d}</span
							></span
						></label
					>{/each}
			</div>
		</fieldset>
		<div>
			<label class="label" for="st-s">Address</label>
			<div class="flex items-center gap-1 text-sm text-slate-500">
				<span>{base}</span><input
					id="st-s"
					class="input flex-1"
					required
					pattern="[a-z0-9\-]{'{'}2,40{'}'}"
					bind:value={form.slug}
					oninput={() => (touched = true)}
				/>
			</div>
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="site-new">Create</button>
	{/snippet}
</Modal>
