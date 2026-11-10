<script lang="ts">
	import { untrack } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import { crm, itemLink, linksCompany, linksContacts, optionLabel, type Address, type CardLayoutInfo, type CrmDetail, type CrmType, type Multifield } from '#lib/crm.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import UserPicker from '../UserPicker.svelte';
	import AddressEditor from './AddressEditor.svelte';
	import CrmPicker from './CrmPicker.svelte';
	import CustomFieldInput from './CustomFieldInput.svelte';
	import MultifieldEditor from './MultifieldEditor.svelte';

	let {
		type: typeProp,
		item: itemProp = null,
		defaults: defaultsProp = {},
		card: cardProp = null,
		onsaved,
		oncancel
	}: {
		type: CrmType;
		item?: CrmDetail | null;
		defaults?: Record<string, unknown>;
		/** The card layout: which fields the form shows, in which sections. */
		card?: CardLayoutInfo | null;
		onsaved: (d: CrmDetail) => void;
		oncancel: () => void;
	} = $props();

	// The form edits a snapshot taken when it opens; live updates don't overwrite typing.
	const { type, item, defaults, card } = untrack(() => ({ type: typeProp, item: itemProp, defaults: defaultsProp, card: cardProp }));
	// Without a layout (still loading), every card field shows in one section.
	const sections = card?.layout.sections ?? [
		{ id: 'all', title: 'Details', fields: [...new Set([...type.system_fields.map((x) => x.code), ...type.custom_fields.map((d) => `uf.${d.code}`), 'contact_details', 'comments'])] }
	];
	const fieldLabel = (code: string) => card?.fields.find((x) => x.code === code)?.label ?? type.system_fields.find((x) => x.code === code)?.label ?? code;
	/** Card fields the form can edit (the campaign tags are set by web forms). */
	const editable = (code: string) =>
		code === 'contact_details' ? has('phone') : code === 'comments' ? true : code.startsWith('uf.') ? type.custom_fields.some((d) => `uf.${d.code}` === code) : code !== 'utm' && has(code);
	const WIDE = new Set(['contact_details', 'address', 'comments', 'source_details']);
	/** Sections a new record's form shows straight away: the first, and any with contact
	 * details or a required field (the rest wait behind "More fields"). */
	const upfront = (sec: { fields: string[] }, i: number) =>
		i === 0 || sec.fields.some((c) => c === 'contact_details' || type.custom_fields.some((d) => d.is_required && `uf.${d.code}` === c));
	const later = () => sections.filter((x, i) => !upfront(x, i) && x.fields.some(editable));

	const has = (code: string) => type.system_fields.some((f) => f.code === code);
	const opts = (k: string) => crm.config?.options[k] ?? [];
	const isNew = !item;

	// Editable copy of the record.
	let f = $state({
		title: item?.title ?? '',
		honorific: item?.honorific ?? '',
		first_name: item?.first_name ?? '',
		middle_name: item?.middle_name ?? '',
		last_name: item?.last_name ?? '',
		job_title: item?.job_title ?? '',
		birthdate: item?.birthdate ?? '',
		company_name: item?.company_name ?? '',
		company_kind: item?.company_kind ?? '',
		industry: item?.industry ?? '',
		employees: item?.employees ?? '',
		annual_revenue: item?.annual_revenue ?? '',
		opportunity: item?.opportunity ? String(Number(item.opportunity)) : '',
		currency: item?.currency ?? crm.config?.base_currency ?? 'USD',
		probability: item?.probability ?? null,
		close_date: item?.close_date ?? '',
		source: item?.source ?? '',
		source_details: item?.source_details ?? '',
		comments: item?.comments ?? '',
		category_id: item?.category_id ?? (defaults.category_id as string) ?? type.categories.find((c) => c.is_default)?.id ?? '',
		stage_id: item?.stage_id ?? (defaults.stage_id as string) ?? '',
		assigned_to: item?.assigned_to ?? session.user!.id,
		assigned_name: item?.assigned_name ?? session.user!.full_name
	});
	let multifields = $state<Multifield[]>(item?.multifields.map((m) => ({ ...m })) ?? []);
	let address = $state<Address | null>(item?.address ?? null);
	let company = $state<{ id: string; title: string } | null>(
		item?.company_id ? { id: item.company_id, title: item.company_title ?? '' } : (defaults.company as { id: string; title: string }) ?? null
	);
	let contacts = $state<{ id: string; title: string }[]>(
		item?.contacts.map((c) => ({ id: c.id, title: c.title })) ?? ((defaults.contacts as { id: string; title: string }[]) ?? [])
	);
	// Every custom field gets a key (null when empty): binding an input to a missing key
	// would hand it `undefined`.
	let uf = $state<Record<string, unknown>>({ ...Object.fromEntries(type.custom_fields.map((d) => [d.code, null])), ...(item?.uf ?? {}) });
	let saving = $state(false);
	let showMore = $state(!isNew);

	const category = $derived(type.categories.find((c) => c.id === f.category_id) ?? type.categories[0]);

	// ----- duplicate warning -----
	type Dup = { id: string; number: number; title: string; reason: string };
	let dups = $state<{ items: Dup[]; hidden_count: number } | null>(null);
	let dupTimer: ReturnType<typeof setTimeout>;
	$effect(() => {
		if (type.code === 'deal') return;
		const phones = multifields.filter((m) => m.kind === 'phone' && m.value.trim()).map((m) => m.value).join(',');
		const emails = multifields.filter((m) => m.kind === 'email' && m.value.trim()).map((m) => m.value).join(',');
		const name =
			type.code === 'company' ? f.company_name : type.code === 'contact' ? [f.first_name, f.middle_name, f.last_name].filter(Boolean).join(' ') : '';
		clearTimeout(dupTimer);
		if (!phones && !emails && name.trim().length < 3) {
			dups = null;
			return;
		}
		dupTimer = setTimeout(async () => {
			dups = await api.get<{ items: Dup[]; hidden_count: number }>(`/crm/${type.code}/duplicates` + query({ phones, emails, name, exclude: item?.id })).catch(() => null);
		}, 400);
	});

	function body(): Record<string, unknown> {
		const b: Record<string, unknown> = {};
		const put = (k: string, v: unknown) => {
			if (has(k) || ['category_id', 'stage_id', 'company_id', 'contact_ids', 'uf', 'multifields', 'is_manual_amount'].includes(k)) b[k] = v === '' ? null : v;
		};
		for (const k of ['title', 'honorific', 'first_name', 'middle_name', 'last_name', 'job_title', 'birthdate', 'company_name', 'company_kind', 'industry', 'employees', 'annual_revenue', 'close_date', 'source', 'source_details', 'comments'] as const)
			put(k, f[k]);
		put('assigned_to', f.assigned_to);
		if (type.has_amount) {
			if (!item || item.is_manual_amount) put('opportunity', f.opportunity || '0');
			put('currency', f.currency);
		}
		if (type.code === 'deal') put('probability', f.probability === null || (f.probability as unknown) === '' ? null : Number(f.probability));
		if (type.has_stages && f.stage_id) put('stage_id', f.stage_id);
		if (type.has_pipelines && f.category_id) put('category_id', f.category_id);
		if (has('address')) b.address = address;
		if (has('phone')) b.multifields = multifields.filter((m) => m.value.trim());
		if (linksCompany(type)) b.company_id = company?.id ?? null;
		if (linksContacts(type)) b.contact_ids = contacts.map((c) => c.id);
		const ufClean: Record<string, unknown> = {};
		for (const d of type.custom_fields) ufClean[d.code] = uf[d.code] === '' || uf[d.code] === undefined ? null : uf[d.code];
		if (type.custom_fields.length) b.uf = ufClean;
		return b;
	}

	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const saved = item
				? await api.patch<CrmDetail>(`/crm/${type.code}/items/${item.id}`, body())
				: await api.post<CrmDetail>(`/crm/${type.code}/items`, body());
			toasts.success(item ? 'Saved' : `${type.name} created`);
			onsaved(saved);
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
</script>

{#snippet input(code: string)}
	{#if code === 'close_date'}
		<label class="label" for="crm-close">{fieldLabel(code)}</label><input id="crm-close" type="date" class="input" bind:value={f.close_date} />
	{:else if code === 'probability'}
		<label class="label" for="crm-prob">{fieldLabel(code)}</label><input id="crm-prob" type="number" min="0" max="100" class="input" bind:value={f.probability} />
	{:else if code === 'source'}
		<label class="label" for="crm-source">{fieldLabel(code)}</label>
		<select id="crm-source" class="input" bind:value={f.source}>
			<option value="">—</option>
			{#each opts('source') as o (o)}<option value={o}>{optionLabel(o)}</option>{/each}
		</select>
	{:else if code === 'source_details'}
		<label class="label" for="crm-sdet">{fieldLabel(code)}</label><input id="crm-sdet" class="input" maxlength="2000" bind:value={f.source_details} />
	{:else if code === 'company_kind' || code === 'industry'}
		{@const id = code === 'industry' ? 'crm-ind' : 'crm-kind'}
		<label class="label" for={id}>{fieldLabel(code)}</label>
		<select {id} class="input" bind:value={f[code]}><option value="">—</option>{#each opts(code) as o (o)}<option value={o}>{optionLabel(o)}</option>{/each}</select>
	{:else if code === 'employees'}
		<label class="label" for="crm-emp">{fieldLabel(code)}</label>
		<select id="crm-emp" class="input" bind:value={f.employees}><option value="">—</option>{#each opts('employees') as o (o)}<option value={o}>{o}</option>{/each}</select>
	{:else if code === 'annual_revenue'}
		<label class="label" for="crm-rev">{fieldLabel(code)} ({crm.config?.base_currency})</label><input id="crm-rev" type="number" min="0" class="input" bind:value={f.annual_revenue} />
	{:else if code === 'birthdate'}
		<label class="label" for="crm-bd">{fieldLabel(code)}</label><input id="crm-bd" type="date" class="input" bind:value={f.birthdate} />
	{:else if code === 'contact_details'}
		<span class="label">{fieldLabel(code)}</span>
		<MultifieldEditor bind:value={multifields} kinds={type.code === 'company' ? ['phone', 'email', 'web'] : ['phone', 'email', 'web', 'im']} />
	{:else if code === 'address'}
		<span class="label">{fieldLabel(code)}</span>
		<AddressEditor bind:value={address} />
	{:else if code === 'comments'}
		<label class="label" for="crm-comments">{fieldLabel(code)}</label><textarea id="crm-comments" class="input" rows="3" bind:value={f.comments}></textarea>
	{:else if code.startsWith('uf.')}
		{@const d = type.custom_fields.find((x) => `uf.${x.code}` === code)!}
		<label class="label" for="uf-{d.code}">{d.label}{d.is_required ? ' *' : ''}</label>
		<CustomFieldInput def={d} bind:value={uf[d.code]} />
	{/if}
{/snippet}

<form class="space-y-6" onsubmit={save}>
	{#if dups && (dups.items.length || dups.hidden_count)}
		<div class="rounded-xl bg-amber-50 px-4 py-3 text-sm text-amber-900">
			<p class="font-medium">Possible duplicate{dups.items.length + dups.hidden_count > 1 ? 's' : ''}</p>
			<ul class="mt-1 space-y-0.5">
				{#each dups.items as d (d.id)}
					<li><a class="underline" href={itemLink(type.code, d.id)} target="_blank">#{d.number} {d.title}</a> <span class="text-amber-700">· same {d.reason}</span></li>
				{/each}
				{#if dups.hidden_count}<li class="text-amber-700">…and {dups.hidden_count} you don't have access to</li>{/if}
			</ul>
		</div>
	{/if}

	<section class="grid gap-4 sm:grid-cols-2">
		{#if has('title')}
			<div class="sm:col-span-2">
				<label class="label" for="crm-title">{type.code === 'deal' ? 'Deal name' : type.code === 'lead' ? 'Lead name' : 'Name'}</label>
				<input id="crm-title" class="input" maxlength="300" placeholder={type.code === 'lead' ? 'e.g. Website enquiry about pricing' : type.code === 'deal' ? 'e.g. 50 licences for Globex' : ''} bind:value={f.title} />
			</div>
		{/if}
		{#if has('first_name')}
			<div class="grid grid-cols-[6rem_1fr] gap-2 sm:col-span-2 sm:grid-cols-[6rem_1fr_1fr_1fr]">
				<div><label class="label" for="crm-hon">Salutation</label><input id="crm-hon" class="input" placeholder="Ms." bind:value={f.honorific} /></div>
				<div><label class="label" for="crm-first">First name</label><input id="crm-first" class="input" bind:value={f.first_name} /></div>
				<div><label class="label" for="crm-middle">Middle name</label><input id="crm-middle" class="input" bind:value={f.middle_name} /></div>
				<div><label class="label" for="crm-last">Last name</label><input id="crm-last" class="input" bind:value={f.last_name} /></div>
			</div>
			<div><label class="label" for="crm-job">Position</label><input id="crm-job" class="input" bind:value={f.job_title} /></div>
		{/if}
		{#if has('company_name')}
			<div class={type.code === 'company' ? 'sm:col-span-2' : ''}>
				<label class="label" for="crm-cname">Company name</label>
				<input id="crm-cname" class="input" required={type.code === 'company'} bind:value={f.company_name} />
			</div>
		{/if}
		{#if type.has_amount}
			<div>
				<label class="label" for="crm-amount">Amount</label>
				<div class="flex gap-2">
					<input id="crm-amount" class="input" type="number" min="0" step="0.01" disabled={item ? !item.is_manual_amount : false} bind:value={f.opportunity} />
					<select class="input w-28" bind:value={f.currency} aria-label="Currency">
						{#each crm.config?.currencies ?? [] as c (c.code)}<option>{c.code}</option>{/each}
					</select>
				</div>
				{#if item && !item.is_manual_amount}<p class="mt-1 text-xs text-slate-500">Calculated from products</p>{/if}
			</div>
		{/if}
		{#if type.has_pipelines && type.categories.length > 1}
			<div>
				<label class="label" for="crm-cat">Pipeline</label>
				<select id="crm-cat" class="input" bind:value={f.category_id} onchange={() => (f.stage_id = '')}>
					{#each type.categories as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
				</select>
			</div>
		{/if}
		{#if type.has_stages && category}
			<div>
				<label class="label" for="crm-stage">Stage</label>
				<select id="crm-stage" class="input" bind:value={f.stage_id}>
					<option value="">{isNew ? 'First stage' : '—'}</option>
					{#each category.stages as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
				</select>
			</div>
		{/if}
		{#if linksCompany(type)}
			<div>
				<span class="label">Company</span>
				{#if company}
					<div class="flex h-10 items-center justify-between rounded-lg border border-slate-300 px-3 text-sm">
						<span class="truncate">{company.title}</span>
						<button type="button" class="text-slate-400 hover:text-red-600" aria-label="Remove company" onclick={() => (company = null)}><Icon name="x" class="size-4" /></button>
					</div>
				{:else}
					<CrmPicker types={['company']} placeholder="Find a company…" onpick={(h) => (company = { id: h.id, title: h.title })} />
				{/if}
			</div>
		{/if}
		{#if linksContacts(type)}
			<div class={type.code === 'company' ? 'sm:col-span-2' : ''}>
				<span class="label">Contacts</span>
				<div class="mb-2 flex flex-wrap gap-1.5">
					{#each contacts as c (c.id)}
						<span class="inline-flex items-center gap-1 rounded-full bg-slate-100 py-0.5 pr-1 pl-2.5 text-sm">
							{c.title}<button type="button" class="rounded-full p-0.5 text-slate-400 hover:text-red-600" aria-label="Remove {c.title}" onclick={() => (contacts = contacts.filter((x) => x.id !== c.id))}><Icon name="x" class="size-3" /></button>
						</span>
					{/each}
				</div>
				<CrmPicker types={['contact']} placeholder="Add a contact…" exclude={contacts.map((c) => c.id)} onpick={(h) => (contacts = [...contacts, { id: h.id, title: h.title }])} />
			</div>
		{/if}
		<div>
			<span class="label">Responsible</span>
			<UserPicker bind:value={f.assigned_to} bind:label={f.assigned_name} />
		</div>
	</section>

	{#each sections as sec, si (sec.id)}
		{@const codes = sec.fields.filter(editable)}
		{#if codes.length && (showMore || upfront(sec, si))}
			<section class="border-t border-slate-100 pt-5" data-form-section={sec.id}>
				<h3 class="mb-3 text-sm font-medium text-slate-600">{sec.title}</h3>
				<div class="grid gap-4 sm:grid-cols-2">
					{#each codes as code (code)}
						<div class={WIDE.has(code) ? 'sm:col-span-2' : ''}>{@render input(code)}</div>
					{/each}
				</div>
			</section>
		{/if}
	{/each}
	{#if !showMore && later().length}
		<button type="button" class="text-sm font-medium text-brand-600 hover:underline" onclick={() => (showMore = true)}>More fields: {later().map((x) => x.title.toLowerCase()).join(', ')}</button>
	{/if}

	<div class="flex gap-2 border-t border-slate-100 pt-5">
		<button class="btn btn-primary" disabled={saving}>{saving ? 'Saving…' : isNew ? `Create ${type.name.toLowerCase()}` : 'Save'}</button>
		<button type="button" class="btn btn-secondary" onclick={oncancel}>Cancel</button>
	</div>
</form>
