<script lang="ts">
	// BI dashboards: everyone sees those shared with them; BI builders see and make them all.
	import { goto } from '$app/navigation';
	import { api, query } from '#lib/api.ts';
	import { formatRelative } from '#lib/format.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type Item = {
		id: string;
		title: string;
		description: string | null;
		owner: string | null;
		updated_at: string;
		charts: number;
		public: boolean;
	};
	let q = $state('');
	let items = $state<Item[] | null>(null);
	let canManage = $state(false);
	let newOpen = $state(false);
	let title = $state('');
	let description = $state('');

	$effect(() => {
		const term = q;
		api
			.get<{ items: Item[]; can_manage: boolean }>(
				'/bi/dashboards' + query({ q: term || undefined })
			)
			.then((r) => ((items = r.items), (canManage = r.can_manage)))
			.catch(toasts.error);
	});
	async function create(e: SubmitEvent) {
		e.preventDefault();
		try {
			const d = await api.post<{ id: string }>('/bi/dashboards', { title, description });
			goto(`/bi/${d.id}?edit=1`);
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>BI dashboards · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="mr-2 text-2xl font-semibold text-slate-900">BI dashboards</h1>
		<input
			class="input ml-auto w-64"
			type="search"
			placeholder="Find by name or owner…"
			aria-label="Find a dashboard"
			bind:value={q}
		/>
		{#if canManage}
			<a class="btn btn-secondary" href="/bi/datasets">Datasets</a>
			<button
				class="btn btn-primary"
				onclick={() => ((title = ''), (description = ''), (newOpen = true))}
				><Icon name="plus" class="size-4" /> New dashboard</button
			>
		{/if}
	</div>
	{#if items === null}
		<p class="text-sm text-slate-400">Loading…</p>
	{:else if items.length}
		<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
			{#each items as d (d.id)}
				<a class="card hover:border-brand-300 block p-4" href="/bi/{d.id}" data-dashboard={d.title}>
					<p class="font-medium text-slate-900">
						{d.title}{#if d.public}<span class="badge ml-2 bg-amber-50 text-amber-800"
								>Public link</span
							>{/if}
					</p>
					{#if d.description}<p class="mt-1 line-clamp-2 text-sm text-slate-600">
							{d.description}
						</p>{/if}
					<p class="mt-1 text-xs text-slate-500">
						{d.charts} chart{d.charts === 1 ? '' : 's'} · {d.owner ?? '—'} · changed {formatRelative(
							d.updated_at
						)}
					</p>
				</a>
			{/each}
		</div>
	{:else}
		<p class="text-sm text-slate-500">
			{q
				? 'Nothing found.'
				: canManage
					? 'No dashboards yet.'
					: 'No dashboards are shared with you yet.'}
		</p>
	{/if}
</div>

<Modal bind:open={newOpen} title="New dashboard">
	<form id="bi-new" class="space-y-3 text-sm" onsubmit={create}>
		<div>
			<label class="label" for="bn-title">Name</label><input
				id="bn-title"
				class="input"
				required
				maxlength="200"
				bind:value={title}
			/>
		</div>
		<div>
			<label class="label" for="bn-desc">What it shows (optional)</label><textarea
				id="bn-desc"
				class="input"
				rows="2"
				bind:value={description}></textarea>
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (newOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="bi-new">Create</button>
	{/snippet}
</Modal>
