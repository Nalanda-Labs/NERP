<script lang="ts">
	// CRM settings → Record types: smart processes (admin-defined record types such as
	// Contracts or Shipments) on top of leads, deals, contacts and companies.
	import { api } from '#lib/api.ts';
	import { crm, type CrmType } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon, { type IconName } from '../Icon.svelte';
	import Modal from '../Modal.svelte';

	const ICONS: IconName[] = ['folder', 'file', 'briefcase', 'calendar', 'clock', 'list', 'kanban', 'globe', 'building', 'user', 'send', 'funnel', 'key', 'cpu', 'mail'];
	const types = $derived(crm.config?.types ?? []);

	type Draft = { code: string | null; kind: string | null; name: string; name_plural: string; icon: string; has_pipelines: boolean; has_amount: boolean; has_client: boolean };
	let draft = $state<Draft | null>(null);
	let open = $state(false);
	let saving = $state(false);

	function edit(t: CrmType | null) {
		draft = t
			? { code: t.code, kind: t.kind, name: t.name, name_plural: t.name_plural, icon: t.icon, has_pipelines: t.has_pipelines, has_amount: t.has_amount, has_client: t.has_client }
			: { code: null, kind: null, name: '', name_plural: '', icon: 'folder', has_pipelines: false, has_amount: false, has_client: true };
		open = true;
	}

	async function save() {
		if (!draft) return;
		saving = true;
		try {
			const body = { ...draft, name_plural: draft.name_plural.trim() || draft.name };
			const t = draft.code ? await api.patch<CrmType>(`/crm/types/${draft.code}`, body) : await api.post<CrmType>('/crm/types', body);
			open = false;
			await crm.load(true);
			toasts.success(draft.code ? 'Record type saved' : `${t.name_plural} created — find them in the sidebar`);
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}

	async function remove() {
		if (!draft?.code || !confirm(`Delete ${draft.name_plural}? Its pipelines, fields, automation and access rights are removed. This is only possible when it has no records.`)) return;
		try {
			await api.del(`/crm/types/${draft.code}`);
			open = false;
			await crm.load(true);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<div class="space-y-4">
	<p class="max-w-3xl text-sm text-slate-600">
		Smart processes are your own record types, such as contracts, shipments or service requests. Each gets a pipeline with stages,
		custom fields, access rights, automation and document templates, like deals.
	</p>
	<section class="card max-w-3xl">
		<ul class="divide-y divide-slate-50">
			{#each types as t (t.code)}
				<li class="flex items-center gap-3 px-5 py-3 text-sm">
					<Icon name={t.icon as IconName} class="size-5 text-slate-500" />
					<div class="min-w-0 flex-1">
						<p class="font-medium text-slate-900">{t.name_plural}</p>
						<p class="text-xs text-slate-500">
							{#if t.is_system}Built in{:else if t.kind}Built in · /crm/{t.code}{:else}/crm/{t.code}{/if}
							{#if t.has_pipelines} · pipelines{/if}{#if t.has_amount} · amount &amp; products{/if}{#if t.has_client} · company &amp; contacts{/if}
						</p>
					</div>
					{#if !t.is_system}<button class="btn btn-ghost text-sm" onclick={() => edit(t)}><Icon name="edit" class="size-4" /> Edit</button>{/if}
				</li>
			{/each}
		</ul>
		<div class="border-t border-slate-100 px-5 py-3"><button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New smart process</button></div>
	</section>
</div>

<Modal bind:open title={draft?.kind ? `Edit ${draft.name_plural.toLowerCase()}` : draft?.code ? 'Edit smart process' : 'New smart process'}>
	{#if draft}
		<div class="space-y-4 text-sm">
			<div class="grid gap-3 sm:grid-cols-2">
				<div><label class="label" for="ty-name">Name (one record)</label><input id="ty-name" class="input" maxlength="60" placeholder="Contract" bind:value={draft.name} /></div>
				<div><label class="label" for="ty-plural">Plural</label><input id="ty-plural" class="input" maxlength="60" placeholder="Contracts" bind:value={draft.name_plural} /></div>
			</div>
			<div>
				<span class="label">Icon</span>
				<div class="flex flex-wrap gap-1.5">
					{#each ICONS as i (i)}
						<button type="button" class="rounded-lg border p-2 {draft.icon === i ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-500 hover:bg-slate-50'}" aria-label="Icon {i}" aria-pressed={draft.icon === i} onclick={() => (draft!.icon = i)}><Icon name={i} class="size-4" /></button>
					{/each}
				</div>
			</div>
			<div class="space-y-2">
				<label class="flex items-start gap-2"><input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={draft.has_client} disabled={!!draft.kind} /><span>Linked to a company and contacts <span class="block text-xs text-slate-500">Shows on their pages; documents are addressed to them.</span></span></label>
				<label class="flex items-start gap-2"><input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={draft.has_amount} disabled={!!draft.kind} /><span>Amount and products <span class="block text-xs text-slate-500">A currency amount, product rows with tax, and totals on the board.</span></span></label>
				<label class="flex items-start gap-2"><input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={draft.has_pipelines} /><span>Several pipelines <span class="block text-xs text-slate-500">Different stage sets, like sales pipelines for deals.</span></span></label>
			</div>
			{#if !draft.code}<p class="text-xs text-slate-500">It starts with the stages New, In progress, Done and Cancelled, and the same access rights as deals. Change them under Pipelines &amp; stages and Access rights.</p>{/if}
		</div>
	{/if}
	{#snippet footer()}
		{#if draft?.code && !draft.kind}<button class="btn btn-ghost mr-auto text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>{/if}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" disabled={saving || !draft?.name.trim()} onclick={save}>{saving ? 'Saving…' : draft?.code ? 'Save' : 'Create'}</button>
	{/snippet}
</Modal>
