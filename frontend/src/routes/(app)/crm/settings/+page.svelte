<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import DocTemplates from '#lib/components/crm/DocTemplates.svelte';
	import Automation from '#lib/components/crm/Automation.svelte';
	import TypeManager from '#lib/components/crm/TypeManager.svelte';
	import { allCurrencies, crm, currencyName, type CrmCategory, type CrmType, type FieldDef } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	const TABS = [['types', 'Record types'], ['pipelines', 'Pipelines & stages'], ['fields', 'Custom fields'], ['currencies', 'Currencies'], ['automation', 'Automation'], ['documents', 'Documents'], ['access', 'Access rights']] as const;
	type Tab = (typeof TABS)[number][0];
	// The tab lives in the address (?tab=documents) so every tab can be bookmarked and shared.
	const tab = $derived<Tab>(TABS.find(([k]) => k === page.url.searchParams.get('tab'))?.[0] ?? 'pipelines');
	let typeCode = $state('deal');
	const type = $derived(crm.config?.types.find((t) => t.code === typeCode) as CrmType | undefined);

	onMount(() => crm.load(true));
	async function reload() {
		await crm.load(true);
	}
	async function run<T>(p: Promise<T>): Promise<T | undefined> {
		try {
			const r = await p;
			await reload();
			return r;
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- pipelines & stages -----
	let newPipeline = $state('');
	let newStage = $state<Record<string, { name: string; semantics: string }>>({});
	function moveStage(c: CrmCategory, i: number, dir: -1 | 1) {
		const ids = c.stages.map((s) => s.id);
		[ids[i], ids[i + dir]] = [ids[i + dir]!, ids[i]!];
		run(api.put(`/crm/categories/${c.id}/stage-order`, { ids }));
	}

	// ----- custom fields -----
	const KINDS = [['string', 'Text'], ['text', 'Long text'], ['number', 'Number'], ['money', 'Money'], ['date', 'Date'], ['datetime', 'Date & time'], ['boolean', 'Yes / no'], ['list', 'List'], ['user', 'Employee'], ['url', 'Link'], ['address', 'Address']];
	let nf = $state({ label: '', kind: 'string', is_required: false, show_in_list: false, is_multiple: false, options: '' });
	async function addField(e: SubmitEvent) {
		e.preventDefault();
		const settings = nf.kind === 'list' ? { options: nf.options.split('\n').map((l) => l.trim()).filter(Boolean).map((label) => ({ label })) } : {};
		const r = await run(api.post(`/crm/${typeCode}/fields`, { label: nf.label, kind: nf.kind, is_required: nf.is_required, show_in_list: nf.show_in_list, is_multiple: nf.is_multiple, settings }));
		if (r) nf = { label: '', kind: 'string', is_required: false, show_in_list: false, is_multiple: false, options: '' };
	}
	const patchField = (d: FieldDef, body: Record<string, unknown>) => run(api.patch(`/crm/${typeCode}/fields/${d.id}`, body));
	async function removeField(d: FieldDef) {
		if (confirm(`Delete the field "${d.label}"? Its values will no longer be shown.`)) await run(api.del(`/crm/${typeCode}/fields/${d.id}`));
	}

	// ----- currencies -----
	const ALL = allCurrencies();
	let nc = $state({ code: '', rate: '' });
	async function addCurrency(e: SubmitEvent) {
		e.preventDefault();
		if (await run(api.post('/crm/currencies', nc))) nc = { code: '', rate: '' };
	}

	// ----- access rights -----
	type Matrix = { roles: { id: string; code: string; name: string; full_access: boolean }[]; actions: string[]; scopes: string[]; matrix: Record<string, Record<string, Record<string, string>>> };
	let access = $state<Matrix | null>(null);
	let roleId = $state('');
	const ACTION_LABELS: Record<string, string> = { read: 'View', add: 'Add', write: 'Edit', delete: 'Delete', export: 'Export', import: 'Import' };
	const SCOPE_LABELS: Record<string, string> = { none: 'None', own: 'Own', department: 'Department', subdepartments: 'Dept. + sub-depts.', all: 'All' };
	async function loadAccess() {
		access = await api.get<Matrix>('/crm/permissions');
		roleId ||= access.roles.find((r) => !r.full_access)?.id ?? access.roles[0]!.id;
	}
	$effect(() => {
		if (tab === 'access' && !access) loadAccess();
	});
	async function setScope(t: number, action: string, scope: string) {
		const m = access!.matrix[roleId] ?? {};
		m[t] = { ...(m[t] ?? {}), [action]: scope };
		const entries = Object.entries(m).flatMap(([et, acts]) => Object.entries(acts).map(([a, s]) => ({ entity_type: Number(et), action: a, scope: s })));
		try {
			access = await api.put<Matrix>(`/crm/permissions/${roleId}`, { entries });
			toasts.success('Access rights saved');
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>CRM settings · NERP</title></svelte:head>

<div class="{tab === 'documents' || tab === 'automation' ? 'max-w-none' : 'max-w-5xl'} space-y-5">
	<h1 class="text-2xl font-semibold text-slate-900">CRM settings</h1>
	<nav class="flex flex-wrap gap-1">
		{#each TABS as [k, l] (k)}
			<a href="?tab={k}" class="btn {tab === k ? 'bg-brand-100 text-brand-900' : 'btn-ghost'}" aria-current={tab === k ? 'page' : undefined} data-sveltekit-replacestate data-sveltekit-noscroll>{l}</a>
		{/each}
	</nav>

	{#if crm.config && !crm.config.is_admin}
		<p class="card p-6 text-sm text-slate-600">You need the “Configure CRM” permission to change these settings.</p>
	{:else if crm.config}
		{#if tab === 'pipelines' || tab === 'fields'}
			<select class="input w-48" bind:value={typeCode} aria-label="Record type">
				{#each crm.config.types.filter((t) => tab === 'fields' || t.has_stages) as t (t.code)}<option value={t.code}>{t.name_plural}</option>{/each}
			</select>
		{/if}

		{#if tab === 'pipelines' && type}
			{#each type.categories as c (c.id)}
				<section class="card">
					<div class="flex flex-wrap items-center gap-2 border-b border-slate-100 px-5 py-3">
						<input class="input w-64 font-medium" value={c.name} aria-label="Pipeline name" onchange={(e) => run(api.patch(`/crm/categories/${c.id}`, { name: e.currentTarget.value }))} />
						{#if c.is_default}<span class="badge bg-brand-50 text-brand-700">Default</span>
						{:else if type.has_pipelines}
							<button class="btn btn-ghost text-sm" onclick={() => run(api.patch(`/crm/categories/${c.id}`, { is_default: true }))}>Make default</button>
							<button class="btn btn-ghost ml-auto text-sm text-red-600" onclick={() => confirm(`Delete pipeline ${c.name}?`) && run(api.del(`/crm/categories/${c.id}`))}>Delete pipeline</button>
						{/if}
					</div>
					<ul class="divide-y divide-slate-50">
						{#each c.stages as s, i (s.id)}
							<li class="flex flex-wrap items-center gap-2 px-5 py-2">
								<div class="flex flex-col">
									<button class="text-slate-400 hover:text-slate-800 disabled:opacity-20" disabled={i === 0} aria-label="Up" onclick={() => moveStage(c, i, -1)}><Icon name="chevronDown" class="size-4 rotate-180" /></button>
									<button class="text-slate-400 hover:text-slate-800 disabled:opacity-20" disabled={i === c.stages.length - 1} aria-label="Down" onclick={() => moveStage(c, i, 1)}><Icon name="chevronDown" class="size-4" /></button>
								</div>
								<input type="color" class="h-8 w-10 cursor-pointer rounded border-0" value={s.color} aria-label="Colour" onchange={(e) => run(api.patch(`/crm/stages/${s.id}`, { color: e.currentTarget.value }))} />
								<input class="input w-56" value={s.name} aria-label="Stage name" onchange={(e) => run(api.patch(`/crm/stages/${s.id}`, { name: e.currentTarget.value }))} />
								<span class="badge {s.semantics === 'success' ? 'bg-emerald-50 text-emerald-700' : s.semantics === 'failure' ? 'bg-red-50 text-red-700' : 'bg-slate-100 text-slate-600'}">
									{s.semantics === 'success' ? 'Success' : s.semantics === 'failure' ? 'Failure' : 'In progress'}
								</span>
								<button class="btn btn-ghost ml-auto p-1.5 text-slate-400 hover:text-red-600" aria-label="Delete stage" onclick={() => confirm(`Delete stage ${s.name}? Its records move to another stage of the same kind.`) && run(api.del(`/crm/stages/${s.id}`))}><Icon name="trash" class="size-4" /></button>
							</li>
						{/each}
					</ul>
					<form class="flex flex-wrap gap-2 border-t border-slate-100 px-5 py-3" onsubmit={(e) => { e.preventDefault(); const n = newStage[c.id]; if (n?.name) run(api.post(`/crm/categories/${c.id}/stages`, n)).then(() => (newStage[c.id] = { name: '', semantics: 'process' })); }}>
						<input class="input w-56" placeholder="New stage" value={newStage[c.id]?.name ?? ''} oninput={(e) => (newStage[c.id] = { semantics: newStage[c.id]?.semantics ?? 'process', name: e.currentTarget.value })} />
						<select class="input w-40" value={newStage[c.id]?.semantics ?? 'process'} onchange={(e) => (newStage[c.id] = { name: newStage[c.id]?.name ?? '', semantics: e.currentTarget.value })} aria-label="Stage kind">
							<option value="process">In progress</option><option value="success">Success</option><option value="failure">Failure</option>
						</select>
						<button class="btn btn-secondary">Add stage</button>
					</form>
				</section>
			{/each}
			{#if type.has_pipelines}
				<form class="flex gap-2" onsubmit={(e) => { e.preventDefault(); run(api.post(`/crm/${typeCode}/categories`, { name: newPipeline })).then(() => (newPipeline = '')); }}>
					<input class="input w-64" placeholder="New pipeline name" required bind:value={newPipeline} />
					<button class="btn btn-primary"><Icon name="plus" class="size-4" /> Add pipeline</button>
				</form>
			{/if}
		{:else if tab === 'fields' && type}
			<section class="card">
				<ul class="divide-y divide-slate-50">
					{#each type.custom_fields as d (d.id)}
						<li class="flex flex-wrap items-center gap-3 px-5 py-3 text-sm">
							<input class="input w-56" value={d.label} aria-label="Label" onchange={(e) => patchField(d, { label: e.currentTarget.value })} />
							<span class="badge bg-slate-100 text-slate-600">{KINDS.find((k) => k[0] === d.kind)?.[1]}{d.is_multiple ? ' (multiple)' : ''}</span>
							<code class="text-xs text-slate-400">{d.code}</code>
							<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={d.is_required} onchange={(e) => patchField(d, { is_required: e.currentTarget.checked })} /> Required</label>
							<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={d.show_in_list} onchange={(e) => patchField(d, { show_in_list: e.currentTarget.checked })} /> Show in list</label>
							<button class="btn btn-ghost ml-auto p-1.5 text-slate-400 hover:text-red-600" aria-label="Delete field" onclick={() => removeField(d)}><Icon name="trash" class="size-4" /></button>
						</li>
					{:else}
						<li class="px-5 py-6 text-sm text-slate-500">No custom fields for {type.name_plural.toLowerCase()} yet.</li>
					{/each}
				</ul>
			</section>
			<form class="card grid gap-3 p-5 sm:grid-cols-2" onsubmit={addField}>
				<h2 class="font-medium text-slate-900 sm:col-span-2">New field</h2>
				<div><label class="label" for="nf-l">Label</label><input id="nf-l" class="input" required bind:value={nf.label} /></div>
				<div><label class="label" for="nf-k">Type</label><select id="nf-k" class="input" bind:value={nf.kind}>{#each KINDS as [k, l] (k)}<option value={k}>{l}</option>{/each}</select></div>
				{#if nf.kind === 'list'}<div class="sm:col-span-2"><label class="label" for="nf-o">Options — one per line</label><textarea id="nf-o" class="input" rows="4" required bind:value={nf.options}></textarea></div>{/if}
				<div class="flex flex-wrap gap-4 text-sm sm:col-span-2">
					<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={nf.is_required} /> Required</label>
					<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={nf.show_in_list} /> Show in list</label>
					<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={nf.is_multiple} /> Several values</label>
				</div>
				<div><button class="btn btn-primary">Add field</button></div>
			</form>
		{:else if tab === 'currencies'}
			<section class="card">
				<ul class="divide-y divide-slate-50">
					{#each crm.config.currencies as c (c.code)}
						<li class="flex flex-wrap items-center gap-3 px-5 py-3 text-sm">
							<span class="w-14 font-mono font-medium">{c.code}</span>
							<span class="min-w-40 flex-1 text-slate-600">{currencyName(c.code)}</span>
							{#if c.is_base}
								<span class="badge bg-brand-50 text-brand-700">Base currency</span>
							{:else}
								<span class="text-slate-500">1 {c.code} =</span>
								<input class="input w-36" type="number" step="any" min="0" value={Number(c.rate)} aria-label="Rate" onchange={(e) => run(api.patch(`/crm/currencies/${c.code}`, { rate: e.currentTarget.value }))} />
								<span class="text-slate-500">{crm.config.base_currency}</span>
								<button class="btn btn-ghost text-sm" onclick={() => confirm(`Make ${c.code} the base currency? Rates are recalculated so amounts keep their value.`) && run(api.patch(`/crm/currencies/${c.code}`, { is_base: true }))}>Make base</button>
								<button class="btn btn-ghost p-1.5 text-slate-400 hover:text-red-600" aria-label="Remove" onclick={() => run(api.del(`/crm/currencies/${c.code}`))}><Icon name="trash" class="size-4" /></button>
							{/if}
						</li>
					{/each}
				</ul>
			</section>
			<form class="card flex flex-wrap items-end gap-2 p-5" onsubmit={addCurrency}>
				<div><label class="label" for="nc-c">Add currency</label>
					<select id="nc-c" class="input w-72" required bind:value={nc.code}>
						<option value="">Choose…</option>
						{#each ALL.filter((x) => !crm.config?.currencies.some((c) => c.code === x.code)) as x (x.code)}<option value={x.code}>{x.code} — {x.name}</option>{/each}
					</select>
				</div>
				<div><label class="label" for="nc-r">1 unit = ? {crm.config.base_currency}</label><input id="nc-r" class="input w-36" type="number" step="any" min="0" required bind:value={nc.rate} /></div>
				<button class="btn btn-primary">Add</button>
			</form>
		{:else if tab === 'types'}
			<TypeManager />
		{:else if tab === 'automation'}
			<Automation />
		{:else if tab === 'documents'}
			<DocTemplates />
		{:else if tab === 'access' && access}
			<div class="flex flex-wrap items-center gap-3">
				<select class="input w-64" bind:value={roleId} aria-label="Role">{#each access.roles as r (r.id)}<option value={r.id}>{r.name}</option>{/each}</select>
				<p class="text-sm text-slate-500">Own = records they are responsible for. Department = also their colleagues' records.</p>
			</div>
			{#if access.roles.find((r) => r.id === roleId)?.full_access}
				<p class="card p-6 text-sm text-slate-600">This role has full CRM access (Administrator or “Configure CRM”).</p>
			{:else}
				<div class="card overflow-x-auto">
					<table class="min-w-full text-sm">
						<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500"><tr><th class="px-4 py-3"></th>{#each access.actions as a (a)}<th class="px-2 py-3">{ACTION_LABELS[a]}</th>{/each}</tr></thead>
						<tbody class="divide-y divide-slate-50">
							{#each crm.config.types as t (t.id)}
								<tr>
									<td class="px-4 py-2 font-medium text-slate-800">{t.name_plural}</td>
									{#each access.actions as a (a)}
										<td class="px-2 py-2">
											<select class="input min-w-32 py-1.5 text-xs" value={access.matrix[roleId]?.[t.id]?.[a] ?? 'none'} onchange={(e) => setScope(t.id, a, e.currentTarget.value)} aria-label="{t.name_plural} {ACTION_LABELS[a]}">
												{#each access.scopes as s (s)}<option value={s}>{SCOPE_LABELS[s]}</option>{/each}
											</select>
										</td>
									{/each}
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		{/if}
	{/if}
</div>
