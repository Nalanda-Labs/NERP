<script lang="ts">
	// CRM settings → Documents → Signing templates: who signs (by role, filled in from each
	// record), the message, and optionally the document template to make the document from.
	// Plus the company switch that limits e-signature to people with its permission.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { crm } from '#lib/crm.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';

	type Signer = { role: string; name?: string; email?: string };
	type Tpl = { id: string; name: string; message: string | null; doc_template_id: string | null; doc_template_name: string | null; signers: Signer[]; entity_types: number[] };
	let items = $state<Tpl[]>([]);
	let roles = $state<{ role: string; label: string }[]>([]);
	let docTemplates = $state<{ id: string; name: string }[]>([]);
	let restricted = $state(false);
	const canSettings = $derived(session.can('settings.manage'));

	async function load() {
		const r = await api.get<{ items: Tpl[]; roles: typeof roles }>('/crm/sign-templates');
		items = r.items;
		roles = r.roles;
	}
	onMount(async () => {
		crm.load();
		await load();
		docTemplates = (await api.get<{ items: { id: string; name: string }[] }>('/crm/doc-templates').catch(() => ({ items: [] }))).items;
		restricted = ((await api.get<Record<string, unknown>>('/settings').catch(() => ({}))) as Record<string, unknown>).esign_restricted === true;
	});
	async function setRestricted() {
		try {
			await api.put('/settings', { esign_restricted: restricted });
			toasts.success(restricted ? 'Only people with “Send for e-signature” send documents now' : 'Everyone who can change a record may send its documents');
		} catch (e) {
			restricted = !restricted;
			toasts.error(e);
		}
	}

	let open = $state(false);
	let draft = $state<{ id: string | null; name: string; message: string; doc_template_id: string; signers: Signer[]; entity_types: number[] }>({ id: null, name: '', message: '', doc_template_id: '', signers: [], entity_types: [] });
	function edit(t: Tpl | null) {
		draft = t
			? { id: t.id, name: t.name, message: t.message ?? '', doc_template_id: t.doc_template_id ?? '', signers: t.signers.map((s) => ({ ...s })), entity_types: [...t.entity_types] }
			: { id: null, name: '', message: '', doc_template_id: '', signers: [{ role: 'contact' }], entity_types: [] };
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		const body = { name: draft.name, message: draft.message || null, doc_template_id: draft.doc_template_id || null, signers: draft.signers, entity_types: draft.entity_types };
		try {
			if (draft.id) await api.patch(`/crm/sign-templates/${draft.id}`, body);
			else await api.post('/crm/sign-templates', body);
			open = false;
			await load();
			toasts.success('Signing template saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove() {
		if (!draft.id || !confirm(`Delete “${draft.name}”?`)) return;
		await api.del(`/crm/sign-templates/${draft.id}`);
		open = false;
		await load();
	}
	const roleLabel = (r: string) => roles.find((x) => x.role === r)?.label ?? r;
	const describe = (t: Tpl) => t.signers.map((s) => (s.role === 'person' ? s.name : roleLabel(s.role))).join(', ');
</script>

<section class="card" aria-labelledby="sign-tpl-h" data-sign-templates>
	<div class="flex flex-wrap items-center gap-3 border-b border-slate-100 px-5 py-3">
		<div class="flex-1">
			<h2 id="sign-tpl-h" class="font-semibold text-slate-900">Signing templates</h2>
			<p class="text-xs text-slate-500">Who signs a record's documents, filled in from each record. Use them when sending for signature, and in automation.</p>
		</div>
		<button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New signing template</button>
	</div>
	{#if canSettings}
		<label class="flex items-center gap-2 border-b border-slate-100 px-5 py-2.5 text-sm">
			<input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={restricted} onchange={setRestricted} data-esign-restricted />
			Only people with the “Send for e-signature” permission (and CRM admins) send documents for signature
		</label>
	{/if}
	<ul class="divide-y divide-slate-50">
		{#each items as t (t.id)}
			<li>
				<button class="flex w-full items-center gap-3 px-5 py-2.5 text-left text-sm hover:bg-slate-50" onclick={() => edit(t)} data-sign-template={t.name}>
					<span class="flex-1"><span class="font-medium text-slate-900">{t.name}</span><br /><span class="text-xs text-slate-500">{describe(t)}{t.doc_template_name ? ` · makes “${t.doc_template_name}”` : ''}</span></span>
				</button>
			</li>
		{:else}
			<li class="px-5 py-4 text-sm text-slate-400">No signing templates yet.</li>
		{/each}
	</ul>
</section>

<Modal bind:open title={draft.id ? 'Edit signing template' : 'New signing template'} size="lg">
	<form id="sign-tpl-form" class="space-y-3 text-sm" onsubmit={save}>
		<div><label class="label" for="st-name">Name</label><input id="st-name" class="input" required maxlength="120" bind:value={draft.name} placeholder="e.g. Contract with the client" /></div>
		<div>
			<span class="label">Signers, in order</span>
			<div class="space-y-2">
				{#each draft.signers as s, i (i)}
					<div class="flex flex-wrap gap-2">
						<select class="input w-64" aria-label="Signer {i + 1}" bind:value={s.role}>{#each roles as r (r.role)}<option value={r.role}>{r.label}</option>{/each}</select>
						{#if s.role === 'person'}
							<input class="input min-w-0 flex-1" required maxlength="200" placeholder="Name" aria-label="Signer {i + 1} name" bind:value={s.name} />
							<input class="input min-w-0 flex-1" type="email" required placeholder="Email" aria-label="Signer {i + 1} email" bind:value={s.email} />
						{/if}
						{#if draft.signers.length > 1}<button type="button" class="btn btn-ghost p-1.5" aria-label="Remove signer" onclick={() => draft.signers.splice(i, 1)}><Icon name="x" class="size-4" /></button>{/if}
					</div>
				{/each}
			</div>
			{#if draft.signers.length < 10}<button type="button" class="mt-1 text-sm font-medium text-brand-600 hover:underline" onclick={() => draft.signers.push({ role: 'responsible' })}>+ Add signer</button>{/if}
		</div>
		<div>
			<label class="label" for="st-doc">Document</label>
			<select id="st-doc" class="input" bind:value={draft.doc_template_id}>
				<option value="">The record's newest ready document</option>
				{#each docTemplates as d (d.id)}<option value={d.id}>Make it from “{d.name}”</option>{/each}
			</select>
		</div>
		<div><label class="label" for="st-msg">Message to the signers <span class="font-normal text-slate-400">(optional)</span></label><textarea id="st-msg" class="input" rows="2" maxlength="2000" bind:value={draft.message}></textarea></div>
		<div>
			<span class="label">For</span>
			<div class="flex flex-wrap gap-3">
				{#each crm.config?.types ?? [] as ty (ty.id)}
					<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" value={ty.id} bind:group={draft.entity_types} /> {ty.name}</label>
				{/each}
			</div>
			<p class="mt-1 text-xs text-slate-500">None ticked: every kind of record.</p>
		</div>
	</form>
	{#snippet footer()}
		{#if draft.id}<button class="btn btn-ghost mr-auto text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>{/if}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="sign-tpl-form">Save</button>
	{/snippet}
</Modal>
