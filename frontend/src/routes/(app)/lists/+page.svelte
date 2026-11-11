<script lang="ts">
	// Lists: the company's own small databases, with business processes.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type L = { id: string; name: string; description: string | null; items: number; level: string; to_all: boolean };
	let items = $state<L[]>([]);
	let canCreate = $state(false);
	onMount(async () => {
		const r = await api.get<{ items: L[]; can_create: boolean }>('/lists');
		items = r.items;
		canCreate = r.can_create;
	});
	// Ready-made starting points.
	const STARTS: Record<string, { label: string; fields: { label: string; kind: string; options?: string[]; required?: boolean }[] }> = {
		empty: { label: 'Empty', fields: [] },
		purchase: { label: 'Purchase requests', fields: [{ label: 'Amount', kind: 'money', required: true }, { label: 'Supplier', kind: 'text' }, { label: 'Needed by', kind: 'date' }, { label: 'Reason', kind: 'textarea' }, { label: 'Status', kind: 'select', options: ['New', 'Approved', 'Rejected', 'Ordered'] }] },
		assets: { label: 'Company assets', fields: [{ label: 'Serial number', kind: 'text' }, { label: 'Holder', kind: 'user' }, { label: 'Bought on', kind: 'date' }, { label: 'Price', kind: 'money' }] },
		contracts: { label: 'Contracts', fields: [{ label: 'Client', kind: 'crm' }, { label: 'Signed on', kind: 'date' }, { label: 'Ends on', kind: 'date' }, { label: 'Document', kind: 'file' }] }
	};
	let open = $state(false);
	let form = $state({ name: '', description: '', start: 'empty' });
	async function create(e: SubmitEvent) {
		e.preventDefault();
		try {
			const l = await api.post<{ id: string }>('/lists', { name: form.name, description: form.description || null, fields: STARTS[form.start].fields.map((f) => ({ ...f, options: f.options ?? [], required: f.required ?? false })) });
			goto(`/lists/${l.id}?tab=settings`);
		} catch (err) {
			toasts.error(err);
		}
	}

	// Ready-made solutions: a list with its fields and approval process in one go.
	type Solution = { key: string; name: string; description: string; roles: string[] };
	let solutions = $state<Solution[]>([]);
	let galleryOpen = $state(false);
	let picked = $state<Solution | null>(null);
	let people = $state<Record<string, PickedUser[]>>({ finance: [], hr: [], office: [] });
	const ROLE: Record<string, string> = { finance: 'Finance (approves larger sums, pays)', hr: 'HR (approves hiring, recruits)', office: 'Office (books and orders)' };
	async function openGallery() {
		solutions = (await api.get<{ items: Solution[] }>('/lists/solutions')).items;
		picked = null;
		people = { finance: [], hr: [], office: [] };
		galleryOpen = true;
	}
	async function install() {
		try {
			const r = await api.post<{ id: string }>(`/lists/solutions/${picked!.key}`, Object.fromEntries(Object.entries(people).map(([k, v]) => [k, v.map((p) => p.id)])));
			goto(`/lists/${r.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Lists · NERP</title></svelte:head>

<div class="max-w-5xl space-y-4">
	<div class="flex items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Lists</h1>
		<a class="btn btn-secondary" href="/approvals">My approvals</a>
		{#if canCreate}<button class="btn btn-secondary" onclick={openGallery}>Ready-made</button><button class="btn btn-primary" onclick={() => (open = true)}><Icon name="plus" class="size-4" /> New list</button>{/if}
	</div>
	<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
		{#each items as l (l.id)}
			<a href="/lists/{l.id}" class="card block p-5 hover:shadow-g2" data-list={l.name}>
				<p class="font-semibold text-slate-900">{l.name}</p>
				{#if l.description}<p class="mt-1 line-clamp-2 text-sm text-slate-600">{l.description}</p>{/if}
				<p class="mt-3 text-xs text-slate-500">{l.items} item{l.items === 1 ? '' : 's'} · you can {l.level === 'manage' ? 'manage it' : l.level === 'add' ? 'add items' : 'read it'}</p>
			</a>
		{:else}
			<div class="card p-10 text-center text-slate-500 sm:col-span-2 lg:col-span-3">No lists yet.{canCreate ? ' Make one for purchase requests, assets, contracts… and add an approval process.' : ''}</div>
		{/each}
	</div>
</div>

<Modal bind:open title="New list">
	<form id="list-new" class="space-y-3" onsubmit={create}>
		<div><label class="label" for="ln-n">Name</label><input id="ln-n" class="input" required maxlength="120" bind:value={form.name} /></div>
		<div><label class="label" for="ln-d">What it's for</label><input id="ln-d" class="input" bind:value={form.description} /></div>
		<div><label class="label" for="ln-s">Start from</label><select id="ln-s" class="input" bind:value={form.start}>{#each Object.entries(STARTS) as [k, v] (k)}<option value={k}>{v.label}</option>{/each}</select></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="list-new">Create</button>
	{/snippet}
</Modal>

<Modal bind:open={galleryOpen} title="Ready-made solutions" size="lg">
	<div class="space-y-3 text-sm">
		<p class="text-slate-500">A list with its fields and its approval process, ready to use. Change anything afterwards.</p>
		<div class="grid gap-2 sm:grid-cols-2">
			{#each solutions as sol (sol.key)}
				<button class="rounded-xl border p-3 text-left {picked?.key === sol.key ? 'border-brand-500 bg-brand-50' : 'border-slate-200 hover:border-slate-300'}" onclick={() => (picked = sol)} data-solution={sol.key}>
					<span class="font-medium text-slate-900">{sol.name}</span><br /><span class="text-xs text-slate-500">{sol.description}</span>
				</button>
			{/each}
		</div>
		{#if picked?.roles.length}
			<div class="space-y-2 border-t border-slate-100 pt-3">
				<p class="text-xs text-slate-500">Who does what (empty: the author's department head):</p>
				{#each picked.roles as r (r)}<div><span class="label">{ROLE[r]}</span><MultiUserPicker bind:value={people[r]} placeholder="Add people…" /></div>{/each}
			</div>
		{/if}
	</div>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (galleryOpen = false)}>Cancel</button>
		<button class="btn btn-primary" disabled={!picked} onclick={install}>Create</button>
	{/snippet}
</Modal>
